use serde::{Deserialize, Serialize};

use super::{
    CartridgeError,
    mapper::{Mapper, MapperSnapshot},
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct NromSnapshot {
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
}

pub struct Nrom {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
}

impl Nrom {
    pub fn new(prg_rom: Vec<u8>, chr: Vec<u8>) -> Result<Self, CartridgeError> {
        if !matches!(prg_rom.len(), 0x4000 | 0x8000) {
            return Err(CartridgeError::InvalidNromPrgSize(prg_rom.len()));
        }
        let chr_is_ram = chr.is_empty();
        Ok(Self {
            prg_rom,
            prg_ram: vec![0; 0x2000],
            chr: if chr_is_ram { vec![0; 0x2000] } else { chr },
            chr_is_ram,
        })
    }

    pub fn load_trainer(&mut self, trainer: &[u8]) {
        let count = trainer.len().min(512);
        self.prg_ram[0x1000..0x1000 + count].copy_from_slice(&trainer[..count]);
    }
}

impl Mapper for Nrom {
    fn cpu_read(&mut self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7fff => Some(self.prg_ram[(address - 0x6000) as usize]),
            0x8000..=0xffff => {
                let offset = (address - 0x8000) as usize % self.prg_rom.len();
                Some(self.prg_rom[offset])
            }
            _ => None,
        }
    }

    fn cpu_peek(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7fff => Some(self.prg_ram[(address - 0x6000) as usize]),
            0x8000..=0xffff => {
                let offset = (address - 0x8000) as usize % self.prg_rom.len();
                Some(self.prg_rom[offset])
            }
            _ => None,
        }
    }

    fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7fff => {
                self.prg_ram[(address - 0x6000) as usize] = value;
                true
            }
            0x8000..=0xffff => true,
            _ => false,
        }
    }

    fn ppu_read(&mut self, address: u16) -> Option<u8> {
        (address <= 0x1fff).then(|| self.chr[address as usize])
    }

    fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address <= 0x1fff {
            if self.chr_is_ram {
                self.chr[address as usize] = value;
            }
            true
        } else {
            false
        }
    }

    fn battery_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }

    fn load_battery_ram(&mut self, data: &[u8]) {
        let count = data.len().min(self.prg_ram.len());
        self.prg_ram[..count].copy_from_slice(&data[..count]);
    }

    fn snapshot(&self) -> MapperSnapshot {
        MapperSnapshot::Nrom(NromSnapshot {
            prg_ram: self.prg_ram.clone(),
            chr: self.chr.clone(),
        })
    }

    fn restore_snapshot(&mut self, snapshot: &MapperSnapshot) -> bool {
        let MapperSnapshot::Nrom(snapshot) = snapshot;
        if snapshot.prg_ram.len() != self.prg_ram.len() || snapshot.chr.len() != self.chr.len() {
            return false;
        }
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
        if self.chr_is_ram
            && let Some(byte) = self.chr.get_mut(offset)
        {
            *byte = value;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrors_16k_prg() {
        let mut rom = vec![0; 0x4000];
        rom[0] = 0x12;
        let mut mapper = Nrom::new(rom, vec![]).unwrap();
        assert_eq!(mapper.cpu_read(0x8000), Some(0x12));
        assert_eq!(mapper.cpu_read(0xc000), Some(0x12));
    }
}
