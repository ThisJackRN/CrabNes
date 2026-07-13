mod ines;
mod mapper;
mod nrom;

use std::{error::Error, fmt};

pub use ines::InesHeader;
use mapper::Mapper;
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
    Nes2Unsupported,
    Truncated { expected: usize, actual: usize },
    UnsupportedMapper(u16),
    InvalidNromPrgSize(usize),
}

impl fmt::Display for CartridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileTooSmall => write!(f, "file is smaller than the 16-byte iNES header"),
            Self::InvalidMagic => write!(f, "file does not start with the NES<EOF> signature"),
            Self::Nes2Unsupported => write!(f, "NES 2.0 ROMs are not supported yet"),
            Self::Truncated { expected, actual } => {
                write!(
                    f,
                    "ROM is truncated: expected at least {expected} bytes, got {actual}"
                )
            }
            Self::UnsupportedMapper(id) => write!(f, "mapper {id} is not supported yet"),
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

impl Cartridge {
    pub fn from_ines(bytes: &[u8]) -> Result<Self, CartridgeError> {
        let (header, prg, chr) = ines::parse(bytes)?;
        let mapper: Box<dyn Mapper> = match header.mapper_id {
            0 => Box::new(Nrom::new(prg, chr)?),
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
}
