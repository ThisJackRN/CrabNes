mod ines;
mod mapper;
mod nrom;

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

pub use ines::{InesHeader, RomFormat, TimingMode};
use mapper::{Mapper, MapperSnapshot};
use nrom::Nrom;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mirroring {
    Horizontal,
    Vertical,
    FourScreen,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CartridgeError {
    FileTooSmall,
    InvalidMagic,
    RomSizeOverflow,
    Truncated { expected: usize, actual: usize },
    UnsupportedConsoleType(u8),
    UnsupportedTiming(TimingMode),
    UnsupportedMapper(u16),
    UnsupportedSubmapper { mapper: u16, submapper: u8 },
    InvalidNromPrgSize(usize),
}

impl fmt::Display for CartridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileTooSmall => write!(f, "file is smaller than the 16-byte iNES header"),
            Self::InvalidMagic => write!(f, "file does not start with the NES<EOF> signature"),
            Self::RomSizeOverflow => write!(f, "ROM sizes in the header are too large"),
            Self::Truncated { expected, actual } => {
                write!(
                    f,
                    "ROM is truncated: expected at least {expected} bytes, got {actual}"
                )
            }
            Self::UnsupportedConsoleType(kind) => {
                write!(f, "NES 2.0 console type {kind} is not supported yet")
            }
            Self::UnsupportedTiming(timing) => {
                write!(f, "{timing:?} timing is not supported yet")
            }
            Self::UnsupportedMapper(id) => write!(f, "mapper {id} is not supported yet"),
            Self::UnsupportedSubmapper { mapper, submapper } => {
                write!(
                    f,
                    "mapper {mapper} submapper {submapper} is not supported yet"
                )
            }
            Self::InvalidNromPrgSize(size) => {
                write!(f, "NROM PRG ROM must be 16 or 32 KiB, got {size} bytes")
            }
        }
    }
}

impl Error for CartridgeError {}

pub struct Cartridge {
    mapper: Box<dyn Mapper>,
    mapper_id: u16,
    mirroring: Mirroring,
    battery_backed: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CartridgeSnapshot {
    mapper_id: u16,
    mapper: MapperSnapshot,
}

impl Cartridge {
    pub fn from_ines(bytes: &[u8]) -> Result<Self, CartridgeError> {
        let parsed = ines::parse(bytes)?;
        let header = parsed.header;
        if header.console_type != 0 {
            return Err(CartridgeError::UnsupportedConsoleType(header.console_type));
        }
        if matches!(header.timing, TimingMode::Pal | TimingMode::Dendy) {
            return Err(CartridgeError::UnsupportedTiming(header.timing));
        }
        let mapper: Box<dyn Mapper> = match header.mapper_id {
            0 if header.submapper == 0 => {
                let mut nrom = Nrom::new(parsed.prg, parsed.chr)?;
                if let Some(trainer) = parsed.trainer {
                    nrom.load_trainer(&trainer);
                }
                Box::new(nrom)
            }
            mapper @ 0 => {
                return Err(CartridgeError::UnsupportedSubmapper {
                    mapper,
                    submapper: header.submapper,
                });
            }
            id => return Err(CartridgeError::UnsupportedMapper(id)),
        };

        Ok(Self {
            mapper,
            mapper_id: header.mapper_id,
            mirroring: header.mirroring,
            battery_backed: header.battery_backed,
        })
    }

    pub fn mapper_id(&self) -> u16 {
        self.mapper_id
    }
    pub fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
    pub fn has_battery(&self) -> bool {
        self.battery_backed
    }

    pub fn cpu_read(&mut self, address: u16) -> Option<u8> {
        self.mapper.cpu_read(address)
    }
    pub fn cpu_peek(&self, address: u16) -> Option<u8> {
        self.mapper.cpu_peek(address)
    }
    pub fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        self.mapper.cpu_write(address, value)
    }
    pub fn ppu_read(&mut self, address: u16) -> Option<u8> {
        self.mapper.ppu_read(address)
    }
    pub fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        self.mapper.ppu_write(address, value)
    }
    pub fn battery_ram(&self) -> Option<&[u8]> {
        self.mapper.battery_ram()
    }
    pub fn load_battery_ram(&mut self, data: &[u8]) {
        self.mapper.load_battery_ram(data);
    }

    pub(crate) fn snapshot(&self) -> CartridgeSnapshot {
        CartridgeSnapshot {
            mapper_id: self.mapper_id,
            mapper: self.mapper.snapshot(),
        }
    }

    pub(crate) fn restore_snapshot(&mut self, snapshot: &CartridgeSnapshot) -> bool {
        snapshot.mapper_id == self.mapper_id && self.mapper.restore_snapshot(&snapshot.mapper)
    }

    pub(crate) fn prg_rom(&self) -> &[u8] {
        self.mapper.prg_rom()
    }

    pub(crate) fn chr(&self) -> &[u8] {
        self.mapper.chr()
    }

    pub(crate) fn chr_is_writable(&self) -> bool {
        self.mapper.chr_is_writable()
    }

    pub(crate) fn debug_write_chr(&mut self, offset: usize, value: u8) -> bool {
        self.mapper.debug_write_chr(offset, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nes2_nrom(timing: u8) -> Vec<u8> {
        let mut rom = vec![0; 16 + 0x4000 + 0x2000];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 1;
        rom[5] = 1;
        rom[7] = 0x08;
        rom[12] = timing;
        rom
    }

    #[test]
    fn loads_ntsc_and_multi_region_nes2_nrom() {
        assert!(Cartridge::from_ines(&nes2_nrom(0)).is_ok());
        assert!(Cartridge::from_ines(&nes2_nrom(2)).is_ok());
    }

    #[test]
    fn rejects_pal_nes2_until_pal_timing_is_implemented() {
        assert!(matches!(
            Cartridge::from_ines(&nes2_nrom(1)),
            Err(CartridgeError::UnsupportedTiming(TimingMode::Pal))
        ));
    }

    #[test]
    fn loads_trainer_bytes_at_cpu_7000() {
        let mut rom = vec![0; 16 + 512 + 0x4000];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 1;
        rom[6] = 0x04;
        rom[16] = 0x5a;
        let mut cartridge = Cartridge::from_ines(&rom).unwrap();
        assert_eq!(cartridge.cpu_read(0x7000), Some(0x5a));
    }
}
