use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Region,
    bus::Bus,
    bus::BusSnapshot,
    cartridge::{Cartridge, CartridgeError},
    controller::Controller,
    cpu::{Cpu, CpuError, CpuState},
    fceux_state::{FceuxState, FceuxStateError},
    ppu::{Frame, PpuState},
};

#[derive(Debug)]
pub enum EmulationError {
    Cartridge(CartridgeError),
    Cpu(CpuError),
}

const STATE_MAGIC: &[u8; 8] = b"MONESST\0";
pub const SAVE_STATE_VERSION: u32 = 6;

#[derive(Debug)]
pub enum StateError {
    InvalidHeader,
    UnsupportedVersion(u32),
    WrongRom,
    InvalidMapperState,
    Codec(String),
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHeader => write!(f, "not a CrabNes save state"),
            Self::UnsupportedVersion(version) => {
                write!(f, "save-state version {version} is not supported")
            }
            Self::WrongRom => write!(f, "save state belongs to a different ROM"),
            Self::InvalidMapperState => write!(f, "save state mapper data is incompatible"),
            Self::Codec(error) => write!(f, "invalid save-state data: {error}"),
        }
    }
}

impl Error for StateError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemorySpace {
    CpuRam,
    PpuNametable,
    Palette,
    Oam,
    PrgRom,
    Chr,
}

pub struct MemoryImage {
    pub bytes: Vec<u8>,
    pub base_address: usize,
    pub writable: bool,
}

#[derive(Serialize, Deserialize)]
struct MachineState {
    cpu: crate::cpu::Cpu,
    bus: BusSnapshot,
    powered: bool,
}

impl fmt::Display for EmulationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cartridge(error) => write!(f, "could not load cartridge: {error}"),
            Self::Cpu(error) => error.fmt(f),
        }
    }
}
impl Error for EmulationError {}
impl From<CartridgeError> for EmulationError {
    fn from(value: CartridgeError) -> Self {
        Self::Cartridge(value)
    }
}
impl From<CpuError> for EmulationError {
    fn from(value: CpuError) -> Self {
        Self::Cpu(value)
    }
}

pub struct Nes {
    cpu: Cpu,
    bus: Bus,
    powered: bool,
    rom_hash: u64,
    rom_sha256: [u8; 32],
    native_output_palette: Option<crate::ppu::OutputPalette>,
}

impl Nes {
    pub fn from_ines(bytes: &[u8]) -> Result<Self, EmulationError> {
        Self::from_cartridge(bytes, Cartridge::from_ines(bytes)?)
    }

    pub fn from_ines_with_region(bytes: &[u8], region: Region) -> Result<Self, EmulationError> {
        Self::from_cartridge(bytes, Cartridge::from_ines_with_region(bytes, region)?)
    }

    fn from_cartridge(bytes: &[u8], cartridge: Cartridge) -> Result<Self, EmulationError> {
        let native_output_palette = native_output_palette(bytes, cartridge.mapper_id());
        let mut bus = Bus::new(cartridge);
        if let Some(palette) = native_output_palette {
            bus.ppu.set_output_palette(palette);
        }
        let mut nes = Self {
            cpu: Cpu::default(),
            bus,
            powered: true,
            rom_hash: hash_rom(bytes),
            rom_sha256: Sha256::digest(bytes).into(),
            native_output_palette,
        };
        nes.bus.begin_cpu_sequence();
        nes.cpu.reset(&mut nes.bus);
        nes.bus.finish_cpu_sequence(7);
        Ok(nes)
    }

    /// Execute one CPU instruction with each CPU bus access interleaved with
    /// the active region's APU and PPU timing.
    pub fn step_instruction(&mut self) -> Result<u16, EmulationError> {
        if !self.powered {
            return Ok(0);
        }
        self.bus.begin_cpu_sequence();
        let cycle_result = if self.cpu.take_latched_nmi() {
            Ok(self.cpu.nmi(&mut self.bus))
        } else if self.cpu.take_polled_irq() {
            Ok(self.cpu.irq(&mut self.bus))
        } else if self.cpu.take_branch_irq_delay() {
            self.cpu.step(&mut self.bus)
        } else if self.bus.irq_pending() && !self.cpu.irq_masked_at_poll() {
            Ok(self.cpu.irq(&mut self.bus))
        } else {
            self.cpu.step(&mut self.bus)
        };
        let cycles = match cycle_result {
            Ok(cycles) => cycles,
            Err(error) => {
                self.bus.cancel_cpu_sequence();
                return Err(error.into());
            }
        };
        let actual_cycles = self.bus.finish_cpu_sequence(cycles);
        self.cpu.complete_interrupt_poll(&mut self.bus);
        Ok(actual_cycles)
    }

    /// Run until the PPU completes a video frame.
    pub fn run_frame(&mut self) -> Result<&Frame, EmulationError> {
        if !self.powered {
            return Ok(self.bus.ppu.frame());
        }
        while !self.bus.ppu.take_frame_complete() {
            self.step_instruction()?;
        }
        Ok(self.bus.ppu.frame())
    }

    /// Console reset: preserve cartridge RAM, but reset CPU/APU/PPU control state.
    pub fn reset(&mut self) {
        self.bus.reset();
        self.bus.begin_cpu_sequence();
        self.cpu.reset(&mut self.bus);
        self.bus.finish_cpu_sequence(7);
        self.powered = true;
    }

    pub fn power_off(&mut self) {
        self.powered = false;
    }
    pub fn power_on(&mut self) {
        if !self.powered {
            self.reset();
        }
    }
    pub fn powered(&self) -> bool {
        self.powered
    }
    pub fn frame(&self) -> &Frame {
        self.bus.ppu.frame()
    }
    pub fn cpu_state(&self) -> CpuState {
        self.cpu.state()
    }
    pub fn ppu_state(&self) -> PpuState {
        self.bus.ppu.state()
    }
    pub fn set_output_palette(&mut self, palette: crate::ppu::OutputPalette) {
        self.bus.ppu.set_output_palette(palette);
    }
    pub fn native_output_palette(&self) -> Option<crate::ppu::OutputPalette> {
        self.native_output_palette
    }
    pub fn cpu_cycles(&self) -> u64 {
        self.bus.cpu_cycles()
    }
    pub fn drain_audio_samples(&mut self, destination: &mut Vec<f32>) {
        self.bus.apu.drain_samples(destination);
    }
    pub fn audio_sample_rate(&self) -> u32 {
        self.bus.apu.sample_rate()
    }
    pub fn apu_state(&self) -> crate::apu::ApuState {
        self.bus.apu.state_for_region(self.bus.cartridge.region())
    }
    pub fn set_apu_channel_output_enabled(
        &mut self,
        channel: crate::apu::ApuChannel,
        enabled: bool,
    ) {
        self.bus.apu.set_channel_output_enabled(channel, enabled);
    }
    pub fn controller_mut(&mut self, port: usize) -> Option<&mut Controller> {
        self.bus.controllers.get_mut(port)
    }
    pub fn mapper_id(&self) -> u16 {
        self.bus.cartridge.mapper_id()
    }
    pub fn has_battery(&self) -> bool {
        self.bus.cartridge.has_battery()
    }
    pub fn battery_ram(&self) -> Option<&[u8]> {
        self.bus.cartridge.battery_ram()
    }
    pub fn load_battery_ram(&mut self, data: &[u8]) {
        self.bus.cartridge.load_battery_ram(data);
    }

    pub fn rom_hash(&self) -> u64 {
        self.rom_hash
    }

    pub fn rom_sha256(&self) -> [u8; 32] {
        self.rom_sha256
    }

    pub fn save_state(&self) -> Result<Vec<u8>, StateError> {
        let payload = bincode::serialize(&MachineState {
            cpu: self.cpu.clone(),
            bus: self.bus.snapshot(),
            powered: self.powered,
        })
        .map_err(|error| StateError::Codec(error.to_string()))?;
        let mut bytes = Vec::with_capacity(20 + payload.len());
        bytes.extend_from_slice(STATE_MAGIC);
        bytes.extend_from_slice(&SAVE_STATE_VERSION.to_le_bytes());
        bytes.extend_from_slice(&self.rom_hash.to_le_bytes());
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }

    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        if bytes.len() < 20 || &bytes[..8] != STATE_MAGIC {
            return Err(StateError::InvalidHeader);
        }
        let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if version != SAVE_STATE_VERSION {
            return Err(StateError::UnsupportedVersion(version));
        }
        let rom_hash = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
        if rom_hash != self.rom_hash {
            return Err(StateError::WrongRom);
        }
        let state: MachineState = bincode::deserialize(&bytes[20..])
            .map_err(|error| StateError::Codec(error.to_string()))?;
        if !self.bus.restore_snapshot(state.bus) {
            return Err(StateError::InvalidMapperState);
        }
        self.cpu = state.cpu;
        self.powered = state.powered;
        Ok(())
    }

    /// Import an FCEUX FCS state embedded in a text FM2 movie.
    ///
    /// FCS is emulator-internal rather than a console standard. This bridge
    /// deliberately accepts only the chunked state fields and mapper family
    /// that CrabNes can translate without guessing.
    pub fn import_fceux_state(&mut self, bytes: &[u8]) -> Result<(), FceuxStateError> {
        let state = FceuxState::parse(bytes)?;
        if self.mapper_id() != 4 {
            return Err(FceuxStateError::UnsupportedMapper(self.mapper_id()));
        }
        let mut cpu = self.cpu.clone();
        cpu.import_fceux_state(
            state.byte(1, b"A\0\0\0")?,
            state.byte(1, b"X\0\0\0")?,
            state.byte(1, b"Y\0\0\0")?,
            state.byte(1, b"S\0\0\0")?,
            state.word(1, b"PC\0\0")?,
            state.byte(1, b"P\0\0\0")?,
            state.byte(2, b"JAMM")? != 0,
            state.byte(2, b"MooP")?,
        );
        self.bus.import_fceux_state(&state)?;
        self.cpu = cpu;
        self.powered = true;
        Ok(())
    }

    pub fn memory_image(&self, space: MemorySpace) -> MemoryImage {
        match space {
            MemorySpace::CpuRam => MemoryImage {
                bytes: self.bus.cpu_ram().to_vec(),
                base_address: 0,
                writable: true,
            },
            MemorySpace::PpuNametable => MemoryImage {
                bytes: self.bus.ppu.nametable_memory().to_vec(),
                base_address: 0x2000,
                writable: true,
            },
            MemorySpace::Palette => MemoryImage {
                bytes: self.bus.ppu.palette_memory().to_vec(),
                base_address: 0x3f00,
                writable: true,
            },
            MemorySpace::Oam => MemoryImage {
                bytes: self.bus.ppu.oam_memory().to_vec(),
                base_address: 0,
                writable: true,
            },
            MemorySpace::PrgRom => MemoryImage {
                bytes: self.bus.cartridge.prg_rom().to_vec(),
                base_address: 0x8000,
                writable: false,
            },
            MemorySpace::Chr => MemoryImage {
                bytes: self.bus.cartridge.chr().to_vec(),
                base_address: 0,
                writable: self.bus.cartridge.chr_is_writable(),
            },
        }
    }

    pub fn debug_write_memory(&mut self, space: MemorySpace, offset: usize, value: u8) -> bool {
        match space {
            MemorySpace::CpuRam => self.bus.debug_write_cpu_ram(offset, value),
            MemorySpace::PpuNametable => self.bus.ppu.debug_write_nametable(offset, value),
            MemorySpace::Palette => self.bus.ppu.debug_write_palette(offset, value),
            MemorySpace::Oam => self.bus.ppu.debug_write_oam(offset, value),
            MemorySpace::PrgRom => false,
            MemorySpace::Chr => self.bus.cartridge.debug_write_chr(offset, value),
        }
    }
    pub fn region(&self) -> Region {
        self.bus.cartridge.region()
    }
    pub fn frame_rate(&self) -> f64 {
        self.region().frame_rate()
    }
    pub fn cpu_clock_hz(&self) -> u32 {
        self.region().cpu_clock_hz()
    }

    /// Copies the side-effect-free 64 KiB CPU address space used by
    /// RetroAchievements. Hardware registers read as zero; RAM, cartridge RAM,
    /// and currently mapped PRG ROM are exposed at their normal CPU addresses.
    pub fn copy_achievement_memory(&self, output: &mut [u8]) {
        self.bus.copy_achievement_memory(output);
    }

    /// Read the CPU address space without triggering hardware register side
    /// effects. Intended for test harnesses and external inspection tools.
    pub fn peek_cpu(&self, address: u16) -> u8 {
        self.bus.peek_cpu(address)
    }

    pub fn controller_reads(&self, port: usize) -> u64 {
        self.bus
            .controllers
            .get(port)
            .map_or(0, Controller::total_reads)
    }
}

fn native_output_palette(bytes: &[u8], mapper_id: u16) -> Option<crate::ppu::OutputPalette> {
    // Legacy iNES headers do not identify the RGB PPU revision. Match the
    // PRG+CHR payload so header repairs do not lose the correct hardware.
    const VS_SMB_PRG_CHR_SHA256: [u8; 32] = [
        0x5e, 0xb7, 0xf1, 0x85, 0x41, 0xc6, 0x1e, 0xb3, 0x94, 0x1b, 0x00, 0x43, 0x66, 0x03, 0xb5,
        0xaa, 0xad, 0x4c, 0x93, 0xa2, 0xb2, 0x99, 0x91, 0x8f, 0x88, 0x94, 0x96, 0x3d, 0x50, 0x62,
        0x71, 0xdc,
    ];
    if mapper_id != 99 || bytes.len() <= 16 {
        return None;
    }
    let content_start = 16 + usize::from(bytes.get(6).is_some_and(|flags| flags & 0x04 != 0)) * 512;
    let content_hash: [u8; 32] = Sha256::digest(bytes.get(content_start..)?).into();
    (content_hash == VS_SMB_PRG_CHR_SHA256).then_some(crate::ppu::RGB_2C04_0004_PALETTE)
}

fn hash_rom(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_rom(program: &[u8]) -> Vec<u8> {
        let mut rom = vec![0; 16 + 0x4000 + 0x2000];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 1;
        rom[5] = 1;
        rom[16..16 + program.len()].copy_from_slice(program);
        // Vectors in the mirrored end of the 16 KiB PRG bank.
        rom[16 + 0x3ffa..16 + 0x4000].copy_from_slice(&[0x00, 0x80, 0x00, 0x80, 0x00, 0x80]);
        rom
    }

    #[test]
    fn executes_a_small_nrom_program() {
        let rom = test_rom(&[0xa9, 0x40, 0xaa, 0xe8, 0x00]); // LDA #$40; TAX; INX; BRK
        let mut nes = Nes::from_ines(&rom).unwrap();
        nes.step_instruction().unwrap();
        nes.step_instruction().unwrap();
        nes.step_instruction().unwrap();
        let state = nes.cpu_state();
        assert_eq!(state.a, 0x40);
        assert_eq!(state.x, 0x41);
        assert_eq!(state.program_counter, 0x8004);
    }

    #[test]
    fn stable_unofficial_rmw_opcodes_update_memory_and_accumulator() {
        let cases = [
            (0x07, 0x01, 0x80, false, 0x01, 0x00), // SLO
            (0x27, 0xff, 0x80, true, 0x01, 0x01),  // RLA
            (0x47, 0x01, 0x03, false, 0x00, 0x01), // SRE
            (0x67, 0x00, 0x01, false, 0x01, 0x00), // RRA
            (0xc7, 0x04, 0x04, false, 0x04, 0x03), // DCP
            (0xe7, 0x05, 0x01, true, 0x03, 0x02),  // ISC
        ];
        for (opcode, accumulator, memory, carry, expected_a, expected_memory) in cases {
            let rom = test_rom(&[
                0xa9,
                accumulator,
                if carry { 0x38 } else { 0x18 },
                opcode,
                0x10,
            ]);
            let mut nes = Nes::from_ines(&rom).unwrap();
            assert!(nes.debug_write_memory(MemorySpace::CpuRam, 0x10, memory));
            nes.step_instruction().unwrap();
            nes.step_instruction().unwrap();
            nes.step_instruction().unwrap();
            assert_eq!(nes.cpu_state().a, expected_a, "opcode ${opcode:02X}");
            assert_eq!(nes.peek_cpu(0x10), expected_memory, "opcode ${opcode:02X}");
        }
    }

    #[test]
    fn stable_unofficial_load_store_and_immediate_opcodes_execute() {
        // LDA #$CC; LDX #$0F; SAX $10; LAX $10; ANC #$08; AXS #$03
        let rom = test_rom(&[
            0xa9, 0xcc, 0xa2, 0x0f, 0x87, 0x10, 0xa7, 0x10, 0x0b, 0x08, 0xcb, 0x03,
        ]);
        let mut nes = Nes::from_ines(&rom).unwrap();
        for _ in 0..6 {
            nes.step_instruction().unwrap();
        }
        assert_eq!(nes.peek_cpu(0x10), 0x0c);
        assert_eq!(nes.cpu_state().a, 0x08);
        assert_eq!(nes.cpu_state().x, 0x05);
    }

    #[test]
    fn unstable_unofficial_stores_execute_with_nmos_address_masking() {
        type StoreCase<'a> = (&'a [u8], Option<(u16, u8)>, u16, u8);
        let cases: &[StoreCase<'_>] = &[
            // AHX ($10),Y with a $02FE pointer.
            (
                &[0xa9, 0xff, 0xa2, 0x0f, 0xa0, 0x01, 0x93, 0x10],
                Some((0x10, 0xfe)),
                0x02ff,
                0x03,
            ),
            // AHX/TAS/SHX absolute,Y and SHY absolute,X.
            (
                &[0xa9, 0xff, 0xa2, 0x0f, 0xa0, 0x01, 0x9f, 0xfe, 0x02],
                None,
                0x02ff,
                0x03,
            ),
            (
                &[0xa9, 0xff, 0xa2, 0x0f, 0xa0, 0x01, 0x9b, 0xfe, 0x02],
                None,
                0x02ff,
                0x03,
            ),
            (
                &[0xa2, 0x01, 0xa0, 0x0f, 0x9c, 0xfe, 0x02],
                None,
                0x02ff,
                0x03,
            ),
            (
                &[0xa2, 0x0f, 0xa0, 0x01, 0x9e, 0xfe, 0x02],
                None,
                0x02ff,
                0x03,
            ),
            // A crossing SHY replaces the effective high byte with its value.
            (
                &[0xa2, 0x01, 0xa0, 0x01, 0x9c, 0xff, 0x02],
                None,
                0x0100,
                0x01,
            ),
        ];

        for (program, pointer, address, expected) in cases {
            let rom = test_rom(program);
            let mut nes = Nes::from_ines(&rom).unwrap();
            if let Some((offset, low)) = pointer {
                assert!(nes.debug_write_memory(MemorySpace::CpuRam, *offset as usize, *low));
                assert!(nes.debug_write_memory(MemorySpace::CpuRam, *offset as usize + 1, 0x02));
            }
            while nes.cpu_state().program_counter < 0x8000 + program.len() as u16 {
                nes.step_instruction().unwrap();
            }
            assert_eq!(nes.peek_cpu(*address), *expected, "program {program:02X?}");
        }
    }

    #[test]
    fn remaining_unofficial_immediates_execute_without_stopping() {
        // XAA #$F0; LAX #$0F; SEC; SBC #$01 through the alternate $EB encoding.
        let rom = test_rom(&[
            0xa9, 0xff, 0xa2, 0x3c, 0x8b, 0xf0, 0xab, 0x0f, 0x38, 0xeb, 0x01,
        ]);
        let mut nes = Nes::from_ines(&rom).unwrap();
        for _ in 0..6 {
            nes.step_instruction().unwrap();
        }
        let state = nes.cpu_state();
        assert_eq!(state.a, 0x0e);
        assert_eq!(state.x, 0x0f);
    }

    #[test]
    fn jam_opcode_holds_the_cpu_until_reset() {
        let rom = test_rom(&[0x02, 0xa9, 0x55]);
        let mut nes = Nes::from_ines(&rom).unwrap();
        assert_eq!(nes.step_instruction().unwrap(), 2);
        let jammed = nes.cpu_state();
        assert!(jammed.jammed);
        assert_eq!(jammed.program_counter, 0x8001);
        assert_eq!(nes.step_instruction().unwrap(), 1);
        assert_eq!(nes.cpu_state().program_counter, 0x8001);
        assert_eq!(nes.cpu_state().a, 0);
        nes.reset();
        assert!(!nes.cpu_state().jammed);
    }

    #[test]
    fn every_supported_opcode_completes_its_declared_bus_sequence() {
        for opcode in 0_u8..=u8::MAX {
            let rom = test_rom(&[opcode, 0x00, 0x00]);
            let mut nes = Nes::from_ines(&rom).unwrap();
            if let Ok(cycles) = nes.step_instruction() {
                assert!((1..=8).contains(&cycles), "opcode ${opcode:02X}");
            }
        }
    }

    #[test]
    fn jmp_indirect_reproduces_the_6502_page_wrap() {
        let rom = test_rom(&[0x6c, 0xff, 0x02]);
        let mut nes = Nes::from_ines(&rom).unwrap();
        assert!(nes.debug_write_memory(MemorySpace::CpuRam, 0x02ff, 0x34));
        assert!(nes.debug_write_memory(MemorySpace::CpuRam, 0x0200, 0x12));
        assert!(nes.debug_write_memory(MemorySpace::CpuRam, 0x0300, 0x56));
        nes.step_instruction().unwrap();
        assert_eq!(nes.cpu_state().program_counter, 0x1234);
    }

    #[test]
    fn both_controller_ports_are_independently_readable() {
        let rom = test_rom(&[
            0xa9, 0x01, 0x8d, 0x16, 0x40, 0xa9, 0x00, 0x8d, 0x16, 0x40, 0xad, 0x16, 0x40, 0x85,
            0x00, 0xad, 0x17, 0x40, 0x85, 0x01,
        ]);
        let mut nes = Nes::from_ines(&rom).unwrap();
        nes.controller_mut(1)
            .unwrap()
            .set_button(crate::Button::A, true);
        for _ in 0..8 {
            nes.step_instruction().unwrap();
        }
        assert_eq!(nes.peek_cpu(0) & 1, 0);
        assert_eq!(nes.peek_cpu(1) & 1, 1);
    }

    #[test]
    fn executes_a_multi_region_nes2_nrom_program_with_ntsc_timing() {
        let mut rom = test_rom(&[0xa9, 0x5a, 0x85, 0x00, 0x4c, 0x04, 0x80]);
        rom[7] = 0x08;
        rom[12] = 2;
        let mut nes = Nes::from_ines(&rom).unwrap();
        nes.step_instruction().unwrap();
        nes.step_instruction().unwrap();
        assert_eq!(nes.peek_cpu(0), 0x5a);
        assert_eq!(nes.cpu_cycles(), 12);
    }

    #[test]
    fn advances_to_a_complete_ntsc_frame() {
        // JMP $8000, an intentionally tiny forever loop.
        let rom = test_rom(&[0x4c, 0x00, 0x80]);
        let mut nes = Nes::from_ines(&rom).unwrap();
        nes.run_frame().unwrap();
        assert_eq!(nes.frame().number, 1);
        // Reset begins on the pre-render scanline, so the first completed picture
        // reaches VBlank slightly sooner than a steady-state 29,780-cycle frame.
        assert!(nes.cpu_cycles() > 27_000);
        assert_eq!(nes.frame().pixels.len(), 256 * 240 * 3);
    }

    #[test]
    fn save_state_round_trip_restores_the_machine() {
        let rom = test_rom(&[0x4c, 0x00, 0x80]);
        let mut nes = Nes::from_ines(&rom).unwrap();
        assert!(nes.debug_write_memory(MemorySpace::CpuRam, 0x123, 0x5a));
        nes.run_frame().unwrap();
        let cpu = nes.cpu_state();
        let ppu = nes.ppu_state();
        let cycles = nes.cpu_cycles();
        let frame_number = nes.frame().number;
        let pixels = nes.frame().pixels.clone();
        let state = nes.save_state().unwrap();

        assert!(nes.debug_write_memory(MemorySpace::CpuRam, 0x123, 0xa5));
        nes.run_frame().unwrap();
        nes.power_off();
        nes.load_state(&state).unwrap();

        assert_eq!(nes.cpu_state(), cpu);
        assert_eq!(nes.ppu_state(), ppu);
        assert_eq!(nes.cpu_cycles(), cycles);
        assert_eq!(nes.frame().number, frame_number);
        assert_eq!(nes.frame().pixels, pixels);
        assert!(nes.powered());
        assert_eq!(nes.memory_image(MemorySpace::CpuRam).bytes[0x123], 0x5a);
    }

    #[test]
    fn save_states_reject_wrong_roms_and_versions() {
        let rom = test_rom(&[0x4c, 0x00, 0x80]);
        let first = Nes::from_ines(&rom).unwrap();
        let state = first.save_state().unwrap();

        let mut other_rom = rom.clone();
        other_rom[16 + 0x4000] = 1;
        let mut other = Nes::from_ines(&other_rom).unwrap();
        assert!(matches!(
            other.load_state(&state),
            Err(StateError::WrongRom)
        ));

        let mut future_state = state;
        future_state[8..12].copy_from_slice(&(SAVE_STATE_VERSION + 1).to_le_bytes());
        let mut matching = Nes::from_ines(&rom).unwrap();
        assert!(matches!(
            matching.load_state(&future_state),
            Err(StateError::UnsupportedVersion(_))
        ));
    }

    #[test]
    fn output_palette_does_not_change_serialized_machine_state() {
        let rom = test_rom(&[0x4c, 0x00, 0x80]);
        let mut nes = Nes::from_ines(&rom).unwrap();
        nes.run_frame().unwrap();
        let default_pixels = nes.frame().pixels.clone();
        let default_state = nes.save_state().unwrap();

        nes.set_output_palette(crate::ppu::RGB_2C03_PALETTE);

        assert_ne!(nes.frame().pixels, default_pixels);
        assert_eq!(nes.save_state().unwrap(), default_state);
    }

    #[test]
    fn loading_a_snapshot_preserves_the_active_output_palette_exactly() {
        let rom = test_rom(&[0x4c, 0x00, 0x80]);
        let mut nes = Nes::from_ines(&rom).unwrap();
        nes.run_frame().unwrap();
        nes.set_output_palette(crate::ppu::RGB_2C04_0004_PALETTE);
        let expected_pixels = nes.frame().pixels.clone();
        let state = nes.save_state().unwrap();

        nes.run_frame().unwrap();
        nes.load_state(&state).unwrap();

        assert_eq!(nes.frame().pixels, expected_pixels);
    }

    #[test]
    fn snapshots_and_frame_inputs_replay_deterministically() {
        // Strobe controller 1, read its A button, store the result, then repeat.
        let program = [
            0xa9, 0x01, 0x8d, 0x16, 0x40, 0xa9, 0x00, 0x8d, 0x16, 0x40, 0xad, 0x16, 0x40, 0x8d,
            0x00, 0x00, 0x4c, 0x00, 0x80,
        ];
        let rom = test_rom(&program);
        let mut nes = Nes::from_ines(&rom).unwrap();
        let start = nes.save_state().unwrap();
        let inputs = [true, false, true, true, false];

        let run = |nes: &mut Nes| {
            for pressed in inputs {
                nes.controller_mut(0)
                    .unwrap()
                    .set_button(crate::Button::A, pressed);
                nes.run_frame().unwrap();
            }
            (
                nes.cpu_state(),
                nes.ppu_state(),
                nes.cpu_cycles(),
                nes.frame().pixels.clone(),
                nes.memory_image(MemorySpace::CpuRam).bytes,
            )
        };

        let first_result = run(&mut nes);
        nes.load_state(&start).unwrap();
        let second_result = run(&mut nes);
        assert_eq!(first_result, second_result);
    }

    #[test]
    fn debug_memory_access_respects_read_only_spaces_and_bounds() {
        let rom = test_rom(&[0x4c, 0x00, 0x80]);
        let mut nes = Nes::from_ines(&rom).unwrap();
        assert!(!nes.memory_image(MemorySpace::PrgRom).writable);
        assert!(!nes.debug_write_memory(MemorySpace::PrgRom, 0, 0xff));
        assert!(nes.debug_write_memory(MemorySpace::Palette, 0, 0xff));
        assert_eq!(nes.memory_image(MemorySpace::Palette).bytes[0], 0x3f);
        assert!(!nes.debug_write_memory(MemorySpace::CpuRam, 0x800, 0xff));
    }

    #[test]
    fn pal_rom_uses_50hz_cpu_and_ppu_timing() {
        let mut rom = test_rom(&[0x4c, 0x00, 0x80]);
        rom[9] = 1;
        let mut nes = Nes::from_ines(&rom).unwrap();
        assert_eq!(nes.region(), Region::Pal);
        assert_eq!(nes.cpu_clock_hz(), crate::PAL_CPU_CLOCK_HZ);
        assert_eq!(nes.frame_rate(), crate::PAL_FRAME_RATE);

        nes.run_frame().unwrap();
        let first_frame_cycles = nes.cpu_cycles();
        nes.run_frame().unwrap();
        let steady_frame_cycles = nes.cpu_cycles() - first_frame_cycles;
        assert!((33_247..=33_255).contains(&steady_frame_cycles));
    }

    #[test]
    fn explicit_region_overrides_an_incorrect_ines_timing_flag_without_changing_identity() {
        let rom = test_rom(&[0x4c, 0x00, 0x80]);
        let ordinary = Nes::from_ines(&rom).unwrap();
        let overridden = Nes::from_ines_with_region(&rom, Region::Pal).unwrap();

        assert_eq!(ordinary.region(), Region::Ntsc);
        assert_eq!(overridden.region(), Region::Pal);
        assert_eq!(ordinary.rom_hash(), overridden.rom_hash());
        assert_eq!(ordinary.rom_sha256(), overridden.rom_sha256());
    }

    #[test]
    fn reset_and_instructions_advance_exact_cpu_slots() {
        let rom = test_rom(&[0xea, 0x4c, 0x00, 0x80]);
        let mut nes = Nes::from_ines(&rom).unwrap();
        assert_eq!(nes.cpu_cycles(), 7);
        assert_eq!(nes.ppu_state().scanline, -1);
        assert_eq!(nes.ppu_state().dot, 21);

        assert_eq!(nes.step_instruction().unwrap(), 2);
        assert_eq!(nes.cpu_cycles(), 9);
        assert_eq!(nes.ppu_state().dot, 27);
    }

    #[test]
    fn oam_dma_stalls_before_the_next_cpu_bus_slot() {
        // LDA #$02; STA $4014; NOP
        let rom = test_rom(&[0xa9, 0x02, 0x8d, 0x14, 0x40, 0xea]);
        let mut nes = Nes::from_ines(&rom).unwrap();
        assert_eq!(nes.step_instruction().unwrap(), 2);
        assert_eq!(nes.step_instruction().unwrap(), 4);
        let before_nop = nes.cpu_cycles();

        assert_eq!(nes.step_instruction().unwrap(), 2);
        // The write ended on odd CPU cycle 13, selecting the 514-cycle DMA
        // stall. The following NOP still owns only its documented two slots.
        assert_eq!(nes.cpu_cycles() - before_nop, 514 + 2);
    }

    #[test]
    fn achievement_memory_is_side_effect_free_and_mirrors_system_ram() {
        let rom = test_rom(&[0xea, 0x4c, 0x00, 0x80]);
        let mut nes = Nes::from_ines(&rom).unwrap();
        assert!(nes.debug_write_memory(MemorySpace::CpuRam, 0x123, 0x5a));
        let mut memory = vec![0xff; 0x1_0000];
        nes.copy_achievement_memory(&mut memory);
        assert_eq!(memory[0x0123], 0x5a);
        assert_eq!(memory[0x0923], 0x5a);
        assert_eq!(memory[0x8000], 0xea);
        assert_eq!(memory[0x2002], 0);
    }
}
