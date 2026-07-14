use serde::{Deserialize, Serialize};

use super::{
    CartridgeError, Mirroring,
    mapper::{Mapper, MapperSnapshot, bank_offset, load_trainer},
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Mmc2Snapshot {
    mapper_id: u16,
    prg_bank: u8,
    chr_banks: [u8; 4],
    latches: [bool; 2],
    mirroring: Mirroring,
    prg_ram: Vec<u8>,
}

pub struct Mmc2 {
    mapper_id: u16,
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    prg_bank: u8,
    chr_banks: [u8; 4],
    // false = FD, true = FE
    latches: [bool; 2],
    mirroring: Mirroring,
}

impl Mmc2 {
    pub fn new(
        mapper_id: u16,
        prg_rom: Vec<u8>,
        chr: Vec<u8>,
        prg_ram_size: usize,
        trainer: Option<&[u8]>,
    ) -> Result<Self, CartridgeError> {
        let unit = if mapper_id == 9 { 0x2000 } else { 0x4000 };
        if prg_rom.len() < 0x8000 || !prg_rom.len().is_multiple_of(unit) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: mapper_id,
                kind: "PRG ROM",
                size: prg_rom.len(),
            });
        }
        if chr.is_empty() || !chr.len().is_multiple_of(0x1000) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: mapper_id,
                kind: "CHR ROM",
                size: chr.len(),
            });
        }
        let mut prg_ram = vec![0; prg_ram_size.max(0x2000)];
        load_trainer(&mut prg_ram, trainer);
        Ok(Self {
            mapper_id,
            prg_rom,
            prg_ram,
            chr,
            prg_bank: 0,
            chr_banks: [0; 4],
            latches: [true; 2],
            mirroring: Mirroring::Vertical,
        })
    }

    fn prg_offset(&self, address: u16) -> usize {
        if self.mapper_id == 9 {
            let count = self.prg_rom.len() / 0x2000;
            let slot = usize::from((address - 0x8000) / 0x2000);
            let bank = if slot == 0 {
                usize::from(self.prg_bank)
            } else {
                count - 4 + slot
            };
            bank_offset(
                bank,
                0x2000,
                usize::from(address & 0x1fff),
                self.prg_rom.len(),
            )
        } else if address < 0xc000 {
            bank_offset(
                usize::from(self.prg_bank),
                0x4000,
                usize::from(address - 0x8000),
                self.prg_rom.len(),
            )
        } else {
            self.prg_rom.len() - 0x4000 + usize::from(address - 0xc000)
        }
    }

    fn chr_offset(&self, address: u16) -> usize {
        let half = usize::from(address >= 0x1000);
        let bank = usize::from(self.chr_banks[half * 2 + usize::from(self.latches[half])]);
        bank_offset(bank, 0x1000, usize::from(address & 0x0fff), self.chr.len())
    }

    fn update_latch(&mut self, address: u16) {
        match address {
            0x0fd8..=0x0fdf if self.mapper_id == 10 || address == 0x0fd8 => self.latches[0] = false,
            0x0fe8..=0x0fef if self.mapper_id == 10 || address == 0x0fe8 => self.latches[0] = true,
            0x1fd8..=0x1fdf => self.latches[1] = false,
            0x1fe8..=0x1fef => self.latches[1] = true,
            _ => {}
        }
    }
}

impl Mapper for Mmc2 {
    fn cpu_read(&mut self, address: u16) -> Option<u8> {
        self.cpu_peek(address)
    }
    fn cpu_peek(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7fff => {
                Some(self.prg_ram[usize::from(address - 0x6000) % self.prg_ram.len()])
            }
            0x8000..=0xffff => Some(self.prg_rom[self.prg_offset(address)]),
            _ => None,
        }
    }
    fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7fff => {
                let o = usize::from(address - 0x6000) % self.prg_ram.len();
                self.prg_ram[o] = value;
                true
            }
            0xa000..=0xafff => {
                self.prg_bank = value & 0x0f;
                true
            }
            0xb000..=0xbfff => {
                self.chr_banks[0] = value & 0x1f;
                true
            }
            0xc000..=0xcfff => {
                self.chr_banks[1] = value & 0x1f;
                true
            }
            0xd000..=0xdfff => {
                self.chr_banks[2] = value & 0x1f;
                true
            }
            0xe000..=0xefff => {
                self.chr_banks[3] = value & 0x1f;
                true
            }
            0xf000..=0xffff => {
                self.mirroring = if value & 1 == 0 {
                    Mirroring::Vertical
                } else {
                    Mirroring::Horizontal
                };
                true
            }
            0x8000..=0x9fff => true,
            _ => false,
        }
    }
    fn ppu_read(&mut self, address: u16) -> Option<u8> {
        if address > 0x1fff {
            return None;
        }
        let value = self.chr[self.chr_offset(address)];
        self.update_latch(address);
        Some(value)
    }
    fn ppu_write(&mut self, address: u16, _value: u8) -> bool {
        if address <= 0x1fff {
            self.update_latch(address);
            true
        } else {
            false
        }
    }
    fn mirroring(&self) -> Option<Mirroring> {
        Some(self.mirroring)
    }
    fn battery_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }
    fn load_battery_ram(&mut self, data: &[u8]) {
        let c = data.len().min(self.prg_ram.len());
        self.prg_ram[..c].copy_from_slice(&data[..c]);
    }
    fn snapshot(&self) -> MapperSnapshot {
        MapperSnapshot::Mmc2(Mmc2Snapshot {
            mapper_id: self.mapper_id,
            prg_bank: self.prg_bank,
            chr_banks: self.chr_banks,
            latches: self.latches,
            mirroring: self.mirroring,
            prg_ram: self.prg_ram.clone(),
        })
    }
    fn restore_snapshot(&mut self, snapshot: &MapperSnapshot) -> bool {
        let MapperSnapshot::Mmc2(s) = snapshot else {
            return false;
        };
        if s.mapper_id != self.mapper_id || s.prg_ram.len() != self.prg_ram.len() {
            return false;
        }
        self.prg_bank = s.prg_bank;
        self.chr_banks = s.chr_banks;
        self.latches = s.latches;
        self.mirroring = s.mirroring;
        self.prg_ram.copy_from_slice(&s.prg_ram);
        true
    }
    fn prg_rom(&self) -> &[u8] {
        &self.prg_rom
    }
    fn chr(&self) -> &[u8] {
        &self.chr
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn latch_changes_the_selected_chr_bank() {
        let mut chr = vec![0; 4 * 0x1000];
        for b in 0..4 {
            chr[b * 0x1000] = b as u8;
        }
        let mut m = Mmc2::new(9, vec![0; 0x10000], chr, 0x2000, None).unwrap();
        m.cpu_write(0xb000, 1);
        m.cpu_write(0xc000, 2);
        assert_eq!(m.ppu_read(0), Some(2));
        m.ppu_read(0x0fd8);
        assert_eq!(m.ppu_read(0), Some(1));
    }
}
