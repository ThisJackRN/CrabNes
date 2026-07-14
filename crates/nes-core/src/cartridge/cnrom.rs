use serde::{Deserialize, Serialize};

use super::{
    CartridgeError,
    mapper::{Mapper, MapperSnapshot, bank_offset, load_trainer},
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CnromSnapshot {
    chr_bank: u8,
    prg_ram: Vec<u8>,
}

pub struct Cnrom {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_bank: u8,
}

impl Cnrom {
    pub fn new(
        prg_rom: Vec<u8>,
        chr: Vec<u8>,
        prg_ram_size: usize,
        trainer: Option<&[u8]>,
    ) -> Result<Self, CartridgeError> {
        if !matches!(prg_rom.len(), 0x4000 | 0x8000) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 3,
                kind: "PRG ROM",
                size: prg_rom.len(),
            });
        }
        if chr.is_empty() || !chr.len().is_multiple_of(0x2000) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 3,
                kind: "CHR ROM",
                size: chr.len(),
            });
        }
        let mut prg_ram = vec![0; prg_ram_size.max(0x2000)];
        load_trainer(&mut prg_ram, trainer);
        Ok(Self {
            prg_rom,
            prg_ram,
            chr,
            chr_bank: 0,
        })
    }
}

impl Mapper for Cnrom {
    fn cpu_read(&mut self, address: u16) -> Option<u8> {
        self.cpu_peek(address)
    }
    fn cpu_peek(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7fff => {
                Some(self.prg_ram[usize::from(address - 0x6000) % self.prg_ram.len()])
            }
            0x8000..=0xffff => {
                Some(self.prg_rom[usize::from(address - 0x8000) % self.prg_rom.len()])
            }
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
                self.chr_bank = value;
                true
            }
            _ => false,
        }
    }
    fn ppu_read(&mut self, address: u16) -> Option<u8> {
        (address <= 0x1fff).then(|| {
            self.chr[bank_offset(
                usize::from(self.chr_bank),
                0x2000,
                usize::from(address),
                self.chr.len(),
            )]
        })
    }
    fn ppu_write(&mut self, address: u16, _value: u8) -> bool {
        address <= 0x1fff
    }
    fn battery_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }
    fn load_battery_ram(&mut self, data: &[u8]) {
        let count = data.len().min(self.prg_ram.len());
        self.prg_ram[..count].copy_from_slice(&data[..count]);
    }
    fn snapshot(&self) -> MapperSnapshot {
        MapperSnapshot::Cnrom(CnromSnapshot {
            chr_bank: self.chr_bank,
            prg_ram: self.prg_ram.clone(),
        })
    }
    fn restore_snapshot(&mut self, snapshot: &MapperSnapshot) -> bool {
        let MapperSnapshot::Cnrom(snapshot) = snapshot else {
            return false;
        };
        if snapshot.prg_ram.len() != self.prg_ram.len() {
            return false;
        }
        self.chr_bank = snapshot.chr_bank;
        self.prg_ram.copy_from_slice(&snapshot.prg_ram);
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
    fn switches_eight_kib_chr_banks() {
        let mut chr = vec![0; 3 * 0x2000];
        chr[0] = 1;
        chr[0x2000] = 2;
        chr[0x4000] = 3;
        let mut mapper = Cnrom::new(vec![0; 0x8000], chr, 0x2000, None).unwrap();
        mapper.cpu_write(0x8000, 2);
        assert_eq!(mapper.ppu_read(0), Some(3));
    }
}
