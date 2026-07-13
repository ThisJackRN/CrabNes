use super::{CartridgeError, Mirroring};

const HEADER_SIZE: usize = 16;
const TRAINER_SIZE: usize = 512;
const PRG_BANK_SIZE: usize = 16 * 1024;
const CHR_BANK_SIZE: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InesHeader {
    pub mapper_id: u16,
    pub mirroring: Mirroring,
    pub battery_backed: bool,
    pub trainer: bool,
    pub prg_rom_bytes: usize,
    pub chr_rom_bytes: usize,
}

pub(super) fn parse(bytes: &[u8]) -> Result<(InesHeader, Vec<u8>, Vec<u8>), CartridgeError> {
    if bytes.len() < HEADER_SIZE {
        return Err(CartridgeError::FileTooSmall);
    }
    if &bytes[0..4] != b"NES\x1a" {
        return Err(CartridgeError::InvalidMagic);
    }
    if bytes[7] & 0x0c == 0x08 {
        return Err(CartridgeError::Nes2Unsupported);
    }

    let flags6 = bytes[6];
    let flags7 = bytes[7];
    let prg_len = bytes[4] as usize * PRG_BANK_SIZE;
    let chr_len = bytes[5] as usize * CHR_BANK_SIZE;
    let trainer = flags6 & 0x04 != 0;
    let start = HEADER_SIZE + if trainer { TRAINER_SIZE } else { 0 };
    let expected = start + prg_len + chr_len;
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
        mapper_id: ((flags7 & 0xf0) | (flags6 >> 4)) as u16,
        mirroring,
        battery_backed: flags6 & 0x02 != 0,
        trainer,
        prg_rom_bytes: prg_len,
        chr_rom_bytes: chr_len,
    };
    let prg = bytes[start..start + prg_len].to_vec();
    let chr = bytes[start + prg_len..expected].to_vec();
    Ok((header, prg, chr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mapper_and_vertical_mirroring() {
        let mut rom = vec![0; 16 + PRG_BANK_SIZE + CHR_BANK_SIZE];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 1;
        rom[5] = 1;
        rom[6] = 0x21;
        rom[7] = 0x40;
        let (header, _, _) = parse(&rom).unwrap();
        assert_eq!(header.mapper_id, 0x42);
        assert_eq!(header.mirroring, Mirroring::Vertical);
    }
}
