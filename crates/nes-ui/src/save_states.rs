use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Local};
use nes_core::{FRAME_HEIGHT, FRAME_WIDTH, Nes};

use crate::{persistence, settings};

const MAGIC: &[u8; 8] = b"MONESUI\0";
const FORMAT_VERSION: u32 = 1;
const HEADER_SIZE: usize = 40;

pub struct SlotInfo {
    pub created: u64,
    pub preview_rgb: Vec<u8>,
    pub path: PathBuf,
}

pub fn save_slot(nes: &Nes, slot: usize) -> io::Result<SlotInfo> {
    let state = nes.save_state().map_err(io::Error::other)?;
    let preview = nes.frame().pixels.clone();
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let path = slot_path(nes.rom_hash(), slot);
    let mut bytes = Vec::with_capacity(HEADER_SIZE + preview.len() + state.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&created.to_le_bytes());
    bytes.extend_from_slice(&nes.rom_hash().to_le_bytes());
    bytes.extend_from_slice(&(FRAME_WIDTH as u16).to_le_bytes());
    bytes.extend_from_slice(&(FRAME_HEIGHT as u16).to_le_bytes());
    bytes.extend_from_slice(&(preview.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(state.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&preview);
    bytes.extend_from_slice(&state);
    persistence::atomic_write(&path, &bytes)?;
    Ok(SlotInfo {
        created,
        preview_rgb: preview,
        path,
    })
}

pub fn load_slot(nes: &mut Nes, slot: usize) -> io::Result<SlotInfo> {
    let path = slot_path(nes.rom_hash(), slot);
    let bytes = fs::read(&path)?;
    let (info, state) = parse(&path, nes.rom_hash(), &bytes)?;
    nes.load_state(state).map_err(io::Error::other)?;
    Ok(info)
}

pub fn inspect_slots(rom_hash: u64, count: usize) -> Vec<Option<SlotInfo>> {
    (0..count)
        .map(|slot| {
            let path = slot_path(rom_hash, slot);
            fs::read(&path)
                .ok()
                .and_then(|bytes| parse(&path, rom_hash, &bytes).ok().map(|(info, _)| info))
        })
        .collect()
}

pub fn delete_slot(rom_hash: u64, slot: usize) -> io::Result<()> {
    let path = slot_path(rom_hash, slot);
    if path.exists() {
        fs::remove_file(path)
    } else {
        Ok(())
    }
}

pub fn format_timestamp(timestamp: u64) -> String {
    DateTime::from_timestamp(timestamp as i64, 0)
        .map(|time| {
            time.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "Unknown date".into())
}

fn parse<'a>(path: &Path, expected_hash: u64, bytes: &'a [u8]) -> io::Result<(SlotInfo, &'a [u8])> {
    if bytes.len() < HEADER_SIZE || &bytes[..8] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid save-state header",
        ));
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if version != FORMAT_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported save-state wrapper version {version}"),
        ));
    }
    let created = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    let hash = u64::from_le_bytes(bytes[20..28].try_into().unwrap());
    if hash != expected_hash {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "save state belongs to another ROM",
        ));
    }
    let width = u16::from_le_bytes(bytes[28..30].try_into().unwrap()) as usize;
    let height = u16::from_le_bytes(bytes[30..32].try_into().unwrap()) as usize;
    let preview_len = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;
    let state_len = u32::from_le_bytes(bytes[36..40].try_into().unwrap()) as usize;
    if width != FRAME_WIDTH
        || height != FRAME_HEIGHT
        || preview_len != width * height * 3
        || HEADER_SIZE + preview_len + state_len != bytes.len()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "corrupt save-state lengths",
        ));
    }
    let preview_rgb = bytes[HEADER_SIZE..HEADER_SIZE + preview_len].to_vec();
    let state = &bytes[HEADER_SIZE + preview_len..];
    Ok((
        SlotInfo {
            created,
            preview_rgb,
            path: path.to_path_buf(),
        },
        state,
    ))
}

pub fn slot_path(hash: u64, slot: usize) -> PathBuf {
    settings::state_root()
        .join(format!("{hash:016x}"))
        .join(format!("slot-{slot}.moss"))
}
