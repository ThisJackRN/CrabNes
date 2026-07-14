use serde::{Deserialize, Serialize};

use super::{
    CartridgeError, Mirroring,
    mapper::{Mapper, MapperSnapshot, bank_offset, load_trainer},
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Mmc1Snapshot {
    shift: u8,
    control: u8,
    chr_bank_0: u8,
    chr_bank_1: u8,
    prg_bank: u8,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
}

pub struct Mmc1 {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    shift: u8,
    control: u8,
    chr_bank_0: u8,
    chr_bank_1: u8,
    prg_bank: u8,
}

impl Mmc1 {
    pub fn new(
        prg_rom: Vec<u8>,
        chr: Vec<u8>,
        prg_ram_size: usize,
        trainer: Option<&[u8]>,
    ) -> Result<Self, CartridgeError> {
        if prg_rom.len() < 0x8000 || !prg_rom.len().is_multiple_of(0x4000) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 1,
                kind: "PRG ROM",
                size: prg_rom.len(),
            });
        }
        if !chr.is_empty() && !chr.len().is_multiple_of(0x1000) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 1,
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
            shift: 0x10,
            control: 0x0c,
            chr_bank_0: 0,
            chr_bank_1: 0,
            prg_bank: 0,
        })
    }

    fn prg_offset(&self, address: u16) -> usize {
        let mode = (self.control >> 2) & 3;
        let bank = usize::from(self.prg_bank & 0x0f);
        match mode {
            0 | 1 => bank_offset(
                bank >> 1,
                0x8000,
                usize::from(address - 0x8000),
                self.prg_rom.len(),
            ),
            2 if address < 0xc000 => usize::from(address - 0x8000),
            2 => bank_offset(
                bank,
                0x4000,
                usize::from(address - 0xc000),
                self.prg_rom.len(),
            ),
            3 if address < 0xc000 => bank_offset(
                bank,
                0x4000,
                usize::from(address - 0x8000),
                self.prg_rom.len(),
            ),
            _ => self.prg_rom.len() - 0x4000 + usize::from(address - 0xc000),
        }
    }

    fn chr_offset(&self, address: u16) -> usize {
        if self.control & 0x10 == 0 {
            bank_offset(
                usize::from(self.chr_bank_0 & !1),
                0x1000,
                usize::from(address),
                self.chr.len(),
            )
        } else if address < 0x1000 {
            bank_offset(
                usize::from(self.chr_bank_0),
                0x1000,
                usize::from(address),
                self.chr.len(),
            )
        } else {
            bank_offset(
                usize::from(self.chr_bank_1),
                0x1000,
                usize::from(address - 0x1000),
                self.chr.len(),
            )
        }
    }

    fn commit_register(&mut self, address: u16, value: u8) {
        match address {
            0x8000..=0x9fff => self.control = value,
            0xa000..=0xbfff => self.chr_bank_0 = value,
            0xc000..=0xdfff => self.chr_bank_1 = value,
            _ => self.prg_bank = value,
        }
    }
}

impl Mapper for Mmc1 {
    fn cpu_read(&mut self, address: u16) -> Option<u8> {
        self.cpu_peek(address)
    }
    fn cpu_peek(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7fff if self.prg_bank & 0x10 == 0 => {
                Some(self.prg_ram[usize::from(address - 0x6000) % self.prg_ram.len()])
            }
            0x8000..=0xffff => Some(self.prg_rom[self.prg_offset(address)]),
            _ => None,
        }
    }
    fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7fff => {
                if self.prg_bank & 0x10 == 0 {
                    let offset = usize::from(address - 0x6000) % self.prg_ram.len();
                    self.prg_ram[offset] = value;
                }
                true
            }
            0x8000..=0xffff => {
                if value & 0x80 != 0 {
                    self.shift = 0x10;
                    self.control |= 0x0c;
                    return true;
                }
                let complete = self.shift & 1 != 0;
                self.shift = (self.shift >> 1) | ((value & 1) << 4);
                if complete {
                    let register = self.shift;
                    self.commit_register(address, register);
                    self.shift = 0x10;
                }
                true
            }
            _ => false,
        }
    }
    fn ppu_read(&mut self, address: u16) -> Option<u8> {
        (address <= 0x1fff).then(|| self.chr[self.chr_offset(address)])
    }
    fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address > 0x1fff {
            return false;
        }
        if self.chr_is_ram {
            let offset = self.chr_offset(address);
            self.chr[offset] = value;
        }
        true
    }
    fn mirroring(&self) -> Option<Mirroring> {
        Some(match self.control & 3 {
            0 => Mirroring::SingleScreenLower,
            1 => Mirroring::SingleScreenUpper,
            2 => Mirroring::Vertical,
            _ => Mirroring::Horizontal,
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
        MapperSnapshot::Mmc1(Mmc1Snapshot {
            shift: self.shift,
            control: self.control,
            chr_bank_0: self.chr_bank_0,
            chr_bank_1: self.chr_bank_1,
            prg_bank: self.prg_bank,
            prg_ram: self.prg_ram.clone(),
            chr: self.chr.clone(),
        })
    }
    fn restore_snapshot(&mut self, snapshot: &MapperSnapshot) -> bool {
        let MapperSnapshot::Mmc1(snapshot) = snapshot else {
            return false;
        };
        if snapshot.prg_ram.len() != self.prg_ram.len() || snapshot.chr.len() != self.chr.len() {
            return false;
        }
        self.shift = snapshot.shift;
        self.control = snapshot.control;
        self.chr_bank_0 = snapshot.chr_bank_0;
        self.chr_bank_1 = snapshot.chr_bank_1;
        self.prg_bank = snapshot.prg_bank;
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
    fn serial_write(mapper: &mut Mmc1, address: u16, value: u8) {
        for bit in 0..5 {
            mapper.cpu_write(address, (value >> bit) & 1);
        }
    }
    #[test]
    fn serial_register_selects_prg_and_mirroring() {
        let mut prg = vec![0; 4 * 0x4000];
        for bank in 0..4 {
            prg[bank * 0x4000] = bank as u8;
        }
        let mut mapper = Mmc1::new(prg, vec![], 0x2000, None).unwrap();
        serial_write(&mut mapper, 0xe000, 2);
        assert_eq!(mapper.cpu_read(0x8000), Some(2));
        assert_eq!(mapper.cpu_read(0xc000), Some(3));
        serial_write(&mut mapper, 0x8000, 2);
        assert_eq!(mapper.mirroring(), Some(Mirroring::Vertical));
    }
}
