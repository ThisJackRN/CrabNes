use serde::{Deserialize, Serialize};

use super::{
    CartridgeError, Mirroring,
    mapper::{Mapper, MapperSnapshot},
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Mapper99Snapshot {
    bank_select: bool,
    prg_ram: Vec<u8>,
}

/// Nintendo Vs. System mapper used by games such as Vs. Super Mario Bros.
/// OUT2 (bit 2 of CPU writes to $4016) selects the second 8 KiB CHR ROM and,
/// on the 40 KiB Gumshoe board, the alternate PRG bank at $8000-$9FFF.
pub struct Mapper99 {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    bank_select: bool,
}

impl Mapper99 {
    pub fn new(
        prg_rom: Vec<u8>,
        chr: Vec<u8>,
        _prg_ram_size: usize,
        trainer: Option<&[u8]>,
    ) -> Result<Self, CartridgeError> {
        if prg_rom.is_empty() || !prg_rom.len().is_multiple_of(0x2000) || prg_rom.len() > 0xa000 {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 99,
                kind: "PRG ROM",
                size: prg_rom.len(),
            });
        }
        if !matches!(chr.len(), 0x2000 | 0x4000) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 99,
                kind: "CHR ROM",
                size: chr.len(),
            });
        }

        // Vs. boards expose 2 KiB of shared RAM throughout $6000-$7FFF.
        let mut prg_ram = vec![0; 0x0800];
        if let Some(trainer) = trainer {
            let count = trainer.len().min(512);
            // $7000 is a mirror of the first byte of the 2 KiB RAM.
            prg_ram[..count].copy_from_slice(&trainer[..count]);
        }
        Ok(Self {
            prg_rom,
            prg_ram,
            chr,
            bank_select: false,
        })
    }

    fn prg_index(&self, address: u16) -> Option<usize> {
        match address {
            0x8000..=0x9fff if self.bank_select && self.prg_rom.len() > 0x8000 => {
                // Only Vs. Gumshoe populates this alternate 8 KiB PRG socket.
                let index = 0x8000 + usize::from(address - 0x8000);
                (index < self.prg_rom.len()).then_some(index)
            }
            0x8000..=0xffff => {
                let index = usize::from(address - 0x8000);
                (index < self.prg_rom.len().min(0x8000)).then_some(index)
            }
            _ => None,
        }
    }
}

impl Mapper for Mapper99 {
    fn cpu_read(&mut self, address: u16) -> Option<u8> {
        self.cpu_peek(address)
    }

    fn cpu_peek(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7fff => {
                Some(self.prg_ram[usize::from(address - 0x6000) % self.prg_ram.len()])
            }
            0x8000..=0xffff => self.prg_index(address).map(|index| self.prg_rom[index]),
            _ => None,
        }
    }

    fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x4016 => {
                self.bank_select = value & 0x04 != 0;
                // $4016 is also the controller strobe. Record OUT2 here, then
                // let the system bus deliver the same write to both controllers.
                false
            }
            0x6000..=0x7fff => {
                let index = usize::from(address - 0x6000) % self.prg_ram.len();
                self.prg_ram[index] = value;
                true
            }
            0x8000..=0xffff => true,
            _ => false,
        }
    }

    fn ppu_read(&mut self, address: u16) -> Option<u8> {
        if address > 0x1fff {
            return None;
        }
        let bank = usize::from(self.bank_select) * 0x2000;
        self.chr.get(bank + usize::from(address)).copied()
    }

    fn ppu_write(&mut self, address: u16, _value: u8) -> bool {
        address <= 0x1fff
    }

    fn mirroring(&self) -> Option<Mirroring> {
        Some(Mirroring::FourScreen)
    }

    fn battery_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }

    fn load_battery_ram(&mut self, data: &[u8]) {
        let count = data.len().min(self.prg_ram.len());
        self.prg_ram[..count].copy_from_slice(&data[..count]);
    }

    fn snapshot(&self) -> MapperSnapshot {
        MapperSnapshot::Mapper99(Mapper99Snapshot {
            bank_select: self.bank_select,
            prg_ram: self.prg_ram.clone(),
        })
    }

    fn restore_snapshot(&mut self, snapshot: &MapperSnapshot) -> bool {
        let MapperSnapshot::Mapper99(snapshot) = snapshot else {
            return false;
        };
        if snapshot.prg_ram.len() != self.prg_ram.len() {
            return false;
        }
        self.bank_select = snapshot.bank_select;
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
    fn switches_chr_with_out2_and_preserves_controller_strobe_delivery() {
        let mut chr = vec![0; 0x4000];
        chr[0] = 0x11;
        chr[0x2000] = 0x22;
        let mut mapper = Mapper99::new(vec![0; 0x8000], chr, 0x2000, None).unwrap();

        assert_eq!(mapper.ppu_read(0), Some(0x11));
        assert!(!mapper.cpu_write(0x4016, 0x04));
        assert_eq!(mapper.ppu_read(0), Some(0x22));
    }

    #[test]
    fn unpopulated_second_chr_socket_is_open_bus() {
        let mut mapper = Mapper99::new(vec![0; 0x8000], vec![0x33; 0x2000], 0x2000, None).unwrap();

        mapper.cpu_write(0x4016, 0x04);
        assert_eq!(mapper.ppu_read(0), None);
    }

    #[test]
    fn switches_the_optional_gumshoe_prg_bank() {
        let mut prg = vec![0; 0xa000];
        prg[0] = 0x10;
        prg[0x8000] = 0x20;
        let mut mapper = Mapper99::new(prg, vec![0; 0x4000], 0x2000, None).unwrap();

        assert_eq!(mapper.cpu_read(0x8000), Some(0x10));
        mapper.cpu_write(0x4016, 0x04);
        assert_eq!(mapper.cpu_read(0x8000), Some(0x20));
    }

    #[test]
    fn standard_thirty_two_kib_prg_remains_fixed_during_chr_switching() {
        let mut prg = vec![0; 0x8000];
        prg[0] = 0x44;
        let mut mapper = Mapper99::new(prg, vec![0; 0x4000], 0x2000, None).unwrap();

        mapper.cpu_write(0x4016, 0x04);
        assert_eq!(mapper.cpu_read(0x8000), Some(0x44));
    }

    #[test]
    fn uses_fixed_four_screen_nametables() {
        let mapper = Mapper99::new(vec![0; 0x8000], vec![0; 0x4000], 0x2000, None).unwrap();
        assert_eq!(mapper.mirroring(), Some(Mirroring::FourScreen));
    }
}
