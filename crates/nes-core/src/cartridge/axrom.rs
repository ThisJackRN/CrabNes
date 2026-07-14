use serde::{Deserialize, Serialize};

use super::{
    CartridgeError, Mirroring,
    mapper::{Mapper, MapperSnapshot, bank_offset, load_trainer},
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct AxromSnapshot {
    register: u8,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
}

pub struct Axrom {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    register: u8,
}

impl Axrom {
    pub fn new(
        prg_rom: Vec<u8>,
        chr: Vec<u8>,
        prg_ram_size: usize,
        trainer: Option<&[u8]>,
    ) -> Result<Self, CartridgeError> {
        if prg_rom.len() < 0x8000 || !prg_rom.len().is_multiple_of(0x8000) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 7,
                kind: "PRG ROM",
                size: prg_rom.len(),
            });
        }
        if !chr.is_empty() && !chr.len().is_multiple_of(0x2000) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 7,
                kind: "CHR",
                size: chr.len(),
            });
        }
        let mut prg_ram = vec![0; prg_ram_size.max(0x2000)];
        load_trainer(&mut prg_ram, trainer);
        Ok(Self {
            prg_rom,
            prg_ram,
            chr: if chr.is_empty() { vec![0; 0x2000] } else { chr },
            register: 0,
        })
    }
}

impl Mapper for Axrom {
    fn cpu_read(&mut self, address: u16) -> Option<u8> {
        self.cpu_peek(address)
    }
    fn cpu_peek(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7fff => {
                Some(self.prg_ram[usize::from(address - 0x6000) % self.prg_ram.len()])
            }
            0x8000..=0xffff => Some(
                self.prg_rom[bank_offset(
                    usize::from(self.register & 0x0f),
                    0x8000,
                    usize::from(address - 0x8000),
                    self.prg_rom.len(),
                )],
            ),
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
                self.register = value;
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
        let offset = usize::from(address) % self.chr.len();
        self.chr[offset] = value;
        true
    }
    fn mirroring(&self) -> Option<Mirroring> {
        Some(if self.register & 0x10 == 0 {
            Mirroring::SingleScreenLower
        } else {
            Mirroring::SingleScreenUpper
        })
    }
    fn battery_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }
    fn load_battery_ram(&mut self, data: &[u8]) {
        let count = data.len().min(self.prg_ram.len());
        self.prg_ram[..count].copy_from_slice(&data[..count]);
    }
    fn snapshot(&self) -> MapperSnapshot {
        MapperSnapshot::Axrom(AxromSnapshot {
            register: self.register,
            prg_ram: self.prg_ram.clone(),
            chr: self.chr.clone(),
        })
    }
    fn restore_snapshot(&mut self, snapshot: &MapperSnapshot) -> bool {
        let MapperSnapshot::Axrom(snapshot) = snapshot else {
            return false;
        };
        if snapshot.prg_ram.len() != self.prg_ram.len() || snapshot.chr.len() != self.chr.len() {
            return false;
        }
        self.register = snapshot.register;
        self.prg_ram.copy_from_slice(&snapshot.prg_ram);
        self.chr.copy_from_slice(&snapshot.chr);
        true
    }
    fn prg_rom(&self) -> &[u8] {
        &self.prg_rom
    }
    fn chr(&self) -> &[u8] {
        &self.chr
    }
    fn chr_is_writable(&self) -> bool {
        true
    }
    fn debug_write_chr(&mut self, offset: usize, value: u8) -> bool {
        self.chr.get_mut(offset).is_some_and(|byte| {
            *byte = value;
            true
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switches_prg_and_single_screen_page_together() {
        let mut prg = vec![0; 2 * 0x8000];
        prg[0] = 1;
        prg[0x8000] = 2;
        let mut mapper = Axrom::new(prg, vec![], 0x2000, None).unwrap();
        mapper.cpu_write(0x8000, 0x11);
        assert_eq!(mapper.cpu_read(0x8000), Some(2));
        assert_eq!(mapper.mirroring(), Some(Mirroring::SingleScreenUpper));
    }
}
