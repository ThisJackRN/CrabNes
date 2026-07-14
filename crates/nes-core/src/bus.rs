use serde::{Deserialize, Serialize};

use crate::{
    apu::Apu,
    cartridge::{Cartridge, CartridgeSnapshot},
    controller::Controller,
    ppu::Ppu,
};

pub struct Bus {
    ram: [u8; 0x800],
    pub ppu: Ppu,
    pub apu: Apu,
    pub cartridge: Cartridge,
    pub controllers: [Controller; 2],
    dma_stall: u16,
    dmc_dma: Option<DmcDma>,
    cpu_cycles: u64,
    open_bus: u8,
    cpu_sequence_cycles: u16,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct BusSnapshot {
    ram: Vec<u8>,
    ppu: Ppu,
    apu: Apu,
    cartridge: CartridgeSnapshot,
    controllers: [Controller; 2],
    dma_stall: u16,
    dmc_dma: Option<DmcDma>,
    cpu_cycles: u64,
    open_bus: u8,
}

impl Bus {
    pub fn new(cartridge: Cartridge) -> Self {
        let region = cartridge.region();
        Self {
            ram: [0; 0x800],
            ppu: Ppu::default(),
            apu: Apu::new(region),
            cartridge,
            controllers: [Controller::default(), Controller::default()],
            dma_stall: 0,
            dmc_dma: None,
            cpu_cycles: 0,
            open_bus: 0,
            cpu_sequence_cycles: 0,
        }
    }

    pub fn reset(&mut self) {
        let region = self.cartridge.region();
        self.ppu.reset();
        self.apu.reset(region);
        self.cartridge.reset();
        self.dma_stall = 0;
        self.dmc_dma = None;
        self.cpu_cycles = 0;
        self.open_bus = 0;
        self.cpu_sequence_cycles = 0;
    }

    pub fn read(&mut self, address: u16) -> u8 {
        self.advance_cpu_slot();
        self.read_untimed(address)
    }

    fn read_untimed(&mut self, address: u16) -> u8 {
        let value = if let Some(value) = self.cartridge.cpu_read(address) {
            value
        } else {
            match address {
                0x0000..=0x1fff => self.ram[address as usize & 0x07ff],
                0x2000..=0x3fff => self.ppu.cpu_read(address & 7, &mut self.cartridge),
                // Bit 5 is not driven by the APU and retains CPU open bus.
                0x4015 => self.apu.read_status() | (self.open_bus & 0x20),
                0x4016 => self.controllers[0].read(),
                0x4017 => self.controllers[1].read(),
                _ => self.open_bus,
            }
        };
        self.open_bus = value;
        value
    }

    pub fn write(&mut self, address: u16, value: u8) {
        self.advance_cpu_slot();
        self.write_untimed(address, value);
    }

    fn write_untimed(&mut self, address: u16, value: u8) {
        self.open_bus = value;
        if self.cartridge.cpu_write(address, value) {
            return;
        }
        match address {
            0x0000..=0x1fff => self.ram[address as usize & 0x07ff] = value,
            0x2000..=0x3fff => self.ppu.cpu_write(address & 7, value, &mut self.cartridge),
            0x4000..=0x4013 | 0x4015 | 0x4017 => self.apu.write(address, value),
            0x4014 => self.perform_oam_dma(value),
            0x4016 => {
                self.controllers[0].write_strobe(value);
                self.controllers[1].write_strobe(value);
            }
            _ => {}
        }
    }

    pub(crate) fn begin_cpu_sequence(&mut self) {
        debug_assert_eq!(self.cpu_sequence_cycles, 0);
        self.cpu_sequence_cycles = 0;
    }

    /// Completes the idle slots in a CPU instruction, reset, or interrupt
    /// sequence. Memory accesses have already advanced their individual slots.
    pub(crate) fn finish_cpu_sequence(&mut self, target_cycles: u16) -> u16 {
        while self.cpu_sequence_cycles < target_cycles {
            self.advance_cpu_slot();
        }
        let actual = self.cpu_sequence_cycles;
        debug_assert_eq!(actual, target_cycles);
        self.cpu_sequence_cycles = 0;
        actual
    }

    pub(crate) fn cancel_cpu_sequence(&mut self) {
        self.cpu_sequence_cycles = 0;
    }

    fn advance_cpu_slot(&mut self) {
        self.service_dma_stalls();
        self.clock_hardware_cycle();
        self.cpu_sequence_cycles = self.cpu_sequence_cycles.saturating_add(1);
    }

    fn service_dma_stalls(&mut self) {
        while self.dma_stall != 0 || self.dmc_dma.is_some() {
            let servicing_dmc = self.dmc_dma.is_some();
            self.clock_hardware_cycle();

            if servicing_dmc {
                let completed = self.dmc_dma.as_mut().and_then(|dma| {
                    dma.cycles_remaining -= 1;
                    (dma.cycles_remaining == 0).then_some(dma.address)
                });
                if let Some(address) = completed {
                    self.dmc_dma = None;
                    let value = self.cartridge.cpu_read(address).unwrap_or(self.open_bus);
                    self.open_bus = value;
                    self.apu.supply_dmc_sample(value);
                }
            } else {
                self.dma_stall -= 1;
            }
        }
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

        if self.dmc_dma.is_none()
            && let Some(address) = self.apu.take_dmc_dma_request()
        {
            // The request halts the CPU before its next bus slot. More exact
            // read/write collision behavior can now be expressed here without
            // changing the CPU instruction decoder.
            self.dmc_dma = Some(DmcDma {
                address,
                cycles_remaining: 4,
            });
        }
    }

    pub fn irq_pending(&self) -> bool {
        self.apu.irq_pending() || self.cartridge.irq_pending()
    }

    pub fn cpu_cycles(&self) -> u64 {
        self.cpu_cycles
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
            dma_stall: self.dma_stall,
            dmc_dma: self.dmc_dma,
            cpu_cycles: self.cpu_cycles,
            open_bus: self.open_bus,
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
        self.dma_stall = snapshot.dma_stall;
        self.dmc_dma = snapshot.dmc_dma;
        self.cpu_cycles = snapshot.cpu_cycles;
        self.open_bus = snapshot.open_bus;
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
        let mut data = [0; 256];
        let base = (page as u16) << 8;
        for (offset, slot) in data.iter_mut().enumerate() {
            *slot = self.read_untimed(base.wrapping_add(offset as u16));
        }
        self.ppu.write_oam_dma(&data);
        self.dma_stall = 513 + (self.cpu_cycles as u16 & 1);
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct DmcDma {
    address: u16,
    cycles_remaining: u8,
}
