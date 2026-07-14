use std::{
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use nes_core::OutputPalette;
use sha2::{Digest, Sha256};

use crate::persistence;

const RGB_PALETTE_BYTES: usize = 64 * 3;
const EMPHASIS_PALETTE_BYTES: usize = RGB_PALETTE_BYTES * 8;
const MAX_PALETTE_FILE_BYTES: u64 = 64 * 1024;

#[derive(Debug)]
pub struct LoadedPalette {
    pub colors: OutputPalette,
    pub warning: Option<String>,
}

#[derive(Debug)]
pub enum PaletteError {
    Io(io::Error),
    Invalid(String),
}

impl fmt::Display for PaletteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl Error for PaletteError {}

impl From<io::Error> for PaletteError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn palette_directory() -> PathBuf {
    persistence::app_directory().join("palettes")
}

pub fn load(path: &Path) -> Result<LoadedPalette, PaletteError> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > MAX_PALETTE_FILE_BYTES {
        return Err(PaletteError::Invalid(
            "palette file is larger than the 64 KiB safety limit".into(),
        ));
    }
    parse_bytes(&fs::read(path)?)
}

/// Validate and copy a palette into app-owned storage as a normalized 192-byte
/// RGB file. The stored copy remains available if the original is moved.
pub fn import(path: &Path) -> Result<(PathBuf, LoadedPalette), PaletteError> {
    let loaded = load(path)?;
    let bytes = flatten(&loaded.colors);
    let digest = Sha256::digest(&bytes);
    let suffix = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let destination = palette_directory().join(format!("custom-{suffix}.pal"));
    persistence::atomic_write(&destination, &bytes)?;
    Ok((destination, loaded))
}

fn parse_bytes(bytes: &[u8]) -> Result<LoadedPalette, PaletteError> {
    if bytes.len() == RGB_PALETTE_BYTES || bytes.len() == EMPHASIS_PALETTE_BYTES {
        let mut colors = [[0; 3]; 64];
        for (color, rgb) in colors.iter_mut().zip(bytes.chunks_exact(3)) {
            color.copy_from_slice(rgb);
        }
        return Ok(LoadedPalette {
            colors,
            warning: (bytes.len() == EMPHASIS_PALETTE_BYTES).then(|| {
                "Imported the base 64-color table; this emulator does not yet use the seven additional emphasis tables".into()
            }),
        });
    }

    let text = std::str::from_utf8(bytes).map_err(|_| {
        PaletteError::Invalid(
            "expected a 192-byte RGB .pal file, a 1536-byte emphasis palette, or UTF-8 text".into(),
        )
    })?;
    parse_text(text)
}

fn parse_text(text: &str) -> Result<LoadedPalette, PaletteError> {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let first = lines.next().unwrap_or_default();
    let mut source = Vec::new();
    if first.eq_ignore_ascii_case("JASC-PAL") {
        let _version = lines.next();
        let count = lines
            .next()
            .unwrap_or_default()
            .parse::<usize>()
            .map_err(|_| PaletteError::Invalid("JASC palette has an invalid color count".into()))?;
        if count != 64 {
            return Err(PaletteError::Invalid(format!(
                "NES palettes require 64 colors; this JASC palette declares {count}"
            )));
        }
        source.extend(lines);
    } else {
        source.push(first);
        source.extend(lines);
    }

    let mut parsed = Vec::new();
    for raw_line in source {
        let line = raw_line.split(';').next().unwrap_or_default().trim();
        if line.is_empty() || (line.starts_with('#') && !is_hex_color(line)) {
            continue;
        }
        let parts = line
            .split(|character: char| character.is_ascii_whitespace() || character == ',')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let color = match parts.as_slice() {
            [hex] if is_hex_color(hex) => parse_hex_color(hex)?,
            [red, green, blue] => [
                parse_component(red)?,
                parse_component(green)?,
                parse_component(blue)?,
            ],
            _ => {
                return Err(PaletteError::Invalid(format!(
                    "invalid palette line: {raw_line}"
                )));
            }
        };
        parsed.push(color);
    }
    if parsed.len() != 64 {
        return Err(PaletteError::Invalid(format!(
            "NES palettes require exactly 64 colors; found {}",
            parsed.len()
        )));
    }
    let colors: OutputPalette = parsed
        .try_into()
        .map_err(|_| PaletteError::Invalid("could not construct the 64-color palette".into()))?;
    Ok(LoadedPalette {
        colors,
        warning: None,
    })
}

fn is_hex_color(value: &str) -> bool {
    let value = value
        .strip_prefix('#')
        .or_else(|| value.strip_prefix("0x"))
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn parse_hex_color(value: &str) -> Result<[u8; 3], PaletteError> {
    let value = value
        .strip_prefix('#')
        .or_else(|| value.strip_prefix("0x"))
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    let packed = u32::from_str_radix(value, 16)
        .map_err(|_| PaletteError::Invalid(format!("invalid RGB color: {value}")))?;
    Ok([(packed >> 16) as u8, (packed >> 8) as u8, packed as u8])
}

fn parse_component(value: &str) -> Result<u8, PaletteError> {
    let (digits, radix) = if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        (hex, 16)
    } else if value.bytes().any(|byte| byte.is_ascii_alphabetic()) {
        (value, 16)
    } else {
        (value, 10)
    };
    u8::from_str_radix(digits, radix)
        .map_err(|_| PaletteError::Invalid(format!("invalid RGB component: {value}")))
}

fn flatten(colors: &OutputPalette) -> Vec<u8> {
    colors.iter().flatten().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_binary_rgb_palette() {
        let bytes = (0..RGB_PALETTE_BYTES)
            .map(|index| index as u8)
            .collect::<Vec<_>>();
        let palette = parse_bytes(&bytes).unwrap();
        assert_eq!(palette.colors[0], [0, 1, 2]);
        assert_eq!(palette.colors[63], [189, 190, 191]);
    }

    #[test]
    fn parses_hex_and_decimal_text_palette() {
        let text = (0..64)
            .map(|index| {
                if index == 0 {
                    "#123456".to_owned()
                } else {
                    format!("{index} 0 255")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let palette = parse_text(&text).unwrap();
        assert_eq!(palette.colors[0], [0x12, 0x34, 0x56]);
        assert_eq!(palette.colors[63], [63, 0, 255]);
    }

    #[test]
    fn rejects_wrong_color_count() {
        let error = parse_text("#000000\n#ffffff").unwrap_err();
        assert!(error.to_string().contains("exactly 64"));
    }
}
