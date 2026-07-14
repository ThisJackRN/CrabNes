use serde::{Deserialize, Serialize};

use super::{
    CartridgeError, Mirroring,
    mapper::{Mapper, MapperSnapshot, bank_offset, load_trainer},
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Mmc3Snapshot {
    bank_select: u8,
    banks: [u8; 8],
    mirroring: Mirroring,
    prg_ram_enabled: bool,
    prg_ram_write_protect: bool,
    irq_latch: u8,
    irq_counter: u8,
    irq_reload: bool,
    irq_enabled: bool,
    irq_pending: bool,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
}

pub struct Mmc3 {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    bank_select: u8,
    banks: [u8; 8],
    mirroring: Mirroring,
    four_screen: bool,
    prg_ram_enabled: bool,
    prg_ram_write_protect: bool,
    irq_latch: u8,
    irq_counter: u8,
    irq_reload: bool,
    irq_enabled: bool,
    irq_pending: bool,
}

impl Mmc3 {
    pub fn new(
        prg_rom: Vec<u8>,
        chr: Vec<u8>,
        prg_ram_size: usize,
        mirroring: Mirroring,
        trainer: Option<&[u8]>,
    ) -> Result<Self, CartridgeError> {
        if prg_rom.len() < 0x8000 || !prg_rom.len().is_multiple_of(0x2000) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 4,
                kind: "PRG ROM",
                size: prg_rom.len(),
            });
        }
        if !chr.is_empty() && !chr.len().is_multiple_of(0x0400) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 4,
                kind: "CHR",
                size: chr.len(),
            });
        }
        let chr_is_ram = chr.is_empty();
        let mut prg_ram = vec![0; prg_ram_size.max(0x2000)];
        load_trainer(&mut prg_ram, trainer);
        Ok(Self {
            prg_rom,
            prg_ram,
            chr: if chr_is_ram { vec![0; 0x2000] } else { chr },
            chr_is_ram,
            bank_select: 0,
            banks: [0; 8],
            mirroring,
            four_screen: mirroring == Mirroring::FourScreen,
            prg_ram_enabled: true,
            prg_ram_write_protect: false,
            irq_latch: 0,
            irq_counter: 0,
            irq_reload: false,
            irq_enabled: false,
            irq_pending: false,
        })
    }

    fn prg_bank(&self, slot: usize) -> usize {
        let last = self.prg_rom.len() / 0x2000 - 1;
        let second_last = last - 1;
        match (self.bank_select & 0x40 != 0, slot) {
            (false, 0) | (true, 2) => usize::from(self.banks[6] & 0x3f),
            (_, 1) => usize::from(self.banks[7] & 0x3f),
            (false, 2) | (true, 0) => second_last,
            _ => last,
        }
    }

    fn chr_bank(&self, slot: usize) -> usize {
        let logical = if self.bank_select & 0x80 != 0 {
            (slot + 4) & 7
        } else {
            slot
        };
        match logical {
            0 => usize::from(self.banks[0] & 0xfe),
            1 => usize::from(self.banks[0] | 1),
            2 => usize::from(self.banks[1] & 0xfe),
            3 => usize::from(self.banks[1] | 1),
            4..=7 => usize::from(self.banks[logical - 2]),
            _ => unreachable!(),
        }
    }
}

impl Mapper for Mmc3 {
    fn cpu_read(&mut self, address: u16) -> Option<u8> {
        self.cpu_peek(address)
    }

    fn cpu_peek(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7fff if self.prg_ram_enabled => {
                Some(self.prg_ram[usize::from(address - 0x6000) % self.prg_ram.len()])
            }
            0x8000..=0xffff => {
                let slot = usize::from((address - 0x8000) / 0x2000);
                Some(
                    self.prg_rom[bank_offset(
                        self.prg_bank(slot),
                        0x2000,
                        usize::from(address & 0x1fff),
                        self.prg_rom.len(),
                    )],
                )
            }
            _ => None,
        }
    }

    fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7fff => {
                if self.prg_ram_enabled && !self.prg_ram_write_protect {
                    let offset = usize::from(address - 0x6000) % self.prg_ram.len();
                    self.prg_ram[offset] = value;
                }
                true
            }
            0x8000..=0x9fff if address & 1 == 0 => {
                self.bank_select = value;
                true
            }
            0x8000..=0x9fff => {
                self.banks[usize::from(self.bank_select & 7)] = value;
                true
            }
            0xa000..=0xbfff if address & 1 == 0 => {
                if !self.four_screen {
                    self.mirroring = if value & 1 == 0 {
                        Mirroring::Vertical
                    } else {
                        Mirroring::Horizontal
                    };
                }
                true
            }
            0xa000..=0xbfff => {
                self.prg_ram_enabled = value & 0x80 != 0;
                self.prg_ram_write_protect = value & 0x40 != 0;
                true
            }
            0xc000..=0xdfff if address & 1 == 0 => {
                self.irq_latch = value;
                true
            }
            0xc000..=0xdfff => {
                self.irq_counter = 0;
                self.irq_reload = true;
                true
            }
            0xe000..=0xffff if address & 1 == 0 => {
                self.irq_enabled = false;
                self.irq_pending = false;
                true
            }
            0xe000..=0xffff => {
                self.irq_enabled = true;
                true
            }
            _ => false,
        }
    }

    fn ppu_read(&mut self, address: u16) -> Option<u8> {
        (address <= 0x1fff).then(|| {
            let slot = usize::from(address / 0x0400);
            self.chr[bank_offset(
                self.chr_bank(slot),
                0x0400,
                usize::from(address & 0x03ff),
                self.chr.len(),
            )]
        })
    }

    fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address > 0x1fff {
            return false;
        }
        if self.chr_is_ram {
            let slot = usize::from(address / 0x0400);
            let offset = bank_offset(
                self.chr_bank(slot),
                0x0400,
                usize::from(address & 0x03ff),
                self.chr.len(),
            );
            self.chr[offset] = value;
        }
        true
    }

    fn mirroring(&self) -> Option<Mirroring> {
        Some(self.mirroring)
    }
    fn clock_scanline(&mut self) {
        if self.irq_counter == 0 || self.irq_reload {
            self.irq_counter = self.irq_latch;
            self.irq_reload = false;
        } else {
            self.irq_counter -= 1;
        }
        if self.irq_counter == 0 && self.irq_enabled {
            self.irq_pending = true;
        }
    }
    fn irq_pending(&self) -> bool {
        self.irq_pending
    }
    fn reset(&mut self) {
        self.irq_enabled = false;
        self.irq_pending = false;
        self.irq_reload = false;
    }
    fn battery_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }
    fn load_battery_ram(&mut self, data: &[u8]) {
        let count = data.len().min(self.prg_ram.len());
        self.prg_ram[..count].copy_from_slice(&data[..count]);
    }
    fn snapshot(&self) -> MapperSnapshot {
        MapperSnapshot::Mmc3(Mmc3Snapshot {
            bank_select: self.bank_select,
            banks: self.banks,
            mirroring: self.mirroring,
            prg_ram_enabled: self.prg_ram_enabled,
            prg_ram_write_protect: self.prg_ram_write_protect,
            irq_latch: self.irq_latch,
            irq_counter: self.irq_counter,
            irq_reload: self.irq_reload,
            irq_enabled: self.irq_enabled,
            irq_pending: self.irq_pending,
            prg_ram: self.prg_ram.clone(),
            chr: self.chr.clone(),
        })
    }
    fn restore_snapshot(&mut self, snapshot: &MapperSnapshot) -> bool {
        let MapperSnapshot::Mmc3(s) = snapshot else {
            return false;
        };
        if s.prg_ram.len() != self.prg_ram.len() || s.chr.len() != self.chr.len() {
            return false;
        }
        self.bank_select = s.bank_select;
        self.banks = s.banks;
        self.mirroring = s.mirroring;
        self.prg_ram_enabled = s.prg_ram_enabled;
        self.prg_ram_write_protect = s.prg_ram_write_protect;
        self.irq_latch = s.irq_latch;
        self.irq_counter = s.irq_counter;
        self.irq_reload = s.irq_reload;
        self.irq_enabled = s.irq_enabled;
        self.irq_pending = s.irq_pending;
        self.prg_ram.copy_from_slice(&s.prg_ram);
        if self.chr_is_ram {
            self.chr.copy_from_slice(&s.chr);
        }
        true
    }
    fn prg_rom(&self) -> &[u8] {
        &self.prg_rom
    }
    fn chr(&self) -> &[u8] {
        &self.chr
    }
    fn chr_is_writable(&self) -> bool {
        self.chr_is_ram
    }
    fn debug_write_chr(&mut self, offset: usize, value: u8) -> bool {
        self.chr_is_ram
            && self.chr.get_mut(offset).is_some_and(|b| {
                *b = value;
                true
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_banks_and_raises_scanline_irq() {
        let mut prg = vec![0; 8 * 0x2000];
        for bank in 0..8 {
            prg[bank * 0x2000] = bank as u8;
        }
        let mut mapper = Mmc3::new(prg, vec![], 0x2000, Mirroring::Vertical, None).unwrap();
        mapper.cpu_write(0x8000, 6);
        mapper.cpu_write(0x8001, 3);
        assert_eq!(mapper.cpu_read(0x8000), Some(3));
        assert_eq!(mapper.cpu_read(0xc000), Some(6));
        mapper.cpu_write(0xc000, 2);
        mapper.cpu_write(0xe001, 0);
        mapper.clock_scanline();
        assert!(!mapper.irq_pending());
        mapper.clock_scanline();
        assert!(!mapper.irq_pending());
        mapper.clock_scanline();
        assert!(mapper.irq_pending());
    }
}
