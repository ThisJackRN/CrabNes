use std::{error::Error, fmt};

const GAME_GENIE_ALPHABET: &str = "APZLGITYEOXUKSVN";

/// A CPU read substitution. Game Genie codes use `$8000-$FFFF`; raw patches
/// can target the complete CPU address space, including FDS program RAM.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cheat {
    pub address: u16,
    pub value: u8,
    pub compare: Option<u8>,
}

impl Cheat {
    pub const fn new(address: u16, value: u8, compare: Option<u8>) -> Self {
        Self {
            address,
            value,
            compare,
        }
    }

    pub fn parse(text: &str) -> Result<Self, CheatError> {
        let trimmed = text.trim();
        if trimmed.contains(':') {
            parse_raw(trimmed)
        } else {
            parse_game_genie(trimmed)
        }
    }

    pub const fn replacement(self, address: u16, actual: u8) -> Option<u8> {
        if self.address == address
            && match self.compare {
                Some(compare) => compare == actual,
                None => true,
            }
        {
            Some(self.value)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheatError(String);

impl CheatError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for CheatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CheatError {}

fn parse_game_genie(text: &str) -> Result<Cheat, CheatError> {
    let code: String = text
        .chars()
        .filter(|character| !character.is_ascii_whitespace() && *character != '-')
        .map(|character| character.to_ascii_uppercase())
        .collect();
    if !matches!(code.len(), 6 | 8) {
        return Err(CheatError::new(
            "NES Game Genie codes must contain 6 or 8 letters",
        ));
    }
    let mut nibbles = [0u8; 8];
    for (index, character) in code.chars().enumerate() {
        let Some(value) = GAME_GENIE_ALPHABET.find(character) else {
            return Err(CheatError::new(format!(
                "'{character}' is not a valid NES Game Genie letter"
            )));
        };
        nibbles[index] = value as u8;
    }

    let [n0, n1, n2, n3, n4, n5, n6, n7] = nibbles;
    let address = 0x8000
        | (u16::from(n3 & 7) << 12)
        | (u16::from(n5 & 7) << 8)
        | (u16::from(n4 & 8) << 8)
        | (u16::from(n2 & 7) << 4)
        | (u16::from(n1 & 8) << 4)
        | u16::from(n4 & 7)
        | u16::from(n3 & 8);
    let (value, compare) = if code.len() == 6 {
        (
            ((n1 & 7) << 4) | ((n0 & 8) << 4) | (n0 & 7) | (n5 & 8),
            None,
        )
    } else {
        (
            ((n1 & 7) << 4) | ((n0 & 8) << 4) | (n0 & 7) | (n7 & 8),
            Some(((n7 & 7) << 4) | ((n6 & 8) << 4) | (n6 & 7) | (n5 & 8)),
        )
    };
    Ok(Cheat::new(address, value, compare))
}

fn parse_raw(text: &str) -> Result<Cheat, CheatError> {
    let compact: String = text
        .chars()
        .filter(|character| !character.is_ascii_whitespace() && *character != '$')
        .collect();
    let (left, value) = compact
        .split_once(':')
        .ok_or_else(|| CheatError::new("raw cheats use ADDRESS:VALUE"))?;
    if value.contains(':') {
        return Err(CheatError::new("raw cheats contain exactly one ':'"));
    }
    let (address, compare) = match left.split_once('?') {
        Some((address, compare)) if !compare.contains('?') => (
            parse_hex_u16(address, "address")?,
            Some(parse_hex_u8(compare, "compare value")?),
        ),
        Some(_) => return Err(CheatError::new("raw cheats contain at most one '?'")),
        None => (parse_hex_u16(left, "address")?, None),
    };
    Ok(Cheat::new(
        address,
        parse_hex_u8(value, "replacement value")?,
        compare,
    ))
}

fn parse_hex_u16(text: &str, label: &str) -> Result<u16, CheatError> {
    if text.is_empty() || text.len() > 4 {
        return Err(CheatError::new(format!(
            "raw cheat {label} must be 1 to 4 hexadecimal digits"
        )));
    }
    u16::from_str_radix(text.trim_start_matches("0x"), 16)
        .map_err(|_| CheatError::new(format!("raw cheat {label} is not hexadecimal")))
}

fn parse_hex_u8(text: &str, label: &str) -> Result<u8, CheatError> {
    if text.is_empty() || text.len() > 2 {
        return Err(CheatError::new(format!(
            "raw cheat {label} must be 1 or 2 hexadecimal digits"
        )));
    }
    u8::from_str_radix(text.trim_start_matches("0x"), 16)
        .map_err(|_| CheatError::new(format!("raw cheat {label} is not hexadecimal")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_six_letter_game_genie_code() {
        assert_eq!(
            Cheat::parse("GOS-SIP").unwrap(),
            Cheat::new(0xd1dd, 0x14, None)
        );
    }

    #[test]
    fn decodes_eight_letter_game_genie_code_with_compare() {
        assert_eq!(
            Cheat::parse("APEETPEY").unwrap(),
            Cheat::new(0x810e, 0x10, Some(0xf0))
        );
    }

    #[test]
    fn parses_raw_codes_for_cartridge_or_fds_memory() {
        assert_eq!(
            Cheat::parse("$6000:EA").unwrap(),
            Cheat::new(0x6000, 0xea, None)
        );
        assert_eq!(
            Cheat::parse("810E?F0:10").unwrap(),
            Cheat::new(0x810e, 0x10, Some(0xf0))
        );
    }

    #[test]
    fn rejects_bad_codes() {
        assert!(Cheat::parse("ABC").is_err());
        assert!(Cheat::parse("6000:XYZ").is_err());
        assert!(Cheat::parse("6000?12?34:56").is_err());
    }
}
