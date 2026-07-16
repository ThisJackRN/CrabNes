use std::{
    fmt, fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use md5::{Digest as _, Md5};

// External movie support is an independent interoperability implementation.
// The FM2 parser follows FCEUX's public format documentation; no FCEUX or
// BizHawk source code is incorporated. See THIRD_PARTY_NOTICES.md.

use zip::ZipArchive;

use crate::tas::{self, TasFrame, TasMarker, TasMovie, TasStartType};

const MAX_IMPORT_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlFormat {
    FceuxFm2,
    BizHawkBk2,
    BizHawkInputLog,
    NativeTas,
}

impl fmt::Display for ControlFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::FceuxFm2 => "FCEUX FM2",
            Self::BizHawkBk2 => "BizHawk BK2",
            Self::BizHawkInputLog => "BizHawk Input Log",
            Self::NativeTas => "CrabNes TAS",
        })
    }
}

#[derive(Clone, Debug)]
pub struct ControlEvent {
    pub frame: usize,
    pub description: String,
}

#[derive(Clone, Debug)]
pub struct ControlMovie {
    pub source_path: PathBuf,
    pub format: ControlFormat,
    pub frames: Vec<TasFrame>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub rerecord_count: u64,
    pub suggested_start: TasStartType,
    pub warnings: Vec<String>,
    pub events: Vec<ControlEvent>,
    pub fceux_rom_md5: Option<[u8; 16]>,
    pub embedded_fceux_state: Option<Vec<u8>>,
}

impl ControlMovie {
    pub fn verify_fceux_rom(&self, rom: &[u8]) -> Result<(), String> {
        let Some(expected) = self.fceux_rom_md5 else {
            return Ok(());
        };
        let actual: [u8; 16] = Md5::digest(ines_rom_payload(rom)?).into();
        if actual != expected {
            return Err(format!(
                "FM2 ROM checksum mismatch: movie expects {}, loaded ROM is {}",
                hex_bytes(&expected),
                hex_bytes(&actual)
            ));
        }
        Ok(())
    }

    pub fn to_native_movie(
        &self,
        rom_sha256: String,
        start_type: TasStartType,
        starting_state: Option<Vec<u8>>,
    ) -> TasMovie {
        let mut movie = TasMovie::new(rom_sha256, start_type, starting_state);
        movie.frames = self.frames.clone();
        movie.author = self.author.clone();
        movie.description = Some(match &self.description {
            Some(description) => format!(
                "Converted from {} ({})\n{description}",
                self.format,
                self.source_path.display()
            ),
            None => format!(
                "Converted from {} ({})",
                self.format,
                self.source_path.display()
            ),
        });
        movie.rerecord_count = self.rerecord_count;
        movie.markers = self
            .events
            .iter()
            .map(|event| TasMarker {
                frame: event.frame.min(movie.frames.len()),
                label: format!("External event (not applied): {}", event.description),
            })
            .collect();
        movie
    }
}

pub fn load(path: &Path, expected_rom_sha256: Option<&str>) -> Result<ControlMovie, String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "fm2" => parse_fm2(&read_text_file(path)?, path.to_path_buf()),
        "bk2" => parse_bk2(path),
        "tas" => {
            let expected = expected_rom_sha256.ok_or_else(|| {
                "load the matching ROM before viewing a native .tas file".to_owned()
            })?;
            let loaded = tas::load(path, expected).map_err(|error| error.to_string())?;
            Ok(ControlMovie {
                source_path: path.to_path_buf(),
                format: ControlFormat::NativeTas,
                frames: loaded.movie.frames,
                author: loaded.movie.author,
                description: loaded.movie.description,
                rerecord_count: loaded.movie.rerecord_count,
                suggested_start: loaded.movie.start_type,
                warnings: loaded.warnings,
                events: Vec::new(),
                fceux_rom_md5: None,
                embedded_fceux_state: None,
            })
        }
        "txt" | "log" => {
            let text = read_text_file(path)?;
            if text.trim_start().starts_with("[Input]") {
                parse_bizhawk_input_log(&text, path.to_path_buf(), ControlFormat::BizHawkInputLog)
            } else if text.lines().any(|line| line.starts_with("version ")) {
                parse_fm2(&text, path.to_path_buf())
            } else {
                Err("unrecognized text movie; expected FM2 or BizHawk [Input] log".into())
            }
        }
        _ => Err(
            "supported control-view formats are .fm2, .bk2, extracted .txt/.log, and .tas".into(),
        ),
    }
}

fn read_text_file(path: &Path) -> Result<String, String> {
    let size = fs::metadata(path).map_err(|error| error.to_string())?.len();
    if size > MAX_IMPORT_BYTES {
        return Err(format!("movie is too large ({size} bytes)"));
    }
    fs::read_to_string(path).map_err(|error| format!("movie is not valid UTF-8 text: {error}"))
}

fn parse_bk2(path: &Path) -> Result<ControlMovie, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_IMPORT_BYTES {
        return Err("BK2 archive is too large".into());
    }
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let mut input_index = None;
    let mut header_index = None;
    for index in 0..archive.len() {
        let name = archive
            .by_index(index)
            .map_err(|error| error.to_string())?
            .name()
            .replace('\\', "/")
            .to_ascii_lowercase();
        if name.ends_with("input log.txt") {
            input_index = Some(index);
        } else if name.ends_with("header.txt") {
            header_index = Some(index);
        }
    }
    let input_index = input_index.ok_or_else(|| "BK2 has no Input Log.txt entry".to_owned())?;
    let input = read_zip_text(&mut archive, input_index)?;
    let header = header_index
        .map(|index| read_zip_text(&mut archive, index))
        .transpose()?
        .unwrap_or_default();
    let mut movie = parse_bizhawk_input_log(&input, path.to_path_buf(), ControlFormat::BizHawkBk2)?;
    let mut platform = None;
    for line in header.lines() {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key.to_ascii_lowercase().as_str() {
            "platform" => platform = nonempty(value),
            "author" => movie.author = nonempty(value),
            "rerecords" | "rerecordcount" => {
                movie.rerecord_count = value.trim().parse().unwrap_or(0)
            }
            "startsfromsavestate" if value.trim().eq_ignore_ascii_case("true") => {
                movie.suggested_start = TasStartType::SaveState;
                movie.warnings.push(
                    "BK2 starts from a BizHawk savestate, which is not compatible with this emulator"
                        .into(),
                );
            }
            _ => {}
        }
    }
    if platform
        .as_deref()
        .is_some_and(|platform| !platform.eq_ignore_ascii_case("NES"))
    {
        return Err(format!(
            "BK2 platform {} is not NES",
            platform.unwrap_or_default()
        ));
    }
    movie.warnings.push(
        "BK2 ROM identity is not cross-checked; load the same game and revision used by BizHawk"
            .into(),
    );
    Ok(movie)
}

fn read_zip_text(
    archive: &mut ZipArchive<Cursor<Vec<u8>>>,
    index: usize,
) -> Result<String, String> {
    let entry = archive.by_index(index).map_err(|error| error.to_string())?;
    if entry.size() > MAX_IMPORT_BYTES {
        return Err(format!("archive entry {} is too large", entry.name()));
    }
    let name = entry.name().to_owned();
    let mut text = String::new();
    entry
        .take(MAX_IMPORT_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("{name} is not valid UTF-8 text: {error}"))?;
    Ok(text)
}

fn parse_fm2(text: &str, source_path: PathBuf) -> Result<ControlMovie, String> {
    let mut frames = Vec::new();
    let mut warnings = Vec::new();
    let mut events = Vec::new();
    let mut author = None;
    let mut description_lines = Vec::new();
    let mut rerecord_count = 0;
    let mut suggested_start = TasStartType::PowerOn;
    let mut binary = false;
    let mut pal = false;
    let mut fourscore = false;
    let mut unsupported_device = false;
    let mut declared_length = None;
    let mut version = None;
    let mut fceux_rom_md5 = None;
    let mut embedded_fceux_state = None;

    for (line_number, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim_end_matches('\r');
        if line.starts_with('|') {
            if binary {
                return Err(
                    "binary FM2 input logs are not supported; save it as text FM2 in FCEUX".into(),
                );
            }
            if declared_length.is_some_and(|length| frames.len() >= length) {
                continue;
            }
            let fields: Vec<_> = line.split('|').collect();
            if fields.len() < 5 {
                return Err(format!(
                    "invalid FM2 input record on line {}",
                    line_number + 1
                ));
            }
            let command = fields[1]
                .trim()
                .parse::<u32>()
                .map_err(|_| format!("invalid FM2 command on line {}", line_number + 1))?;
            if command != 0 {
                add_command_events(&mut events, frames.len(), command, "FM2");
            }
            let player1 = parse_pad(fields[2], PadOrder::Fm2)
                .map_err(|error| format!("FM2 player 1 on line {}: {error}", line_number + 1))?;
            let player2 = parse_pad(fields[3], PadOrder::Fm2)
                .map_err(|error| format!("FM2 player 2 on line {}: {error}", line_number + 1))?;
            frames.push(TasFrame { player1, player2 });
            continue;
        }

        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "version" => version = value.trim().parse::<u32>().ok(),
            "binary" => binary = value.trim() == "1",
            "palFlag" => pal = value.trim() == "1",
            "fourscore" => fourscore = value.trim() == "1",
            "port0" | "port1" if !matches!(value.trim(), "0" | "1") => unsupported_device = true,
            "FDS" if value.trim() == "1" => warnings.push("FDS movies are not supported".into()),
            "rerecordCount" => rerecord_count = value.trim().parse().unwrap_or(0),
            "length" => declared_length = value.trim().parse::<usize>().ok(),
            "romChecksum" => {
                fceux_rom_md5 = Some(decode_fm2_md5(value.trim()).map_err(|error| {
                    format!(
                        "invalid FM2 romChecksum on line {}: {error}",
                        line_number + 1
                    )
                })?);
            }
            "savestate" => {
                suggested_start = TasStartType::SaveState;
                embedded_fceux_state = Some(decode_fm2_blob(value.trim()).map_err(|error| {
                    format!(
                        "invalid embedded FCEUX savestate on line {}: {error}",
                        line_number + 1
                    )
                })?);
            }
            "comment" => {
                let value = value.trim();
                if let Some(value) = value.strip_prefix("author ") {
                    author = nonempty(value);
                } else if !value.is_empty() {
                    description_lines.push(value.to_owned());
                }
            }
            _ => {}
        }
    }
    if binary {
        return Err("binary FM2 input logs are not supported; save it as text FM2 in FCEUX".into());
    }
    if frames.is_empty() {
        return Err("FM2 contains no text input records".into());
    }
    match version {
        Some(3) => {}
        Some(version) => return Err(format!("unsupported FM2 version {version}; expected 3")),
        None => return Err("FM2 header is missing version".into()),
    }
    if fceux_rom_md5.is_none() {
        warnings
            .push("FM2 has no usable ROM MD5; load the exact game revision used by FCEUX".into());
    }
    if embedded_fceux_state.is_some() {
        warnings.push(
            "Embedded FCEUX start state will be imported when selected; this bridge currently supports MMC3 (mapper 4) chunked FCS states"
                .into(),
        );
    }
    if pal {
        warnings.push(
            "FM2 uses PAL timing; load a PAL ROM/header or synchronization will differ".into(),
        );
    }
    if fourscore {
        warnings.push(
            "Four Score players 3 and 4 are ignored; only the first two controllers are converted"
                .into(),
        );
    }
    if unsupported_device {
        warnings.push("Zapper or another unsupported FM2 input device was declared".into());
    }
    if !events.is_empty() {
        warnings.push("Reset/power/FDS/coin commands are shown as events but are not applied by converted controller input".into());
    }
    Ok(ControlMovie {
        source_path,
        format: ControlFormat::FceuxFm2,
        frames,
        author,
        description: (!description_lines.is_empty()).then(|| description_lines.join("\n")),
        rerecord_count,
        suggested_start,
        warnings,
        events,
        fceux_rom_md5,
        embedded_fceux_state,
    })
}

fn parse_bizhawk_input_log(
    text: &str,
    source_path: PathBuf,
    format: ControlFormat,
) -> Result<ControlMovie, String> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("[Input]") {
        return Err("BizHawk input log is missing [Input]".into());
    }
    if !lines.next().is_some_and(|line| line.starts_with("LogKey:")) {
        return Err("BizHawk input log is missing LogKey".into());
    }
    let mut frames = Vec::new();
    let mut events = Vec::new();
    for (offset, raw_line) in lines.enumerate() {
        let line = raw_line.trim_end_matches('\r');
        if line == "[/Input]" {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split('|').collect();
        if fields.first() != Some(&"") || fields.len() < 4 {
            return Err(format!(
                "invalid BizHawk input record on log line {}",
                offset + 3
            ));
        }
        let command = fields[1];
        if command
            .chars()
            .any(|character| !matches!(character, '.' | ' '))
        {
            let mut value = 0;
            if command.contains('r') || command.contains('R') {
                value |= 1;
            }
            if command.contains('P') || command.contains('p') {
                value |= 2;
            }
            add_command_events(&mut events, frames.len(), value, "BizHawk");
        }
        let pads: Vec<_> = fields[2..]
            .iter()
            .filter(|field| field.len() == 8)
            .take(2)
            .map(|field| parse_pad(field, PadOrder::BizHawk))
            .collect::<Result<_, _>>()?;
        let player1 = pads
            .first()
            .copied()
            .ok_or_else(|| format!("BizHawk frame {} has no NES controller field", frames.len()))?;
        frames.push(TasFrame {
            player1,
            player2: pads.get(1).copied().unwrap_or(0),
        });
    }
    if frames.is_empty() {
        return Err("BizHawk input log contains no frames".into());
    }
    let mut warnings = Vec::new();
    if !events.is_empty() {
        warnings.push("Power/reset commands are shown as events but are not applied by converted controller input".into());
    }
    Ok(ControlMovie {
        source_path,
        format,
        frames,
        author: None,
        description: None,
        rerecord_count: 0,
        suggested_start: TasStartType::PowerOn,
        warnings,
        events,
        fceux_rom_md5: None,
        embedded_fceux_state: None,
    })
}

fn decode_fm2_md5(value: &str) -> Result<[u8; 16], String> {
    let bytes = decode_fm2_blob(value)?;
    bytes
        .try_into()
        .map_err(|bytes: Vec<u8>| format!("checksum decoded to {} bytes, expected 16", bytes.len()))
}

fn decode_fm2_blob(value: &str) -> Result<Vec<u8>, String> {
    let bytes = if let Some(value) = value.strip_prefix("base64:") {
        BASE64.decode(value).map_err(|error| error.to_string())?
    } else if let Some(value) = value.strip_prefix("0x") {
        if !value.len().is_multiple_of(2) {
            return Err("hex data has an odd number of digits".into());
        }
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let pair = std::str::from_utf8(pair).unwrap();
                u8::from_str_radix(pair, 16).map_err(|_| "hex data contains a non-hex digit")
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(str::to_owned)?
    } else {
        return Err("expected base64: or 0x encoding".into());
    };
    if bytes.len() as u64 > MAX_IMPORT_BYTES {
        return Err("decoded data is too large".into());
    }
    Ok(bytes)
}

fn ines_rom_payload(rom: &[u8]) -> Result<&[u8], String> {
    if rom.len() < 16 || &rom[..4] != b"NES\x1a" {
        return Err("loaded file is not an iNES ROM".into());
    }
    let trainer: usize = if rom[6] & 0x04 != 0 { 512 } else { 0 };
    let start = 16usize + trainer;
    let prg = usize::from(rom[4]) * 0x4000;
    let chr = usize::from(rom[5]) * 0x2000;
    let end = start
        .checked_add(prg)
        .and_then(|end| end.checked_add(chr))
        .ok_or_else(|| "ROM size overflows".to_owned())?;
    rom.get(start..end)
        .ok_or_else(|| "ROM is truncated relative to its iNES header".to_owned())
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Clone, Copy)]
enum PadOrder {
    Fm2,
    BizHawk,
}

fn parse_pad(field: &str, order: PadOrder) -> Result<u8, String> {
    if field.is_empty() {
        return Ok(0);
    }
    if field.len() != 8 {
        return Err(format!(
            "expected 8 controller columns, got {}",
            field.len()
        ));
    }
    let bits = match order {
        PadOrder::Fm2 => [7, 6, 5, 4, 3, 2, 1, 0],
        PadOrder::BizHawk => [4, 5, 6, 7, 3, 2, 1, 0],
    };
    let mut mask = 0;
    for (index, character) in field.chars().enumerate() {
        if !matches!(character, '.' | ' ') {
            mask |= 1 << bits[index];
        }
    }
    Ok(mask)
}

fn add_command_events(events: &mut Vec<ControlEvent>, frame: usize, command: u32, source: &str) {
    for (bit, description) in [
        (1, "soft reset"),
        (2, "power cycle"),
        (4, "disk insert"),
        (8, "disk select"),
        (16, "coin insert"),
    ] {
        if command & bit != 0 {
            events.push(ControlEvent {
                frame,
                description: format!("{source} {description}"),
            });
        }
    }
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;
    use zip::{ZipWriter, write::SimpleFileOptions};

    #[test]
    fn parses_fm2_controller_order_metadata_and_events() {
        let text = "version 3\nrerecordCount 7\ncomment author Jane\nport0 1\nport1 1\nport2 0\n|0|R......A|.L....B.||\n|1|....T...|.....S..||\n";
        let movie = parse_fm2(text, PathBuf::from("movie.fm2")).unwrap();
        assert_eq!(movie.frames.len(), 2);
        assert_eq!(movie.frames[0].player1, 0x81);
        assert_eq!(movie.frames[0].player2, 0x42);
        assert_eq!(movie.frames[1].player1, 0x08);
        assert_eq!(movie.frames[1].player2, 0x04);
        assert_eq!(movie.author.as_deref(), Some("Jane"));
        assert_eq!(movie.rerecord_count, 7);
        assert_eq!(movie.events.len(), 1);
    }

    #[test]
    fn retains_fm2_rom_checksum_and_embedded_state() {
        let text = "version 3\nromChecksum base64:AAECAwQFBgcICQoLDA0ODw==\nsavestate base64:RkNTWA==\n|0|........|........||\n";
        let movie = parse_fm2(text, PathBuf::from("state.fm2")).unwrap();
        assert_eq!(
            movie.fceux_rom_md5,
            Some([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])
        );
        assert_eq!(
            movie.embedded_fceux_state.as_deref(),
            Some(b"FCSX".as_slice())
        );
        assert_eq!(movie.suggested_start, TasStartType::SaveState);
    }

    #[test]
    fn fceux_rom_check_hashes_only_prg_and_chr_payload() {
        let mut rom = vec![0; 16 + 0x4000 + 0x2000];
        rom[..4].copy_from_slice(b"NES\x1a");
        rom[4] = 1;
        rom[5] = 1;
        for (index, byte) in rom[16..].iter_mut().enumerate() {
            *byte = index as u8;
        }
        let expected: [u8; 16] = Md5::digest(&rom[16..]).into();
        let mut movie = parse_fm2(
            "version 3\n|0|........|........||\n",
            PathBuf::from("movie.fm2"),
        )
        .unwrap();
        movie.fceux_rom_md5 = Some(expected);
        movie.verify_fceux_rom(&rom).unwrap();
        rom[16] ^= 1;
        assert!(movie.verify_fceux_rom(&rom).is_err());
    }

    #[test]
    fn parses_extracted_bizhawk_neshawk_input_log() {
        let text = "[Input]\nLogKey:#Reset|P1 Up|P1 Down|P1 Left|P1 Right|P1 Start|P1 Select|P1 B|P1 A|\n|..|U...S..A|.D...sB.|\n|..|........|........|\n[/Input]\n";
        let movie = parse_bizhawk_input_log(
            text,
            PathBuf::from("Input Log.txt"),
            ControlFormat::BizHawkInputLog,
        )
        .unwrap();
        assert_eq!(movie.frames[0].player1, 0x19);
        assert_eq!(movie.frames[0].player2, 0x26);
        assert_eq!(movie.frames[1], TasFrame::default());
    }

    #[test]
    fn reads_bizhawk_bk2_archive() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("nes-ui-control-view-{nonce}.bk2"));
        let file = fs::File::create(&path).unwrap();
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        archive.start_file("Header.txt", options).unwrap();
        archive
            .write_all(b"Platform NES\nAuthor Archive Tester\nRerecords 12\n")
            .unwrap();
        archive.start_file("Input Log.txt", options).unwrap();
        archive
            .write_all(
                b"[Input]\nLogKey:#Reset|P1 Up|P1 Down|P1 Left|P1 Right|P1 Start|P1 Select|P1 B|P1 A|\n|..|U...S..A|........|\n[/Input]\n",
            )
            .unwrap();
        archive.finish().unwrap();

        let movie = parse_bk2(&path).unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(movie.format, ControlFormat::BizHawkBk2);
        assert_eq!(movie.frames[0].player1, 0x19);
        assert_eq!(movie.author.as_deref(), Some("Archive Tester"));
        assert_eq!(movie.rerecord_count, 12);
    }

    #[test]
    fn rejects_binary_fm2_without_guessing() {
        let text = "version 3\nbinary 1\n";
        assert!(parse_fm2(text, PathBuf::from("binary.fm2")).is_err());
    }

    #[test]
    fn conversion_preserves_inputs_and_marks_unapplied_events() {
        let source = ControlMovie {
            source_path: PathBuf::from("movie.fm2"),
            format: ControlFormat::FceuxFm2,
            frames: vec![TasFrame {
                player1: 1,
                player2: 2,
            }],
            author: Some("A".into()),
            description: None,
            rerecord_count: 9,
            suggested_start: TasStartType::PowerOn,
            warnings: Vec::new(),
            events: vec![ControlEvent {
                frame: 0,
                description: "FM2 soft reset".into(),
            }],
            fceux_rom_md5: None,
            embedded_fceux_state: None,
        };
        let movie = source.to_native_movie("12".repeat(32), TasStartType::PowerOn, None);
        assert_eq!(movie.frames, source.frames);
        assert_eq!(movie.rerecord_count, 9);
        assert_eq!(movie.markers.len(), 1);
    }
}
