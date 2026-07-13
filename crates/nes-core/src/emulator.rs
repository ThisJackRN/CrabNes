use std::{error::Error, fmt};

use crate::{
    bus::Bus,
    cartridge::{Cartridge, CartridgeError},
    controller::Controller,
    cpu::{Cpu, CpuError, CpuState},
    ppu::{Frame, PpuState},
};

#[derive(Debug)]
pub enum EmulationError {
    Cartridge(CartridgeError),
    Cpu(CpuError),
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
}

impl Nes {
    pub fn from_ines(bytes: &[u8]) -> Result<Self, EmulationError> {
        let cartridge = Cartridge::from_ines(bytes)?;
        let mut nes = Self {
            cpu: Cpu::default(),
            bus: Bus::new(cartridge),
            powered: true,
        };
        nes.cpu.reset(&mut nes.bus);
        nes.bus.clock_cpu_cycles(7);
        Ok(nes)
    }

    /// Execute one CPU instruction and advance the APU and PPU by matching clocks.
    pub fn step_instruction(&mut self) -> Result<u16, EmulationError> {
        if !self.powered {
            return Ok(0);
        }
        let cycles = if self.bus.ppu.take_nmi() {
            self.cpu.nmi(&mut self.bus)
        } else if self.bus.irq_pending() {
            let irq_cycles = self.cpu.irq(&mut self.bus);
            if irq_cycles == 0 {
                self.cpu.step(&mut self.bus)?
            } else {
                irq_cycles
            }
        } else {
            self.cpu.step(&mut self.bus)?
        };
        self.bus.clock_cpu_cycles(cycles);
        Ok(cycles)
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
        self.cpu.reset(&mut self.bus);
        self.bus.clock_cpu_cycles(7);
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
        self.bus.apu.state()
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
}
