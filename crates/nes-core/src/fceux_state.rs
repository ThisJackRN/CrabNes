//! Reader for the chunked FCS save states embedded in text FM2 movies.

use std::{
    collections::HashMap,
    error::Error,
    fmt,
    io::{Cursor, Read},
};

use flate2::read::ZlibDecoder;

const HEADER_LEN: usize = 16;
const MAX_STATE_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FceuxStateError {
    InvalidHeader,
    UnsupportedVersion(u32),
    TooLarge,
    Truncated,
    InvalidCompression(String),
    InvalidChunks,
    MissingChunk {
        section: u8,
        name: &'static str,
    },
    InvalidChunkSize {
        section: u8,
        name: &'static str,
        expected: usize,
        actual: usize,
    },
    UnsupportedMapper(u16),
}

impl fmt::Display for FceuxStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHeader => f.write_str("not an FCEUX FCS save state"),
            Self::UnsupportedVersion(version) => {
                write!(
                    f,
                    "FCEUX state version {version} predates supported chunked FCS states"
                )
            }
            Self::TooLarge => f.write_str("FCEUX state exceeds the 128 MiB safety limit"),
            Self::Truncated => f.write_str("FCEUX state is truncated"),
            Self::InvalidCompression(error) => {
                write!(f, "invalid FCEUX state compression: {error}")
            }
            Self::InvalidChunks => f.write_str("invalid FCEUX state chunk table"),
            Self::MissingChunk { section, name } => {
                write!(f, "FCEUX state section {section} is missing {name}")
            }
            Self::InvalidChunkSize {
                section,
                name,
                expected,
                actual,
            } => write!(
                f,
                "FCEUX state {name} in section {section} is {actual} bytes; expected {expected}",
            ),
            Self::UnsupportedMapper(mapper) => write!(
                f,
                "embedded FCEUX state import currently supports MMC3 (mapper 4), not mapper {mapper}",
            ),
        }
    }
}

impl Error for FceuxStateError {}

#[derive(Debug)]
pub(crate) struct FceuxState {
    chunks: HashMap<(u8, [u8; 4]), Vec<u8>>,
}

pub(crate) struct FceuxMmc3State {
    pub bank_select: u8,
    pub banks: [u8; 8],
    pub mirroring: u8,
    pub ram_control: u8,
    pub irq_reload: bool,
    pub irq_counter: u8,
    pub irq_latch: u8,
    pub irq_enabled: bool,
    pub irq_pending: bool,
    pub prg_ram: Vec<u8>,
}

impl FceuxState {
    pub fn parse(bytes: &[u8]) -> Result<Self, FceuxStateError> {
        if bytes.len() < HEADER_LEN || &bytes[..4] != b"FCSX" {
            return Err(FceuxStateError::InvalidHeader);
        }
        let inflated_len = le_u32(&bytes[4..8]) as usize;
        let version = le_u32(&bytes[8..12]);
        if version < 9_500 {
            return Err(FceuxStateError::UnsupportedVersion(version));
        }
        if inflated_len > MAX_STATE_BYTES {
            return Err(FceuxStateError::TooLarge);
        }
        let compressed_len = le_u32(&bytes[12..16]);
        let payload = if compressed_len == u32::MAX {
            let payload = bytes.get(HEADER_LEN..).ok_or(FceuxStateError::Truncated)?;
            if payload.len() != inflated_len {
                return Err(FceuxStateError::Truncated);
            }
            payload.to_vec()
        } else {
            let compressed_len = compressed_len as usize;
            let end = HEADER_LEN
                .checked_add(compressed_len)
                .ok_or(FceuxStateError::TooLarge)?;
            let compressed = bytes
                .get(HEADER_LEN..end)
                .ok_or(FceuxStateError::Truncated)?;
            let mut decoder = ZlibDecoder::new(Cursor::new(compressed));
            let mut payload = Vec::with_capacity(inflated_len);
            decoder
                .by_ref()
                .take(MAX_STATE_BYTES as u64 + 1)
                .read_to_end(&mut payload)
                .map_err(|error| FceuxStateError::InvalidCompression(error.to_string()))?;
            if payload.len() > MAX_STATE_BYTES {
                return Err(FceuxStateError::TooLarge);
            }
            if payload.len() != inflated_len {
                return Err(FceuxStateError::Truncated);
            }
            payload
        };

        let mut chunks = HashMap::new();
        let mut offset = 0usize;
        while offset < payload.len() {
            let header = payload
                .get(offset..offset + 5)
                .ok_or(FceuxStateError::InvalidChunks)?;
            let section = header[0];
            let section_len = le_u32(&header[1..5]) as usize;
            let start = offset + 5;
            let end = start
                .checked_add(section_len)
                .filter(|&end| end <= payload.len())
                .ok_or(FceuxStateError::InvalidChunks)?;
            // Type 8 is a raw 256x256 backbuffer, not a table of named chunks.
            if section != 8 {
                let mut cursor = start;
                while cursor < end {
                    let chunk_header = payload
                        .get(cursor..cursor + 8)
                        .ok_or(FceuxStateError::InvalidChunks)?;
                    let name: [u8; 4] = chunk_header[..4].try_into().unwrap();
                    let len = le_u32(&chunk_header[4..8]) as usize;
                    let data_start = cursor + 8;
                    let data_end = data_start
                        .checked_add(len)
                        .filter(|&data_end| data_end <= end)
                        .ok_or(FceuxStateError::InvalidChunks)?;
                    if chunks
                        .insert((section, name), payload[data_start..data_end].to_vec())
                        .is_some()
                    {
                        return Err(FceuxStateError::InvalidChunks);
                    }
                    cursor = data_end;
                }
            }
            offset = end;
        }
        Ok(Self { chunks })
    }

    pub(crate) fn optional(&self, section: u8, name: &[u8; 4]) -> Option<&[u8]> {
        self.chunks.get(&(section, *name)).map(Vec::as_slice)
    }

    pub(crate) fn required(
        &self,
        section: u8,
        name: &'static [u8; 4],
        expected: usize,
    ) -> Result<&[u8], FceuxStateError> {
        let data = self
            .optional(section, name)
            .ok_or(FceuxStateError::MissingChunk {
                section,
                name: printable_name(name),
            })?;
        if data.len() != expected {
            return Err(FceuxStateError::InvalidChunkSize {
                section,
                name: printable_name(name),
                expected,
                actual: data.len(),
            });
        }
        Ok(data)
    }

    pub(crate) fn byte(&self, section: u8, name: &'static [u8; 4]) -> Result<u8, FceuxStateError> {
        Ok(self.required(section, name, 1)?[0])
    }

    pub(crate) fn word(&self, section: u8, name: &'static [u8; 4]) -> Result<u16, FceuxStateError> {
        Ok(u16::from_le_bytes(
            self.required(section, name, 2)?.try_into().unwrap(),
        ))
    }
}

const fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

const fn printable_name(name: &'static [u8; 4]) -> &'static str {
    match name {
        b"PC\0\0" => "PC",
        b"A\0\0\0" => "A",
        b"X\0\0\0" => "X",
        b"Y\0\0\0" => "Y",
        b"S\0\0\0" => "S",
        b"P\0\0\0" => "P",
        b"DB\0\0" => "DB",
        b"RAM\0" => "RAM",
        b"PSG\0" => "PSG",
        b"CMD\0" => "CMD",
        _ => "required chunk",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uncompressed_chunked_state() {
        let mut payload = Vec::new();
        payload.push(1);
        payload.extend_from_slice(&9u32.to_le_bytes());
        payload.extend_from_slice(b"A\0\0\0");
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.push(0x42);
        let mut state = Vec::new();
        state.extend_from_slice(b"FCSX");
        state.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        state.extend_from_slice(&22_020u32.to_le_bytes());
        state.extend_from_slice(&u32::MAX.to_le_bytes());
        state.extend_from_slice(&payload);
        let parsed = FceuxState::parse(&state).unwrap();
        assert_eq!(parsed.byte(1, b"A\0\0\0").unwrap(), 0x42);
    }
}
