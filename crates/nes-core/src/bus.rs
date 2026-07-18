use serde::{Deserialize, Serialize};

use crate::{
    apu::Apu,
    cartridge::{Cartridge, CartridgeSnapshot},
    cheat::Cheat,
    controller::Controller,
    fceux_state::{FceuxMmc3State, FceuxState, FceuxStateError},
    ppu::Ppu,
};

pub struct Bus {
    ram: [u8; 0x800],
    pub ppu: Ppu,
    pub apu: Apu,
    pub cartridge: Cartridge,
    pub controllers: [Controller; 2],
    oam_dma: Option<OamDma>,
    dmc_dma: Option<DmcDma>,
    cpu_cycles: u64,
    /// Data latch inside the 2A03. A $4015 read affects this latch without
    /// driving the cartridge-side CPU data bus.
    internal_data_bus: u8,
    /// Last value driven on the cartridge-side CPU data bus.
    open_bus: u8,
    dmc_completed_last_cpu_slot: bool,
    cpu_sequence_cycles: u16,
    /// Tracks contiguous reads of the same joypad port (index 0 = $4016,
    /// 1 = $4017); cleared by any other bus access. Only consulted when
    /// `fceux_joypad_compat` is enabled.
    joypad_oe: [bool; 2],
    /// Joypad clocking model. `false` (default) is NES-001 front-loader
    /// hardware: every CPU read slot on $4016/$4017 clocks the controller
    /// shift register, so DMC/OAM DMA cycles overlapping a joypad read cause
    /// the multi-clock corruption AccuracyCoin verifies (games like SMB3
    /// mitigate it by re-reading). `true` filters contiguous same-port reads
    /// down to one clock, matching the simplified model FCEUX movies were
    /// recorded against; the front end enables it only for such movies.
    fceux_joypad_compat: bool,
    nmi_samples: Vec<bool>,
    irq_samples: Vec<bool>,
    cheats: Vec<Cheat>,
    /// Runtime-only substitution counts parallel to `cheats`. Deliberately not
    /// part of snapshots: presentation statistics must never alter TAS hashes.
    cheat_hits: Vec<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct BusSnapshot {
    ram: Vec<u8>,
    ppu: Ppu,
    apu: Apu,
    cartridge: CartridgeSnapshot,
    controllers: [Controller; 2],
    oam_dma: Option<OamDma>,
    dmc_dma: Option<DmcDma>,
    cpu_cycles: u64,
    internal_data_bus: u8,
    open_bus: u8,
    dmc_completed_last_cpu_slot: bool,
}

impl Bus {
    pub fn new(cartridge: Cartridge) -> Self {
        let region = cartridge.region();
        Self {
            ram: [0; 0x800],
            // Power-on write lock is armed by Nes::from_cartridge / reset, not
            // here — unit tests construct bare buses mid-"run".
            ppu: Ppu::default(),
            apu: Apu::new(region),
            cartridge,
            controllers: [Controller::default(), Controller::default()],
            oam_dma: None,
            dmc_dma: None,
            cpu_cycles: 0,
            internal_data_bus: 0,
            open_bus: 0,
            dmc_completed_last_cpu_slot: false,
            cpu_sequence_cycles: 0,
            joypad_oe: [false; 2],
            fceux_joypad_compat: false,
            nmi_samples: Vec::with_capacity(8),
            irq_samples: Vec::with_capacity(8),
            cheats: Vec::new(),
            cheat_hits: Vec::new(),
        }
    }

    pub(crate) fn arm_ppu_reset_reg_lock(&mut self) {
        self.ppu.arm_reset_reg_lock();
    }

    pub fn reset(&mut self) {
        let region = self.cartridge.region();
        self.ppu.reset();
        self.apu.reset(region);
        self.cartridge.reset();
        self.oam_dma = None;
        self.dmc_dma = None;
        self.cpu_cycles = 0;
        self.internal_data_bus = 0;
        self.open_bus = 0;
        self.dmc_completed_last_cpu_slot = false;
        self.cpu_sequence_cycles = 0;
        self.joypad_oe = [false; 2];
        self.nmi_samples.clear();
        self.irq_samples.clear();
    }

    pub(crate) fn set_fceux_joypad_compat(&mut self, enabled: bool) {
        self.fceux_joypad_compat = enabled;
        self.joypad_oe = [false; 2];
    }

    pub(crate) fn fceux_joypad_compat(&self) -> bool {
        self.fceux_joypad_compat
    }

    /// Read a joypad port. Hardware (default) clocks the shift register on
    /// every read slot, including DMA-induced repeats of a halted read. The
    /// FCEUX-compat model instead folds back-to-back cycles on the same port
    /// into a single clock so DMA overlap cannot corrupt the pad stream.
    fn read_joypad(&mut self, port: usize) -> u8 {
        let contiguous = self.joypad_oe[port];
        self.joypad_oe = [false; 2];
        self.joypad_oe[port] = true;
        let clock = !(self.fceux_joypad_compat && contiguous);
        self.controllers[port].read(clock)
    }

    fn clear_joypad_oe(&mut self) {
        self.joypad_oe = [false; 2];
    }

    pub fn read(&mut self, address: u16) -> u8 {
        self.advance_cpu_slot(true, Some(address));
        self.record_irq_line();
        let value = self.read_untimed(address);
        self.record_nmi_line();
        value
    }

    pub(crate) fn import_fceux_state(&mut self, state: &FceuxState) -> Result<(), FceuxStateError> {
        let ram = state.required(1, b"RAM\0", 0x800)?;
        let data_bus = state.byte(1, b"DB\0\0")?;
        let nametable = state.required(3, b"NTAR", 0x800)?;
        let palette = state.required(3, b"PRAM", 0x20)?;
        let oam = state.required(3, b"SPRA", 0x100)?;
        let ppu_registers = state.required(3, b"PPUR", 4)?;
        let fine_x = state.byte(3, b"XOFF")?;
        let write_latch = state.byte(3, b"VTGL")? != 0;
        let vram_address = state.word(3, b"RADD")?;
        let temp_address = state.word(3, b"TADD")?;
        let read_buffer = state.byte(3, b"VBUF")?;
        let ppu_open_bus = state.byte(3, b"PGEN")?;
        let scanline = i32::from_le_bytes(state.required(31, b"PST0", 4)?.try_into().unwrap());
        let dot = i32::from_le_bytes(state.required(31, b"PST1", 4)?.try_into().unwrap());
        let odd_frame = state.byte(3, b"KOOK")? != 0;

        let joy_read_bits = state.required(4, b"JYRB", 2)?;
        let joy = state.required(4, b"JOYS", 4)?;
        let last_strobe = state.byte(4, b"LSTS")? != 0;

        let psg = state.required(5, b"PSG\0", 16)?;
        let enabled = state.byte(5, b"ENCH")?;
        let irq_frame_mode = state.byte(5, b"IQFM")?;
        let dmc_format = state.byte(5, b"5FMT")?;
        let dmc_output = state.byte(5, b"RWDA")?;
        let dmc_address = state.byte(5, b"5ADL")?;
        let dmc_length = state.byte(5, b"5SZL")?;

        let prg_ram = state.required(16, b"WRAM", 0x2000)?.to_vec();
        let banks: [u8; 8] = state.required(16, b"REGS", 8)?.try_into().unwrap();
        let irq_low = u32::from_le_bytes(state.required(2, b"IQLB", 4)?.try_into().unwrap());
        let mapper = FceuxMmc3State {
            bank_select: state.byte(16, b"CMD\0")?,
            banks,
            mirroring: state.byte(16, b"A000")?,
            ram_control: state.byte(16, b"A001")?,
            irq_reload: state.byte(16, b"IRQR")? != 0,
            irq_counter: state.byte(16, b"IRQC")?,
            irq_latch: state.byte(16, b"IRQL")?,
            irq_enabled: state.byte(16, b"IRQA")? != 0,
            irq_pending: irq_low & 0x001 != 0,
            prg_ram,
        };
        if !self.cartridge.import_fceux_mmc3(&mapper) {
            return Err(FceuxStateError::UnsupportedMapper(
                self.cartridge.mapper_id(),
            ));
        }

        self.ram.copy_from_slice(ram);
        self.ppu.import_fceux_state(
            nametable,
            palette,
            oam,
            ppu_registers,
            fine_x,
            write_latch,
            vram_address,
            temp_address,
            read_buffer,
            ppu_open_bus,
            scanline,
            dot,
            odd_frame,
        );
        let region = self.cartridge.region();
        self.apu.import_fceux_state(
            region,
            psg,
            enabled,
            dmc_format,
            dmc_output,
            dmc_address,
            dmc_length,
            irq_frame_mode,
        );
        for port in 0..2 {
            self.controllers[port].import_fceux_serial_state(
                joy[port],
                joy_read_bits[port],
                last_strobe,
            );
        }
        self.oam_dma = None;
        self.dmc_dma = None;
        self.cpu_cycles = 0;
        self.internal_data_bus = data_bus;
        self.open_bus = data_bus;
        self.dmc_completed_last_cpu_slot = false;
        self.cpu_sequence_cycles = 0;
        self.nmi_samples.clear();
        self.irq_samples.clear();
        Ok(())
    }

    fn read_untimed(&mut self, address: u16) -> u8 {
        let (actual, drives_external_bus) = if let Some(value) = self.cartridge.cpu_read(address) {
            self.clear_joypad_oe();
            (value, true)
        } else {
            match address {
                0x0000..=0x1fff => {
                    self.clear_joypad_oe();
                    (self.ram[address as usize & 0x07ff], true)
                }
                0x2000..=0x3fff => {
                    self.clear_joypad_oe();
                    (self.ppu.cpu_read(address & 7, &mut self.cartridge), true)
                }
                // $4015 is entirely internal to the 2A03. Bit 5 retains the
                // internal latch, and the read does not drive the external bus.
                0x4015 => {
                    self.clear_joypad_oe();
                    (
                        self.apu.read_status() | (self.internal_data_bus & 0x20),
                        false,
                    )
                }
                // The controller drives only the low serial bit. The upper
                // three bits retain CPU open bus on an NTSC NES.
                0x4016 if self.cartridge.mapper_id() == 99 => (
                    self.read_joypad(0) | (u8::from(self.controllers[0].coin()) * 0x20),
                    true,
                ),
                0x4017 if self.cartridge.mapper_id() == 99 => (self.read_joypad(1), true),
                0x4016 => (self.read_joypad(0) | (self.open_bus & 0xe0), true),
                0x4017 => (self.read_joypad(1) | (self.open_bus & 0xe0), true),
                _ => {
                    self.clear_joypad_oe();
                    (self.open_bus, false)
                }
            }
        };
        let value = self.apply_cheats_counted(address, actual);
        self.internal_data_bus = value;
        if drives_external_bus {
            self.open_bus = value;
        }
        value
    }

    pub fn write(&mut self, address: u16, value: u8) {
        self.advance_cpu_slot(false, None);
        self.record_irq_line();
        self.write_untimed(address, value);
        self.record_nmi_line();
    }

    fn write_untimed(&mut self, address: u16, value: u8) {
        self.internal_data_bus = value;
        self.open_bus = value;
        self.clear_joypad_oe();
        if self.cartridge.cpu_write(address, value) {
            return;
        }
        match address {
            0x0000..=0x1fff => self.ram[address as usize & 0x07ff] = value,
            0x2000..=0x3fff => self.ppu.cpu_write(address & 7, value, &mut self.cartridge),
            0x4000..=0x4013 | 0x4015 | 0x4017 => {
                self.apu.write(address, value);
                if address == 0x4015 && value & 0x10 == 0 && self.dmc_dma.is_some() {
                    // A disable write landing after the DMA request has
                    // reached the CPU is latched on the following APU phase.
                    self.apu.extend_dmc_disable_delay_for_active_dma();
                }
            }
            0x4014 => self.perform_oam_dma(value),
            0x4016 => {
                // The CPU drives the strobe level on every write, but the
                // controller latches an RMW pulse when it falls on the APU
                // get phase. This makes the one-cycle high pulse visible
                // only for the correct get/put ordering.
                let sample_latch =
                    self.cpu_sequence_cycles < 5 || (value & 1 == 0 && self.cpu_cycles & 1 != 0);
                self.controllers[0].write_strobe(value, sample_latch);
                self.controllers[1].write_strobe(value, sample_latch);
            }
            _ => {}
        }
    }

    pub(crate) fn begin_cpu_sequence(&mut self) {
        debug_assert_eq!(self.cpu_sequence_cycles, 0);
        self.cpu_sequence_cycles = 0;
        self.nmi_samples.clear();
        self.irq_samples.clear();
    }

    /// Completes the idle slots in a CPU instruction, reset, or interrupt
    /// sequence. Memory accesses have already advanced their individual slots.
    pub(crate) fn finish_cpu_sequence(&mut self, target_cycles: u16) -> u16 {
        while self.cpu_sequence_cycles < target_cycles {
            self.advance_cpu_slot(true, None);
            self.record_irq_line();
            self.record_nmi_line();
        }
        let actual = self.cpu_sequence_cycles;
        debug_assert_eq!(actual, target_cycles);
        self.cpu_sequence_cycles = 0;
        actual
    }

    pub(crate) fn cancel_cpu_sequence(&mut self) {
        self.cpu_sequence_cycles = 0;
        self.nmi_samples.clear();
        self.irq_samples.clear();
    }

    pub(crate) fn nmi_pending_at_slot(&self, slot: u16) -> bool {
        slot.checked_sub(1)
            .and_then(|slot| self.nmi_samples.get(slot as usize))
            .copied()
            .unwrap_or(false)
    }

    pub(crate) fn irq_pending_at_slot(&self, slot: u16) -> bool {
        slot.checked_sub(1)
            .and_then(|slot| self.irq_samples.get(slot as usize))
            .copied()
            .unwrap_or(false)
    }

    fn record_irq_line(&mut self) {
        self.irq_samples.push(self.irq_pending());
    }

    fn record_nmi_line(&mut self) {
        self.nmi_samples.push(self.ppu.nmi_pending());
    }

    fn advance_cpu_slot(&mut self, can_service_dmc: bool, halted_address: Option<u16>) -> bool {
        let dmc_completed = self.service_dma_stalls(can_service_dmc, halted_address);
        self.clock_hardware_cycle();
        self.cpu_sequence_cycles = self.cpu_sequence_cycles.saturating_add(1);
        self.dmc_completed_last_cpu_slot = dmc_completed;
        dmc_completed
    }

    fn service_dma_stalls(&mut self, can_service_dmc: bool, halted_address: Option<u16>) -> bool {
        let mut dmc_completed = false;
        if self.oam_dma.is_none() && self.dmc_dma.is_none() {
            return false;
        }
        // RDY can only halt the 6502 during a read. DMA requests remain
        // pending through all consecutive CPU write slots.
        if !can_service_dmc {
            return false;
        }

        // NTSC: while halted on $4016/$4017, dummy/align DMA cycles must not
        // issue extra external re-reads. The halt cycle *does* re-read (and
        // clocks the pad); the resumed CPU access clocks again — hardware's
        // DMA read corruption. Under the FCEUX-compat model, read_joypad folds
        // those contiguous same-port reads into a single shift edge instead.
        let skip_dummy_reads = self.cartridge.region() == crate::Region::Ntsc
            && matches!(halted_address, Some(0x4016 | 0x4017));

        // OAM and DMC share the first halt cycle when both are pending.
        let halt_pending = self.oam_dma.as_ref().is_some_and(|dma| dma.halt_pending)
            || self.dmc_dma.as_ref().is_some_and(|dma| dma.need_halt);
        if halt_pending {
            if let Some(dma) = self.oam_dma.as_mut() {
                dma.halt_pending = false;
            }
            if let Some(dma) = self.dmc_dma.as_mut() {
                dma.need_halt = false;
            }
            self.clock_hardware_cycle();
            // Halt cycle always re-issues the CPU read (visible on joypad /OE).
            if let Some(address) = halted_address {
                self.read_untimed(address);
            }
            if self.dmc_dma.as_ref().is_some_and(|dma| dma.abort) {
                self.cancel_dmc_dma();
                if self.oam_dma.is_none() {
                    return false;
                }
            }
        }

        while self.oam_dma.is_some() || self.dmc_dma.is_some() {
            if self.dmc_dma.as_ref().is_some_and(|dma| dma.abort) {
                self.cancel_dmc_dma();
                if self.oam_dma.is_none() {
                    break;
                }
            }

            // The DMA get/put cadence is tied to the APU clock. A DMC get has
            // priority over an OAM get; all setup/no-op work can overlap OAM.
            let get_cycle = self.cpu_cycles & 1 != 0;
            let dmc_ready = self
                .dmc_dma
                .as_ref()
                .is_some_and(|dma| !dma.need_halt && !dma.need_dummy);

            if get_cycle && dmc_ready {
                let address = self.dmc_dma.unwrap().address;
                self.clock_hardware_cycle();
                // DMA address is on the external bus, so joypad /OE drops.
                self.clear_joypad_oe();
                let actual = self.cartridge.cpu_read(address).unwrap_or(self.open_bus);
                let value = self.apply_cheats_counted(address, actual);
                self.open_bus = value;
                if self.cartridge.region() == crate::Region::Ntsc
                    && let Some(cpu_address @ 0x4000..=0x401f) = halted_address
                {
                    let conflict_address = (cpu_address & 0xffe0) | (address & 0x001f);
                    self.read_untimed(conflict_address);
                }
                self.dmc_dma = None;
                self.apu.supply_dmc_sample(value);
                if let Some(next_address) = self.apu.take_dmc_dma_request() {
                    self.dmc_dma = Some(DmcDma {
                        address: next_address,
                        need_halt: true,
                        need_dummy: true,
                        abort: false,
                    });
                }
                dmc_completed = true;
            } else if get_cycle && self.oam_dma.is_some() {
                self.advance_dmc_setup();
                let (page, index) = {
                    let dma = self.oam_dma.as_ref().unwrap();
                    (dma.page, dma.index)
                };
                self.clock_hardware_cycle();
                let value =
                    self.read_oam_dma_untimed((u16::from(page) << 8) | index, halted_address);
                if let Some(dma) = self.oam_dma.as_mut() {
                    dma.latch = value;
                    dma.read_pending = true;
                }
            } else if !get_cycle && self.oam_dma.as_ref().is_some_and(|dma| dma.read_pending) {
                self.advance_dmc_setup();
                let value = self.oam_dma.as_ref().unwrap().latch;
                self.clock_hardware_cycle();
                self.ppu.cpu_write(4, value, &mut self.cartridge);
                let dma = self.oam_dma.as_mut().unwrap();
                dma.read_pending = false;
                dma.index += 1;
                if dma.index == 0x100 {
                    self.oam_dma = None;
                }
            } else {
                self.advance_dmc_setup();
                self.clock_hardware_cycle();
                // Dummy / alignment: re-read unless NTSC joypad (contiguous /OE
                // already held from halt; extra external clocks are filtered).
                if !skip_dummy_reads && let Some(address) = halted_address {
                    self.read_untimed(address);
                }
            }
        }
        dmc_completed
    }

    /// The OAM DMA address bus alone does not activate the CPU's internal
    /// APU/I/O register decode.  The CPU address bus must be in the
    /// $4000-$401F window; then its low-five-bit register select can conflict
    /// with the OAM source data.
    fn read_oam_dma_untimed(&mut self, address: u16, cpu_address: Option<u16>) -> u8 {
        let Some(cpu_address @ 0x4000..=0x401f) = cpu_address else {
            return if (0x4000..=0x401f).contains(&address) {
                self.internal_data_bus
            } else {
                self.read_untimed(address)
            };
        };

        if (0x4000..=0x401f).contains(&address) || matches!(address & 0x001f, 0x0015..=0x0017) {
            let active_register = (cpu_address & 0xffe0) | (address & 0x001f);
            if matches!(address & 0x001f, 0x0016 | 0x0017) {
                let port = (address & 1) as usize;
                // The controller shift clock still sees this read, but a
                // mapped OAM source wins the data-bus conflict. That makes
                // controller bits visible for open-bus source pages while
                // preserving the source byte for RAM pages.
                // OAM DMA conflict with joypad: treat as a fresh /OE edge so
                // the shift clock still fires (side-effect on the joypad pin).
                self.joypad_oe[port] = false;
                let controller_bit = self.read_joypad(port);
                if address < 0x2000 {
                    return self.read_untimed(address);
                }
                let value = controller_bit | (self.internal_data_bus & 0xe0);
                self.internal_data_bus = value;
                self.open_bus = value;
                value
            } else {
                self.read_untimed(active_register)
            }
        } else {
            self.read_untimed(address)
        }
    }

    fn advance_dmc_setup(&mut self) {
        if let Some(dma) = self.dmc_dma.as_mut() {
            if dma.need_halt {
                dma.need_halt = false;
            } else if dma.need_dummy {
                dma.need_dummy = false;
            }
        }
    }

    fn cancel_dmc_dma(&mut self) {
        self.dmc_dma = None;
        self.apu.cancel_dmc_dma();
    }

    /// Complete an unstable NMOS store. A DMC DMA immediately before the
    /// write replaces the internal high-byte mask, turning these opcodes into
    /// their unmasked store form for that cycle.
    pub(crate) fn write_unstable_store(
        &mut self,
        masked_address: u16,
        masked_value: u8,
        unmasked_address: u16,
        unmasked_value: u8,
    ) {
        let dmc_interrupted_previous_slot = self.dmc_completed_last_cpu_slot;
        let dmc_completed = self.advance_cpu_slot(false, None);
        self.record_irq_line();
        if dmc_interrupted_previous_slot || dmc_completed {
            self.write_untimed(unmasked_address, unmasked_value);
        } else {
            self.write_untimed(masked_address, masked_value);
        }
        self.record_nmi_line();
    }

    fn clock_hardware_cycle(&mut self) {
        let region = self.cartridge.region();
        self.cartridge.clock_cpu();
        self.apu
            .clock_with_expansion(self.cartridge.expansion_audio(), region);
        let ppu_dots = match region {
            crate::Region::Ntsc => 3,
            // The PAL PPU runs at 16/5 of the CPU rate: 3,3,3,3,4 dots.
            crate::Region::Pal => 3 + u8::from(self.cpu_cycles % 5 == 4),
        };
        for _ in 0..ppu_dots {
            self.ppu.clock_for_region(&mut self.cartridge, region);
        }
        self.cpu_cycles = self.cpu_cycles.wrapping_add(1);

        if self.apu.take_dmc_dma_abort() {
            if self.dmc_dma.as_ref().is_some_and(|dma| dma.need_halt) {
                self.cancel_dmc_dma();
            } else if let Some(dma) = self.dmc_dma.as_mut() {
                dma.abort = true;
            }
        }

        if self.dmc_dma.is_none()
            && let Some(address) = self.apu.take_dmc_dma_request()
        {
            // The request halts the CPU before its next bus slot. More exact
            // read/write collision behavior can now be expressed here without
            // changing the CPU instruction decoder.
            self.dmc_dma = Some(DmcDma {
                address,
                need_halt: true,
                need_dummy: true,
                abort: false,
            });
        }
    }

    pub fn irq_pending(&self) -> bool {
        self.apu.irq_pending() || self.cartridge.irq_pending()
    }

    pub fn cpu_cycles(&self) -> u64 {
        self.cpu_cycles
    }

    pub(crate) fn set_cheats(&mut self, cheats: Vec<Cheat>) {
        self.cheat_hits = vec![0; cheats.len()];
        self.cheats = cheats;
    }

    pub(crate) fn cheats_with_hits(&self) -> impl Iterator<Item = (Cheat, u64)> + '_ {
        self.cheats
            .iter()
            .copied()
            .zip(self.cheat_hits.iter().copied())
    }

    fn apply_cheats(&self, address: u16, actual: u8) -> u8 {
        self.cheats
            .iter()
            .find_map(|cheat| cheat.replacement(address, actual))
            .unwrap_or(actual)
    }

    /// Cheat substitution on the emulated CPU's own read paths. Unlike the
    /// side-effect-free peek used by inspection tools, real reads count hits
    /// so the front end can show where a code is firing.
    fn apply_cheats_counted(&mut self, address: u16, actual: u8) -> u8 {
        for (cheat, hits) in self.cheats.iter().zip(&mut self.cheat_hits) {
            if let Some(value) = cheat.replacement(address, actual) {
                *hits = hits.wrapping_add(1);
                return value;
            }
        }
        actual
    }

    pub(crate) fn snapshot(&self) -> BusSnapshot {
        let mut ppu = self.ppu.clone();
        // Frame RGB bytes are part of the legacy snapshot layout. Normalize
        // them so a visual palette preference cannot change TAS state hashes.
        ppu.canonicalize_output_for_snapshot();
        BusSnapshot {
            ram: self.ram.to_vec(),
            ppu,
            apu: self.apu.clone(),
            cartridge: self.cartridge.snapshot(),
            controllers: self.controllers.clone(),
            oam_dma: self.oam_dma,
            dmc_dma: self.dmc_dma,
            cpu_cycles: self.cpu_cycles,
            internal_data_bus: self.internal_data_bus,
            open_bus: self.open_bus,
            dmc_completed_last_cpu_slot: self.dmc_completed_last_cpu_slot,
        }
    }

    pub(crate) fn restore_snapshot(&mut self, snapshot: BusSnapshot) -> bool {
        if snapshot.ram.len() != self.ram.len()
            || !self.cartridge.restore_snapshot(&snapshot.cartridge)
        {
            return false;
        }
        self.ram.copy_from_slice(&snapshot.ram);
        let output_palette = self.ppu.output_palette();
        self.ppu = snapshot.ppu;
        // Output colors are a front-end preference, not machine state. Keep
        // the active palette when loading save states, rewind, or TAS points.
        // Serialized snapshots are already normalized to the default palette.
        // Avoid remapping every framebuffer pixel when that is also the active
        // palette; this is the hot path during continuous rewind.
        if self.ppu.output_palette() != output_palette {
            self.ppu.set_output_palette(output_palette);
        } else {
            self.ppu.prepare_default_output_after_snapshot_restore();
        }
        self.apu = snapshot.apu;
        self.apu.clear_samples();
        self.controllers = snapshot.controllers;
        self.oam_dma = snapshot.oam_dma;
        self.dmc_dma = snapshot.dmc_dma;
        self.cpu_cycles = snapshot.cpu_cycles;
        self.internal_data_bus = snapshot.internal_data_bus;
        self.open_bus = snapshot.open_bus;
        self.dmc_completed_last_cpu_slot = snapshot.dmc_completed_last_cpu_slot;
        self.cpu_sequence_cycles = 0;
        true
    }

    pub(crate) fn cpu_ram(&self) -> &[u8] {
        &self.ram
    }

    pub(crate) fn copy_achievement_memory(&self, output: &mut [u8]) {
        for (address, byte) in output.iter_mut().take(0x1_0000).enumerate() {
            *byte = self.peek_cpu(address as u16);
        }
    }

    pub(crate) fn peek_cpu(&self, address: u16) -> u8 {
        self.apply_cheats(address, self.peek_cpu_actual(address))
    }

    /// The byte the console would see with no cheat device attached.
    pub(crate) fn peek_cpu_actual(&self, address: u16) -> u8 {
        self.cartridge
            .cpu_peek(address)
            .unwrap_or_else(|| match address {
                0x0000..=0x1fff => self.ram[address as usize & 0x07ff],
                // Avoid side effects from PPU, APU, and controller reads.
                _ => 0,
            })
    }

    pub(crate) fn debug_write_cpu_ram(&mut self, offset: usize, value: u8) -> bool {
        self.ram.get_mut(offset).is_some_and(|byte| {
            *byte = value;
            true
        })
    }

    fn perform_oam_dma(&mut self, page: u8) {
        self.oam_dma = Some(OamDma {
            page,
            index: 0,
            latch: 0,
            halt_pending: true,
            read_pending: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nrom_cartridge() -> Cartridge {
        let mut rom = vec![0; 16 + 0x4000 + 0x2000];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 1;
        rom[5] = 1;
        Cartridge::from_ines(&rom).unwrap()
    }

    fn mapper99_cartridge() -> Cartridge {
        let mut rom = vec![0; 16 + 0x8000 + 0x4000];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 2;
        rom[5] = 2;
        rom[6] = 0x30;
        rom[7] = 0x60;
        Cartridge::from_ines(&rom).unwrap()
    }

    #[test]
    fn controller_reads_preserve_upper_cpu_open_bus_bits() {
        let mut bus = Bus::new(nrom_cartridge());
        bus.open_bus = 0xa0;
        bus.internal_data_bus = 0xa0;
        assert_eq!(bus.read_untimed(0x4016), 0xa0);
        bus.controllers[0].set_button(crate::controller::Button::A, true);
        bus.controllers[0].write_strobe(1, true);
        bus.open_bus = 0xe0;
        bus.internal_data_bus = 0xe0;
        assert_eq!(bus.read_untimed(0x4016), 0xe1);
    }

    #[test]
    fn raw_cheats_replace_cpu_ram_reads_and_honor_compare_values() {
        let mut bus = Bus::new(nrom_cartridge());
        bus.ram[0x20] = 0x12;
        bus.set_cheats(vec![Cheat::new(0x0020, 0x34, Some(0x12))]);
        assert_eq!(bus.read_untimed(0x0020), 0x34);
        assert_eq!(bus.ram[0x20], 0x12, "a read patch does not alter RAM");

        bus.set_cheats(vec![Cheat::new(0x0020, 0x56, Some(0xff))]);
        assert_eq!(bus.read_untimed(0x0020), 0x12);
    }

    #[test]
    fn mapper99_coin_input_is_separate_from_the_select_button() {
        let mut bus = Bus::new(mapper99_cartridge());
        bus.open_bus = 0xe0;
        assert_eq!(bus.read_untimed(0x4016), 0x00);

        bus.controllers[0].set_coin(true);
        assert_eq!(bus.read_untimed(0x4016) & 0x60, 0x20);
        assert_eq!(bus.read_untimed(0x4017) & 0xfc, 0x00);

        bus.controllers[0].set_coin(false);
        bus.controllers[0].set_button(crate::controller::Button::Select, true);
        assert_eq!(bus.read_untimed(0x4016) & 0x60, 0x00);
    }

    #[test]
    fn apu_status_updates_only_the_internal_data_bus() {
        let mut bus = Bus::new(nrom_cartridge());
        bus.open_bus = 0x40;
        bus.internal_data_bus = 0x20;

        assert_eq!(bus.read_untimed(0x4015), 0x20);
        assert_eq!(bus.open_bus, 0x40);
        assert_eq!(bus.internal_data_bus, 0x20);
        assert_eq!(bus.read_untimed(0x4115), 0x40);
    }

    #[test]
    fn dmc_interrupted_unstable_store_uses_the_unmasked_value_and_address() {
        let mut bus = Bus::new(nrom_cartridge());
        bus.dmc_dma = Some(DmcDma {
            address: 0x8000,
            need_halt: true,
            need_dummy: true,
            abort: false,
        });

        bus.write_unstable_store(0x0001, 0x11, 0x0002, 0x22);
        assert_eq!(bus.ram[1], 0x11);
        assert_eq!(bus.ram[2], 0);
        assert!(
            bus.dmc_dma.is_some(),
            "DMC cannot halt the final write slot"
        );

        let mut bus = Bus::new(nrom_cartridge());
        bus.dmc_dma = Some(DmcDma {
            address: 0x8000,
            need_halt: true,
            need_dummy: true,
            abort: false,
        });
        bus.read(0x0000);
        bus.write_unstable_store(0x0001, 0x11, 0x0002, 0x22);
        assert_eq!(bus.ram[1], 0);
        assert_eq!(bus.ram[2], 0x22);
    }

    #[test]
    fn dmc_dma_waits_through_cpu_writes() {
        let mut bus = Bus::new(nrom_cartridge());
        bus.dmc_dma = Some(DmcDma {
            address: 0x8000,
            need_halt: true,
            need_dummy: true,
            abort: false,
        });

        bus.write(0x0000, 0x5a);
        assert_eq!(bus.ram[0], 0x5a);
        assert!(bus.dmc_dma.is_some());

        bus.read(0x0000);
        assert!(bus.dmc_dma.is_none());
    }

    #[test]
    fn dmc_dma_collision_clocks_ntsc_controller_before_retried_read() {
        // Halt re-reads $4016 (clocks A off), dummy/align filtered, DMA get
        // drops /OE, resume clocks again — returned bit is B (released = 0).
        let mut bus = Bus::new(nrom_cartridge());
        bus.controllers[0].set_button(crate::controller::Button::A, true);
        bus.controllers[0].write_strobe(1, true);
        bus.controllers[0].write_strobe(0, true);
        bus.dmc_dma = Some(DmcDma {
            address: 0xc000,
            need_halt: true,
            need_dummy: true,
            abort: false,
        });

        assert_eq!(bus.read(0x4016) & 1, 0, "the retried read sees B, not A");
        // Halt + resume both access $4016 (dummy/align skipped for NTSC joypad).
        assert_eq!(bus.controllers[0].total_reads(), 2);
    }

    #[test]
    fn fceux_compat_folds_back_to_back_joypad_reads_into_one_clock() {
        let mut bus = Bus::new(nrom_cartridge());
        bus.set_fceux_joypad_compat(true);
        bus.controllers[0].set_button(crate::controller::Button::A, true);
        bus.controllers[0].set_button(crate::controller::Button::B, true);
        bus.controllers[0].write_strobe(1, true);
        bus.controllers[0].write_strobe(0, true);

        // Two untimed reads in the same "contiguous" sense need the OE flag.
        let a = bus.read_untimed(0x4016) & 1;
        let b = bus.read_untimed(0x4016) & 1;
        assert_eq!(a, 1, "first contiguous read clocks A");
        assert_eq!(b, 1, "held contiguous set does not shift — still A");
        // Break contiguity with an unrelated access; the next poll clocks B.
        let _ = bus.read_untimed(0x0000);
        assert_eq!(bus.read_untimed(0x4016) & 1, 1, "fresh read clocks B");
    }

    #[test]
    fn hardware_default_clocks_every_joypad_read_slot() {
        // The NES-001 model AccuracyCoin verifies: no contiguity filtering,
        // so back-to-back reads (as DMA overlap produces) shift every time.
        let mut bus = Bus::new(nrom_cartridge());
        bus.controllers[0].set_button(crate::controller::Button::A, true);
        bus.controllers[0].write_strobe(1, true);
        bus.controllers[0].write_strobe(0, true);

        assert_eq!(bus.read_untimed(0x4016) & 1, 1, "first read clocks A");
        assert_eq!(
            bus.read_untimed(0x4016) & 1,
            0,
            "second contiguous read still shifts to B (released)"
        );
    }

    #[test]
    fn dmc_no_op_cycles_repeat_a_read_sensitive_ppu_access() {
        let mut bus = Bus::new(nrom_cartridge());
        bus.write_untimed(0x2006, 0x20);
        bus.write_untimed(0x2006, 0x00);
        bus.dmc_dma = Some(DmcDma {
            address: 0xc000,
            need_halt: true,
            need_dummy: true,
            abort: false,
        });

        bus.read(0x2007);

        assert_eq!(bus.ppu.state().vram_address, 0x2004);
    }

    #[test]
    fn dmc_fetch_low_address_bits_can_activate_a_controller_register() {
        let mut rom = vec![0; 16 + 0x4000 + 0x2000];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 1;
        rom[5] = 1;
        rom[16 + 0x16] = 0xe0;
        let mut bus = Bus::new(Cartridge::from_ines(&rom).unwrap());
        bus.controllers[0].set_button(crate::controller::Button::A, true);
        bus.controllers[0].write_strobe(1, true);
        bus.controllers[0].write_strobe(0, true);
        bus.dmc_dma = Some(DmcDma {
            address: 0xc016,
            need_halt: false,
            need_dummy: false,
            abort: false,
        });

        assert_eq!(bus.read(0x4000), 0xe1);
        assert_eq!(bus.controllers[0].total_reads(), 1);
    }

    #[test]
    fn active_apu_oam_dma_uses_internal_latch_or_a_mapped_source() {
        let mut bus = Bus::new(nrom_cartridge());
        bus.controllers[0].set_button(crate::controller::Button::A, true);
        bus.controllers[0].write_strobe(1, true);
        bus.controllers[0].write_strobe(0, true);
        bus.internal_data_bus = 0x40;
        bus.open_bus = 0x50;

        assert_eq!(bus.read_oam_dma_untimed(0x4036, Some(0x4001)), 0x41);
        assert_eq!(bus.open_bus, 0x41);

        bus.ram[0x216] = 0xff;
        assert_eq!(bus.read_oam_dma_untimed(0x0216, Some(0x4001)), 0xff);
        assert_eq!(bus.controllers[0].total_reads(), 2);
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct DmcDma {
    address: u16,
    need_halt: bool,
    need_dummy: bool,
    abort: bool,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct OamDma {
    page: u8,
    index: u16,
    latch: u8,
    halt_pending: bool,
    read_pending: bool,
}
