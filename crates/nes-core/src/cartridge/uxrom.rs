use serde::{Deserialize, Serialize};

use super::{
    CartridgeError,
    mapper::{Mapper, MapperSnapshot, bank_offset, load_trainer},
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct UxromSnapshot {
    bank: u8,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
}

pub struct Uxrom {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    bank: u8,
}

impl Uxrom {
    pub fn new(
        prg_rom: Vec<u8>,
        chr: Vec<u8>,
        prg_ram_size: usize,
        trainer: Option<&[u8]>,
    ) -> Result<Self, CartridgeError> {
        if prg_rom.len() < 0x8000 || !prg_rom.len().is_multiple_of(0x4000) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 2,
                kind: "PRG ROM",
                size: prg_rom.len(),
            });
        }
        if !chr.is_empty() && !chr.len().is_multiple_of(0x2000) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 2,
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
            bank: 0,
        })
    }

    fn read_prg(&self, address: u16) -> u8 {
        let bank_count = self.prg_rom.len() / 0x4000;
        let bank = if address < 0xc000 {
            usize::from(self.bank)
        } else {
            bank_count - 1
        };
        self.prg_rom[bank_offset(
            bank,
            0x4000,
            usize::from(address & 0x3fff),
            self.prg_rom.len(),
        )]
    }
}

impl Mapper for Uxrom {
    fn cpu_read(&mut self, address: u16) -> Option<u8> {
        self.cpu_peek(address)
    }

    fn cpu_peek(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7fff => {
                Some(self.prg_ram[usize::from(address - 0x6000) % self.prg_ram.len()])
            }
            0x8000..=0xffff => Some(self.read_prg(address)),
            _ => None,
        }
    }

    fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7fff => {
                let offset = usize::from(address - 0x6000) % self.prg_ram.len();
                self.prg_ram[offset] = value;
                true
            }
            0x8000..=0xffff => {
                self.bank = value;
                true
            }
            _ => false,
        }
    }

    fn ppu_read(&mut self, address: u16) -> Option<u8> {
        (address <= 0x1fff).then(|| self.chr[usize::from(address) % self.chr.len()])
    }

    fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address > 0x1fff {
            return false;
        }
        if self.chr_is_ram {
            let offset = usize::from(address) % self.chr.len();
            self.chr[offset] = value;
        }
        true
    }

    fn battery_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }
    fn load_battery_ram(&mut self, data: &[u8]) {
        let count = data.len().min(self.prg_ram.len());
        self.prg_ram[..count].copy_from_slice(&data[..count]);
    }
    fn snapshot(&self) -> MapperSnapshot {
        MapperSnapshot::Uxrom(UxromSnapshot {
            bank: self.bank,
            prg_ram: self.prg_ram.clone(),
            chr: self.chr.clone(),
        })
    }
    fn restore_snapshot(&mut self, snapshot: &MapperSnapshot) -> bool {
        let MapperSnapshot::Uxrom(snapshot) = snapshot else {
            return false;
        };
        if snapshot.prg_ram.len() != self.prg_ram.len() || snapshot.chr.len() != self.chr.len() {
            return false;
        }
        self.bank = snapshot.bank;
        self.prg_ram.copy_from_slice(&snapshot.prg_ram);
        if self.chr_is_ram {
            self.chr.copy_from_slice(&snapshot.chr);
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
            && self.chr.get_mut(offset).is_some_and(|byte| {
                *byte = value;
                true
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_the_low_bank_and_keeps_the_last_bank_fixed() {
        let mut prg = vec![0; 4 * 0x4000];
        for bank in 0..4 {
            prg[bank * 0x4000] = bank as u8;
        }
        let mut mapper = Uxrom::new(prg, vec![], 0x2000, None).unwrap();
        assert_eq!(mapper.cpu_read(0x8000), Some(0));
        assert_eq!(mapper.cpu_read(0xc000), Some(3));
        mapper.cpu_write(0x8000, 2);
        assert_eq!(mapper.cpu_read(0x8000), Some(2));
        assert_eq!(mapper.cpu_read(0xc000), Some(3));
    }
}
