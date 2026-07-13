use crate::{apu::Apu, cartridge::Cartridge, controller::Controller, ppu::Ppu};

pub struct Bus {
    ram: [u8; 0x800],
    pub ppu: Ppu,
    pub apu: Apu,
    pub cartridge: Cartridge,
    pub controllers: [Controller; 2],
    dma_stall: u16,
    cpu_cycles: u64,
    open_bus: u8,
}

impl Bus {
    pub fn new(cartridge: Cartridge) -> Self {
        Self {
            ram: [0; 0x800],
            ppu: Ppu::default(),
            apu: Apu::default(),
            cartridge,
            controllers: [Controller::default(), Controller::default()],
            dma_stall: 0,
            cpu_cycles: 0,
            open_bus: 0,
        }
    }

    pub fn reset(&mut self) {
        self.ppu.reset();
        self.apu.reset();
        self.dma_stall = 0;
        self.cpu_cycles = 0;
        self.open_bus = 0;
    }

    pub fn read(&mut self, address: u16) -> u8 {
        let value = if let Some(value) = self.cartridge.cpu_read(address) {
            value
        } else {
            match address {
                0x0000..=0x1fff => self.ram[address as usize & 0x07ff],
                0x2000..=0x3fff => self.ppu.cpu_read(address & 7, &mut self.cartridge),
                0x4015 => self.apu.read_status(),
                0x4016 => self.controllers[0].read(),
                0x4017 => self.controllers[1].read(),
                _ => self.open_bus,
            }
        };
        self.open_bus = value;
        value
    }

    pub fn write(&mut self, address: u16, value: u8) {
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

    pub fn clock_cpu_cycles(&mut self, count: u16) {
        let mut remaining = u32::from(count + std::mem::take(&mut self.dma_stall));
        while remaining > 0 {
            self.apu.clock();
            if let Some(address) = self.apu.take_dmc_dma_request() {
                let value = self.cartridge.cpu_read(address).unwrap_or(self.open_bus);
                self.open_bus = value;
                self.apu.supply_dmc_sample(value);
                // A DMC fetch halts the 2A03 CPU while the APU and PPU continue.
                remaining += 4;
            }
            for _ in 0..3 {
                self.ppu.clock(&mut self.cartridge);
            }
            self.cpu_cycles = self.cpu_cycles.wrapping_add(1);
            remaining -= 1;
        }
    }

    pub fn irq_pending(&self) -> bool {
        self.apu.irq_pending()
    }

    pub fn cpu_cycles(&self) -> u64 {
        self.cpu_cycles
    }

    fn perform_oam_dma(&mut self, page: u8) {
        let mut data = [0; 256];
        let base = (page as u16) << 8;
        for (offset, slot) in data.iter_mut().enumerate() {
            *slot = self.read(base.wrapping_add(offset as u16));
        }
        self.ppu.write_oam_dma(&data);
        self.dma_stall = 513 + (self.cpu_cycles as u16 & 1);
    }
}
