use super::{CartridgeError, Mirroring};

const HEADER_SIZE: usize = 16;
const TRAINER_SIZE: usize = 512;
const PRG_BANK_SIZE: usize = 16 * 1024;
const CHR_BANK_SIZE: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RomFormat {
    INes1,
    Nes2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingMode {
    Ntsc,
    Pal,
    MultiRegion,
    Dendy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InesHeader {
    pub format: RomFormat,
    pub mapper_id: u16,
    pub submapper: u8,
    pub mirroring: Mirroring,
    pub battery_backed: bool,
    pub trainer: bool,
    pub console_type: u8,
    pub timing: TimingMode,
    pub prg_rom_bytes: usize,
    pub chr_rom_bytes: usize,
    pub prg_ram_bytes: usize,
    pub prg_nvram_bytes: usize,
    pub chr_ram_bytes: usize,
    pub chr_nvram_bytes: usize,
}

pub(super) struct ParsedInes {
    pub header: InesHeader,
    pub trainer: Option<Vec<u8>>,
    pub prg: Vec<u8>,
    pub chr: Vec<u8>,
}

pub(super) fn parse(bytes: &[u8]) -> Result<ParsedInes, CartridgeError> {
    if bytes.len() < HEADER_SIZE {
        return Err(CartridgeError::FileTooSmall);
    }
    if &bytes[0..4] != b"NES\x1a" {
        return Err(CartridgeError::InvalidMagic);
    }

    let flags6 = bytes[6];
    let flags7 = bytes[7];
    let format = if flags7 & 0x0c == 0x08 {
        RomFormat::Nes2
    } else {
        RomFormat::INes1
    };
    let battery_backed = flags6 & 0x02 != 0;
    let trainer_present = flags6 & 0x04 != 0;

    let (
        mapper_id,
        submapper,
        prg_len,
        chr_len,
        prg_ram_bytes,
        prg_nvram_bytes,
        chr_ram_bytes,
        chr_nvram_bytes,
        timing,
    ) = match format {
        RomFormat::INes1 => {
            let prg_len = usize::from(bytes[4]) * PRG_BANK_SIZE;
            let chr_len = usize::from(bytes[5]) * CHR_BANK_SIZE;
            let declared_prg_ram_banks = usize::from(bytes[8]).max(1);
            let declared_prg_ram = declared_prg_ram_banks * 8 * 1024;
            let (prg_ram, prg_nvram) = if battery_backed {
                (0, declared_prg_ram)
            } else {
                (declared_prg_ram, 0)
            };
            (
                u16::from((flags7 & 0xf0) | (flags6 >> 4)),
                0,
                prg_len,
                chr_len,
                prg_ram,
                prg_nvram,
                if chr_len == 0 { CHR_BANK_SIZE } else { 0 },
                0,
                if bytes[9] & 1 == 0 {
                    TimingMode::Ntsc
                } else {
                    TimingMode::Pal
                },
            )
        }
        RomFormat::Nes2 => {
            let mapper_id = u16::from(flags6 >> 4)
                | u16::from(flags7 & 0xf0)
                | (u16::from(bytes[8] & 0x0f) << 8);
            (
                mapper_id,
                bytes[8] >> 4,
                decode_nes2_rom_size(bytes[4], bytes[9] & 0x0f, PRG_BANK_SIZE)?,
                decode_nes2_rom_size(bytes[5], bytes[9] >> 4, CHR_BANK_SIZE)?,
                decode_ram_size(bytes[10] & 0x0f),
                decode_ram_size(bytes[10] >> 4),
                decode_ram_size(bytes[11] & 0x0f),
                decode_ram_size(bytes[11] >> 4),
                match bytes[12] & 0x03 {
                    0 => TimingMode::Ntsc,
                    1 => TimingMode::Pal,
                    2 => TimingMode::MultiRegion,
                    _ => TimingMode::Dendy,
                },
            )
        }
    };

    let trainer_start = HEADER_SIZE;
    let prg_start = trainer_start
        .checked_add(if trainer_present { TRAINER_SIZE } else { 0 })
        .ok_or(CartridgeError::RomSizeOverflow)?;
    let chr_start = prg_start
        .checked_add(prg_len)
        .ok_or(CartridgeError::RomSizeOverflow)?;
    let expected = chr_start
        .checked_add(chr_len)
        .ok_or(CartridgeError::RomSizeOverflow)?;
    if bytes.len() < expected {
        return Err(CartridgeError::Truncated {
            expected,
            actual: bytes.len(),
        });
    }

    let mirroring = if flags6 & 0x08 != 0 {
        Mirroring::FourScreen
    } else if flags6 & 0x01 != 0 {
        Mirroring::Vertical
    } else {
        Mirroring::Horizontal
    };
    let header = InesHeader {
        format,
        mapper_id,
        submapper,
        mirroring,
        battery_backed,
        trainer: trainer_present,
        console_type: flags7 & 0x03,
        timing,
        prg_rom_bytes: prg_len,
        chr_rom_bytes: chr_len,
        prg_ram_bytes,
        prg_nvram_bytes,
        chr_ram_bytes,
        chr_nvram_bytes,
    };
    Ok(ParsedInes {
        header,
        trainer: trainer_present.then(|| bytes[trainer_start..prg_start].to_vec()),
        prg: bytes[prg_start..chr_start].to_vec(),
        chr: bytes[chr_start..expected].to_vec(),
    })
}

fn decode_nes2_rom_size(lsb: u8, msb: u8, bank_size: usize) -> Result<usize, CartridgeError> {
    if msb != 0x0f {
        let banks = (usize::from(msb) << 8) | usize::from(lsb);
        return banks
            .checked_mul(bank_size)
            .ok_or(CartridgeError::RomSizeOverflow);
    }

    let exponent = u32::from(lsb >> 2);
    let multiplier = usize::from((lsb & 0x03) * 2 + 1);
    1_usize
        .checked_shl(exponent)
        .and_then(|base| base.checked_mul(multiplier))
        .ok_or(CartridgeError::RomSizeOverflow)
}

fn decode_ram_size(shift: u8) -> usize {
    if shift == 0 { 0 } else { 64_usize << shift }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ines_mapper_and_vertical_mirroring() {
        let mut rom = vec![0; 16 + PRG_BANK_SIZE + CHR_BANK_SIZE];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 1;
        rom[5] = 1;
        rom[6] = 0x21;
        rom[7] = 0x40;
        let parsed = parse(&rom).unwrap();
        assert_eq!(parsed.header.format, RomFormat::INes1);
        assert_eq!(parsed.header.mapper_id, 0x42);
        assert_eq!(parsed.header.mirroring, Mirroring::Vertical);
        assert_eq!(parsed.header.timing, TimingMode::Ntsc);
    }

    #[test]
    fn parses_nes2_extended_fields_and_linear_sizes() {
        let mut rom = vec![0; 16 + PRG_BANK_SIZE + CHR_BANK_SIZE];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 1;
        rom[5] = 1;
        rom[6] = 0x21;
        rom[7] = 0x48;
        rom[8] = 0xa3;
        rom[10] = 0x87;
        rom[11] = 0x70;
        rom[12] = 2;
        let parsed = parse(&rom).unwrap();
        assert_eq!(parsed.header.format, RomFormat::Nes2);
        assert_eq!(parsed.header.mapper_id, 0x342);
        assert_eq!(parsed.header.submapper, 0x0a);
        assert_eq!(parsed.header.prg_rom_bytes, PRG_BANK_SIZE);
        assert_eq!(parsed.header.chr_rom_bytes, CHR_BANK_SIZE);
        assert_eq!(parsed.header.prg_ram_bytes, 8 * 1024);
        assert_eq!(parsed.header.prg_nvram_bytes, 16 * 1024);
        assert_eq!(parsed.header.chr_ram_bytes, 0);
        assert_eq!(parsed.header.chr_nvram_bytes, 8 * 1024);
        assert_eq!(parsed.header.timing, TimingMode::MultiRegion);
    }

    #[test]
    fn parses_nes2_exponent_multiplier_sizes() {
        // 2^14 * 1 = 16 KiB PRG, 2^13 * 1 = 8 KiB CHR.
        let mut rom = vec![0; 16 + PRG_BANK_SIZE + CHR_BANK_SIZE];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 14 << 2;
        rom[5] = 13 << 2;
        rom[7] = 0x08;
        rom[9] = 0xff;
        let parsed = parse(&rom).unwrap();
        assert_eq!(parsed.header.prg_rom_bytes, PRG_BANK_SIZE);
        assert_eq!(parsed.header.chr_rom_bytes, CHR_BANK_SIZE);
    }

    #[test]
    fn extracts_a_trainer_before_prg_rom() {
        let mut rom = vec![0; 16 + TRAINER_SIZE + PRG_BANK_SIZE];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 1;
        rom[6] = 0x04;
        rom[16] = 0x5a;
        rom[16 + TRAINER_SIZE] = 0xa5;
        let parsed = parse(&rom).unwrap();
        assert_eq!(parsed.trainer.as_ref().unwrap()[0], 0x5a);
        assert_eq!(parsed.prg[0], 0xa5);
    }
}
