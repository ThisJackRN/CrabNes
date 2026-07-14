mod axrom;
mod cnrom;
mod fme7;
mod ines;
mod mapper;
mod mmc1;
mod mmc2;
mod mmc3;
mod mmc5;
mod n163;
mod nrom;
mod uxrom;
mod vrc;
mod vrc6;
mod vrc7;

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use axrom::Axrom;
use cnrom::Cnrom;
use fme7::Fme7;
pub use ines::{InesHeader, RomFormat, TimingMode};
use mapper::{Mapper, MapperSnapshot};
use mmc1::Mmc1;
use mmc2::Mmc2;
use mmc3::Mmc3;
use mmc5::Mmc5;
use n163::N163;
use nrom::Nrom;
use uxrom::Uxrom;
use vrc::Vrc;
use vrc6::Vrc6;
use vrc7::Vrc7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mirroring {
    Horizontal,
    Vertical,
    FourScreen,
    SingleScreenLower,
    SingleScreenUpper,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CartridgeError {
    FileTooSmall,
    InvalidMagic,
    RomSizeOverflow,
    Truncated {
        expected: usize,
        actual: usize,
    },
    UnsupportedConsoleType(u8),
    UnsupportedTiming(TimingMode),
    UnsupportedMapper(u16),
    UnsupportedSubmapper {
        mapper: u16,
        submapper: u8,
    },
    InvalidNromPrgSize(usize),
    InvalidMapperRomSize {
        mapper: u16,
        kind: &'static str,
        size: usize,
    },
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
            Self::InvalidMapperRomSize { mapper, kind, size } => {
                write!(
                    f,
                    "mapper {mapper} has an invalid {kind} size of {size} bytes"
                )
            }
        }
    }
}

impl Error for CartridgeError {}

pub struct Cartridge {
    mapper: Box<dyn Mapper>,
    mapper_id: u16,
    header_mirroring: Mirroring,
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
            1 => Box::new(Mmc1::new(
                parsed.prg,
                parsed.chr,
                header.prg_ram_bytes + header.prg_nvram_bytes,
                parsed.trainer.as_deref(),
            )?),
            2 => Box::new(Uxrom::new(
                parsed.prg,
                parsed.chr,
                header.prg_ram_bytes + header.prg_nvram_bytes,
                parsed.trainer.as_deref(),
            )?),
            3 => Box::new(Cnrom::new(
                parsed.prg,
                parsed.chr,
                header.prg_ram_bytes + header.prg_nvram_bytes,
                parsed.trainer.as_deref(),
            )?),
            4 => Box::new(Mmc3::new(
                parsed.prg,
                parsed.chr,
                header.prg_ram_bytes + header.prg_nvram_bytes,
                header.mirroring,
                parsed.trainer.as_deref(),
            )?),
            5 => Box::new(Mmc5::new(
                parsed.prg,
                parsed.chr,
                header.prg_ram_bytes + header.prg_nvram_bytes,
                parsed.trainer.as_deref(),
            )?),
            7 => Box::new(Axrom::new(
                parsed.prg,
                parsed.chr,
                header.prg_ram_bytes + header.prg_nvram_bytes,
                parsed.trainer.as_deref(),
            )?),
            9 | 10 => Box::new(Mmc2::new(
                header.mapper_id,
                parsed.prg,
                parsed.chr,
                header.prg_ram_bytes + header.prg_nvram_bytes,
                parsed.trainer.as_deref(),
            )?),
            19 => Box::new(N163::new(
                header.submapper,
                parsed.prg,
                parsed.chr,
                header.prg_ram_bytes + header.prg_nvram_bytes,
                parsed.trainer.as_deref(),
            )?),
            21 | 22 | 23 | 25 => Box::new(Vrc::new(
                header.mapper_id,
                header.submapper,
                parsed.prg,
                parsed.chr,
                header.prg_ram_bytes + header.prg_nvram_bytes,
                parsed.trainer.as_deref(),
            )?),
            24 | 26 => Box::new(Vrc6::new(
                header.mapper_id,
                parsed.prg,
                parsed.chr,
                header.prg_ram_bytes + header.prg_nvram_bytes,
                parsed.trainer.as_deref(),
            )?),
            69 => Box::new(Fme7::new(
                parsed.prg,
                parsed.chr,
                header.prg_ram_bytes + header.prg_nvram_bytes,
                parsed.trainer.as_deref(),
            )?),
            85 => Box::new(Vrc7::new(
                header.submapper,
                parsed.prg,
                parsed.chr,
                header.prg_ram_bytes + header.prg_nvram_bytes,
                parsed.trainer.as_deref(),
            )?),
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
            header_mirroring: header.mirroring,
            battery_backed: header.battery_backed,
        })
    }

    pub fn mapper_id(&self) -> u16 {
        self.mapper_id
    }
    pub fn mirroring(&self) -> Mirroring {
        self.mapper.mirroring().unwrap_or(self.header_mirroring)
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
    pub fn nametable_read(&mut self, address: u16) -> Option<u8> {
        self.mapper.nametable_read(address)
    }
    pub fn nametable_write(&mut self, address: u16, value: u8) -> bool {
        self.mapper.nametable_write(address, value)
    }
    pub fn nametable_ciram_index(&self, address: u16) -> Option<usize> {
        self.mapper.nametable_ciram_index(address)
    }
    pub fn clock_cpu(&mut self) {
        self.mapper.clock_cpu();
    }
    pub fn clock_scanline(&mut self, scanline: i16) {
        self.mapper.clock_scanline_at(scanline);
    }
    pub fn irq_pending(&self) -> bool {
        self.mapper.irq_pending()
    }
    pub fn expansion_audio(&self) -> f32 {
        self.mapper.expansion_audio()
    }
    pub fn reset(&mut self) {
        self.mapper.reset();
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

    fn mapper_rom(mapper: u16, prg_banks: u8, chr_banks: u8) -> Vec<u8> {
        let mut rom =
            vec![0; 16 + usize::from(prg_banks) * 0x4000 + usize::from(chr_banks) * 0x2000];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = prg_banks;
        rom[5] = chr_banks;
        rom[6] = (mapper as u8 & 0x0f) << 4;
        rom[7] = mapper as u8 & 0xf0;
        rom
    }

    #[test]
    fn loads_every_supported_mapper_id_from_an_ines_header() {
        let cases = [
            (0, 1, 1),
            (1, 2, 1),
            (2, 2, 0),
            (3, 2, 2),
            (4, 2, 1),
            (5, 2, 1),
            (7, 2, 0),
            (9, 4, 2),
            (10, 2, 2),
            (19, 4, 1),
            (21, 4, 1),
            (22, 4, 1),
            (23, 4, 1),
            (24, 4, 1),
            (25, 4, 1),
            (26, 4, 1),
            (69, 4, 1),
            (85, 4, 1),
        ];
        for (mapper, prg, chr) in cases {
            let cartridge = Cartridge::from_ines(&mapper_rom(mapper, prg, chr))
                .unwrap_or_else(|error| panic!("mapper {mapper} did not load: {error}"));
            assert_eq!(cartridge.mapper_id(), mapper);
        }
    }
}
