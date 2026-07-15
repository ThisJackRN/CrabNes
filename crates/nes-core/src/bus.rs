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
    /// Data latch inside the 2A03. A $4015 read affects this latch without
    /// driving the cartridge-side CPU data bus.
    internal_data_bus: u8,
    /// Last value driven on the cartridge-side CPU data bus.
    open_bus: u8,
    dmc_completed_last_cpu_slot: bool,
    cpu_sequence_cycles: u16,
    nmi_samples: Vec<bool>,
    irq_samples: Vec<bool>,
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
    internal_data_bus: u8,
    open_bus: u8,
    dmc_completed_last_cpu_slot: bool,
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
            internal_data_bus: 0,
            open_bus: 0,
            dmc_completed_last_cpu_slot: false,
            cpu_sequence_cycles: 0,
            nmi_samples: Vec::with_capacity(8),
            irq_samples: Vec::with_capacity(8),
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
        self.internal_data_bus = 0;
        self.open_bus = 0;
        self.dmc_completed_last_cpu_slot = false;
        self.cpu_sequence_cycles = 0;
        self.nmi_samples.clear();
        self.irq_samples.clear();
    }

    pub fn read(&mut self, address: u16) -> u8 {
        self.advance_cpu_slot(true);
        self.record_irq_line();
        let value = self.read_untimed(address);
        self.record_nmi_line();
        value
    }

    fn read_untimed(&mut self, address: u16) -> u8 {
        let (value, drives_external_bus) = if let Some(value) = self.cartridge.cpu_read(address) {
            (value, true)
        } else {
            match address {
                0x0000..=0x1fff => (self.ram[address as usize & 0x07ff], true),
                0x2000..=0x3fff => (self.ppu.cpu_read(address & 7, &mut self.cartridge), true),
                // $4015 is entirely internal to the 2A03. Bit 5 retains the
                // internal latch, and the read does not drive the external bus.
                0x4015 => (
                    self.apu.read_status() | (self.internal_data_bus & 0x20),
                    false,
                ),
                // The controller drives only the low serial bit. The upper
                // three bits retain CPU open bus on an NTSC NES.
                0x4016 if self.cartridge.mapper_id() == 99 => (
                    self.controllers[0].read() | u8::from(self.controllers[0].coin()) * 0x20,
                    true,
                ),
                0x4017 if self.cartridge.mapper_id() == 99 => (self.controllers[1].read(), true),
                0x4016 => (self.controllers[0].read() | (self.open_bus & 0xe0), true),
                0x4017 => (self.controllers[1].read() | (self.open_bus & 0xe0), true),
                _ => (self.open_bus, false),
            }
        };
        self.internal_data_bus = value;
        if drives_external_bus {
            self.open_bus = value;
        }
        value
    }

    pub fn write(&mut self, address: u16, value: u8) {
        self.advance_cpu_slot(false);
        self.record_irq_line();
        self.write_untimed(address, value);
        self.record_nmi_line();
    }

    fn write_untimed(&mut self, address: u16, value: u8) {
        self.internal_data_bus = value;
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
        self.nmi_samples.clear();
        self.irq_samples.clear();
    }

    /// Completes the idle slots in a CPU instruction, reset, or interrupt
    /// sequence. Memory accesses have already advanced their individual slots.
    pub(crate) fn finish_cpu_sequence(&mut self, target_cycles: u16) -> u16 {
        while self.cpu_sequence_cycles < target_cycles {
            self.advance_cpu_slot(true);
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

    fn advance_cpu_slot(&mut self, can_service_dmc: bool) -> bool {
        let dmc_completed = self.service_dma_stalls(can_service_dmc);
        self.clock_hardware_cycle();
        self.cpu_sequence_cycles = self.cpu_sequence_cycles.saturating_add(1);
        self.dmc_completed_last_cpu_slot = dmc_completed;
        dmc_completed
    }

    fn service_dma_stalls(&mut self, can_service_dmc: bool) -> bool {
        let mut dmc_completed = false;
        if self.dma_stall == 0 && !can_service_dmc {
            return false;
        }
        if self.dma_stall == 0
            && self
                .dmc_dma
                .as_ref()
                .is_some_and(|dma| dma.wait_for_alignment)
        {
            self.dmc_dma.as_mut().unwrap().wait_for_alignment = false;
            return false;
        }
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
                    // The DMC owns the external CPU bus during its fetch, but
                    // does not overwrite the 2A03's internal data latch.
                    self.open_bus = value;
                    self.apu.supply_dmc_sample(value);
                    dmc_completed = true;
                }
            } else {
                self.dma_stall -= 1;
            }
        }
        dmc_completed
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
        let dmc_completed = self.advance_cpu_slot(false);
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

        if self.dmc_dma.is_none()
            && let Some(address) = self.apu.take_dmc_dma_request()
        {
            // The request halts the CPU before its next bus slot. More exact
            // read/write collision behavior can now be expressed here without
            // changing the CPU instruction decoder.
            self.dmc_dma = Some(DmcDma {
                address,
                cycles_remaining: 4,
                wait_for_alignment: self.cpu_cycles & 1 == 0,
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
        self.dma_stall = snapshot.dma_stall;
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
        bus.controllers[0].write_strobe(1);
        bus.open_bus = 0xe0;
        bus.internal_data_bus = 0xe0;
        assert_eq!(bus.read_untimed(0x4016), 0xe1);
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
            cycles_remaining: 1,
            wait_for_alignment: false,
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
            cycles_remaining: 1,
            wait_for_alignment: false,
        });
        bus.read(0x0000);
        bus.write_unstable_store(0x0001, 0x11, 0x0002, 0x22);
        assert_eq!(bus.ram[1], 0);
        assert_eq!(bus.ram[2], 0x22);
    }

    #[test]
    fn dmc_dma_waits_through_cpu_writes_and_alignment_slot() {
        let mut bus = Bus::new(nrom_cartridge());
        bus.dmc_dma = Some(DmcDma {
            address: 0x8000,
            cycles_remaining: 1,
            wait_for_alignment: true,
        });

        bus.write(0x0000, 0x5a);
        assert_eq!(bus.ram[0], 0x5a);
        assert!(bus.dmc_dma.is_some());

        bus.read(0x0000);
        assert!(
            bus.dmc_dma.is_some(),
            "first eligible read is the alignment slot"
        );
        bus.read(0x0000);
        assert!(bus.dmc_dma.is_none());
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct DmcDma {
    address: u16,
    cycles_remaining: u8,
    wait_for_alignment: bool,
}
