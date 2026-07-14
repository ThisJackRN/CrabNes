use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::persistence;
use nes_core::{Nes, Region};

const LIBRARY_VERSION: u32 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoverSource {
    Custom,
    RetroAchievements,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccuracyEstimate {
    pub score: u8,
    pub rating: String,
    pub details: Vec<String>,
}

#[derive(Clone)]
pub struct LibraryEntry {
    pub title: String,
    pub file_name: String,
    pub path: PathBuf,
    pub last_played: u64,
    pub cover_image: Option<PathBuf>,
    pub cover_source: Option<CoverSource>,
    pub accuracy: Option<AccuracyEstimate>,
    pub status: EntryStatus,
}

#[derive(Clone)]
pub enum EntryStatus {
    Ready,
    Missing,
    Invalid(String),
}

#[derive(Default, Serialize, Deserialize)]
struct LibraryFile {
    version: u32,
    opened: Vec<StoredEntry>,
    #[serde(default)]
    hidden: Vec<PathBuf>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredEntry {
    path: PathBuf,
    last_played: u64,
    #[serde(default)]
    custom_title: Option<String>,
    #[serde(default)]
    cover_image: Option<PathBuf>,
    #[serde(default)]
    retro_cover_image: Option<PathBuf>,
}

pub struct RomLibrary {
    stored: Vec<StoredEntry>,
    hidden: Vec<PathBuf>,
    pub entries: Vec<LibraryEntry>,
}

impl RomLibrary {
    pub fn load() -> Self {
        let file = fs::read(library_path())
            .ok()
            .and_then(|data| serde_json::from_slice::<LibraryFile>(&data).ok())
            .unwrap_or_default();
        let mut library = Self {
            stored: file.opened,
            hidden: file.hidden,
            entries: Vec::new(),
        };
        library.refresh(None);
        library
    }

    pub fn refresh(&mut self, folder: Option<&Path>) {
        struct Candidate {
            path: PathBuf,
            last_played: u64,
            custom_title: Option<String>,
            custom_cover: Option<PathBuf>,
            retro_cover: Option<PathBuf>,
        }

        let mut paths: Vec<Candidate> = self
            .stored
            .iter()
            .map(|entry| Candidate {
                path: entry.path.clone(),
                last_played: entry.last_played,
                custom_title: entry.custom_title.clone(),
                custom_cover: entry.cover_image.clone(),
                retro_cover: entry.retro_cover_image.clone(),
            })
            .collect();
        if let Some(folder) = folder
            && let Ok(read_dir) = fs::read_dir(folder)
        {
            for item in read_dir.flatten() {
                let path = item.path();
                if path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("nes"))
                    && !self
                        .hidden
                        .iter()
                        .any(|hidden| normalize(hidden) == normalize(&path))
                {
                    paths.push(Candidate {
                        path,
                        last_played: 0,
                        custom_title: None,
                        custom_cover: None,
                        retro_cover: None,
                    });
                }
            }
        }
        let mut seen = HashSet::new();
        let mut seen_roms = HashSet::new();
        self.entries.clear();
        for candidate in paths {
            let Candidate {
                path,
                last_played,
                custom_title,
                custom_cover,
                retro_cover,
            } = candidate;
            let key = normalize(&path);
            if !seen.insert(key) {
                continue;
            }
            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
                .to_owned();
            let default_title = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
                .replace(['_', '-'], " ");
            let title = custom_title
                .filter(|title| !title.trim().is_empty())
                .unwrap_or(default_title);
            let (status, rom_hash, accuracy) = if !path.is_file() {
                (EntryStatus::Missing, None, None)
            } else {
                match fs::read(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| {
                        Nes::from_ines(&bytes)
                            .map(|nes| {
                                let accuracy = estimate_accuracy(&nes, &bytes);
                                (nes.rom_hash(), accuracy)
                            })
                            .map_err(|e| e.to_string())
                    }) {
                    Ok((hash, accuracy)) => (EntryStatus::Ready, Some(hash), Some(accuracy)),
                    Err(error) => (EntryStatus::Invalid(error), None, None),
                }
            };
            if rom_hash.is_some_and(|hash| !seen_roms.insert(hash)) {
                continue;
            }
            let (cover_image, cover_source) = effective_cover(custom_cover, retro_cover);
            self.entries.push(LibraryEntry {
                title,
                file_name,
                path,
                last_played,
                cover_image,
                cover_source,
                accuracy,
                status,
            });
        }
        let available_names: HashSet<String> = self
            .entries
            .iter()
            .filter(|entry| matches!(&entry.status, EntryStatus::Ready))
            .map(|entry| entry.file_name.to_lowercase())
            .collect();
        self.entries.retain(|entry| {
            !matches!(&entry.status, EntryStatus::Missing)
                || !available_names.contains(&entry.file_name.to_lowercase())
        });
    }

    pub fn remember(&mut self, path: &Path) -> io::Result<()> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let key = normalize(&path);
        self.hidden.retain(|hidden| normalize(hidden) != key);
        let existing = self
            .stored
            .iter()
            .find(|entry| normalize(&entry.path) == key)
            .cloned();
        self.stored.retain(|entry| normalize(&entry.path) != key);
        self.stored.insert(
            0,
            StoredEntry {
                path,
                last_played: now,
                custom_title: existing
                    .as_ref()
                    .and_then(|entry| entry.custom_title.clone()),
                cover_image: existing
                    .as_ref()
                    .and_then(|entry| entry.cover_image.clone()),
                retro_cover_image: existing.and_then(|entry| entry.retro_cover_image),
            },
        );
        self.save()
    }

    pub fn recent(&self) -> Vec<&LibraryEntry> {
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.last_played > 0)
            .collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_played));
        entries
    }

    pub fn forget(&mut self, path: &Path) -> io::Result<()> {
        let key = normalize(path);
        let covers = self
            .stored
            .iter()
            .find(|entry| normalize(&entry.path) == key)
            .map(|entry| [entry.cover_image.clone(), entry.retro_cover_image.clone()])
            .unwrap_or_default();
        self.stored.retain(|entry| normalize(&entry.path) != key);
        self.entries.retain(|entry| normalize(&entry.path) != key);
        if !self.hidden.iter().any(|hidden| normalize(hidden) == key) {
            self.hidden.push(path.to_path_buf());
        }
        self.save()?;
        for cover in covers.into_iter().flatten() {
            remove_managed_cover(&cover);
        }
        Ok(())
    }

    pub fn rename(&mut self, path: &Path, title: &str) -> io::Result<()> {
        let title = title.trim();
        let custom_title = (!title.is_empty()).then(|| title.to_owned());
        let stored = self.ensure_stored(path);
        stored.custom_title = custom_title.clone();
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| normalize(&entry.path) == normalize(path))
        {
            entry.title = custom_title.unwrap_or_else(|| default_title(path));
        }
        self.save()
    }

    pub fn set_cover_image(&mut self, path: &Path, source: &Path) -> io::Result<PathBuf> {
        fs::create_dir_all(cover_directory())?;
        let extension = source
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("png")
            .to_ascii_lowercase();
        let digest = Sha256::digest(normalize(path).as_bytes());
        let id = digest[..12]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let destination = cover_directory().join(format!("{id}.{extension}"));
        fs::copy(source, &destination)?;
        let old_cover = self
            .ensure_stored(path)
            .cover_image
            .replace(destination.clone());
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| normalize(&entry.path) == normalize(path))
        {
            entry.cover_image = Some(destination.clone());
            entry.cover_source = Some(CoverSource::Custom);
        }
        self.save()?;
        if let Some(old_cover) = old_cover
            && old_cover != destination
            && old_cover.starts_with(cover_directory())
        {
            let _ = fs::remove_file(old_cover);
        }
        Ok(destination)
    }

    pub fn remove_cover_image(&mut self, path: &Path) -> io::Result<()> {
        let (old_cover, retro_cover) = {
            let stored = self.ensure_stored(path);
            (
                stored.cover_image.take(),
                stored
                    .retro_cover_image
                    .clone()
                    .filter(|image| image.is_file()),
            )
        };
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| normalize(&entry.path) == normalize(path))
        {
            entry.cover_image = retro_cover;
            entry.cover_source = entry
                .cover_image
                .as_ref()
                .map(|_| CoverSource::RetroAchievements);
        }
        self.save()?;
        if let Some(old_cover) = old_cover
            && old_cover.starts_with(cover_directory())
        {
            let _ = fs::remove_file(old_cover);
        }
        Ok(())
    }

    pub fn has_retro_cover(&self, path: &Path) -> bool {
        let key = normalize(path);
        self.stored
            .iter()
            .find(|entry| normalize(&entry.path) == key)
            .and_then(|entry| entry.retro_cover_image.as_ref())
            .is_some_and(|cover| cover.is_file())
    }

    pub fn set_retro_cover_image(
        &mut self,
        path: &Path,
        size: [usize; 2],
        rgba: &[u8],
    ) -> io::Result<PathBuf> {
        let expected_len = size[0]
            .checked_mul(size[1])
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "image is too large"))?;
        if size.contains(&0) || rgba.len() != expected_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid RGBA image dimensions",
            ));
        }
        let width = u32::try_from(size[0])
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "image is too wide"))?;
        let height = u32::try_from(size[1])
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "image is too tall"))?;
        fs::create_dir_all(cover_directory())?;
        let destination =
            cover_directory().join(format!("{}-retroachievements.png", cover_id(path)));
        image::save_buffer_with_format(
            &destination,
            rgba,
            width,
            height,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(io::Error::other)?;

        let (old_cover, custom_active) = {
            let stored = self.ensure_stored(path);
            let old_cover = stored.retro_cover_image.replace(destination.clone());
            let custom_active = stored
                .cover_image
                .as_ref()
                .is_some_and(|cover| cover.is_file());
            (old_cover, custom_active)
        };
        if !custom_active
            && let Some(entry) = self
                .entries
                .iter_mut()
                .find(|entry| normalize(&entry.path) == normalize(path))
        {
            entry.cover_image = Some(destination.clone());
            entry.cover_source = Some(CoverSource::RetroAchievements);
        }
        self.save()?;
        if let Some(old_cover) = old_cover
            && old_cover != destination
        {
            remove_managed_cover(&old_cover);
        }
        Ok(destination)
    }

    fn save(&self) -> io::Result<()> {
        let file = LibraryFile {
            version: LIBRARY_VERSION,
            opened: self.stored.clone(),
            hidden: self.hidden.clone(),
        };
        let data = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
        persistence::atomic_write(&library_path(), &data)
    }

    fn ensure_stored(&mut self, path: &Path) -> &mut StoredEntry {
        let key = normalize(path);
        if let Some(index) = self
            .stored
            .iter()
            .position(|entry| normalize(&entry.path) == key)
        {
            return &mut self.stored[index];
        }
        let last_played = self
            .entries
            .iter()
            .find(|entry| normalize(&entry.path) == key)
            .map_or(0, |entry| entry.last_played);
        self.stored.push(StoredEntry {
            path: path.to_path_buf(),
            last_played,
            custom_title: None,
            cover_image: None,
            retro_cover_image: None,
        });
        self.stored.last_mut().unwrap()
    }
}

fn estimate_accuracy(nes: &Nes, bytes: &[u8]) -> AccuracyEstimate {
    let mapper = nes.mapper_id();
    let (mut score, mapper_note): (u8, &str) = match mapper {
        0 => (
            94,
            "NROM has no bank-switching hardware and receives the strongest coverage.",
        ),
        1 => (
            91,
            "MMC1 banking, mirroring, RAM, and serial writes are covered.",
        ),
        2 | 3 | 7 => (
            92,
            "This discrete mapper family has focused banking and mirroring tests.",
        ),
        9 | 10 => (
            87,
            "MMC2/MMC4 latch behavior is supported; uncommon board details need more coverage.",
        ),
        4 => (
            84,
            "MMC3 is broadly supported, but exact IRQ and MMC6 board variants remain accuracy work.",
        ),
        21 | 22 | 23 | 25 => (
            82,
            "VRC2/VRC4 banking and cycle IRQs work; wiring variants reduce confidence.",
        ),
        24 | 26 => (
            80,
            "VRC6 banking, IRQs, and expansion audio work; analog audio matching is approximate.",
        ),
        69 => (
            80,
            "FME-7 banking, IRQs, and Sunsoft 5B audio work; uncommon board variants need testing.",
        ),
        19 => (
            76,
            "Namco 163 banking, IRQs, and audio work; channel mixing remains approximate.",
        ),
        5 => (
            68,
            "MMC5 works, but extended attributes and vertical split rendering remain incomplete.",
        ),
        85 => (
            65,
            "VRC7 works, but FM operator and envelope behavior is still experimental.",
        ),
        _ => (
            50,
            "This mapper does not have a dedicated CrabNes confidence profile yet.",
        ),
    };
    let mut details = vec![format!("Mapper {mapper}: {mapper_note}")];

    let nes2 = bytes.get(7).is_some_and(|value| value & 0x0c == 0x08);
    let submapper = if nes2 {
        bytes.get(8).map_or(0, |value| value >> 4)
    } else {
        0
    };
    if nes2 {
        details.push(format!("NES 2.0 header; submapper {submapper}."));
        if submapper != 0 && !matches!(mapper, 19 | 21 | 22 | 23 | 25 | 85) {
            score = score.saturating_sub(6);
            details.push(
                "This mapper does not yet specialize every NES 2.0 submapper variant.".into(),
            );
        }
    } else {
        details.push("iNES header; hardware variants may not be fully identified.".into());
    }

    match nes.region() {
        Region::Ntsc => details.push("NTSC cycle timing is enabled.".into()),
        Region::Pal => {
            score = score.saturating_sub(2);
            details.push(
                "PAL timing is enabled; its automated game coverage is smaller than NTSC.".into(),
            );
        }
    }
    details.push(
        "The shared CPU and PPU accuracy suite is applied, but the PPU fetch pipeline is not yet dot-perfect."
            .into(),
    );
    details.push(
        "This estimate is recomputed from the current core and ROM metadata whenever CrabNes starts or the library is refreshed."
            .into(),
    );

    AccuracyEstimate {
        score,
        rating: match score {
            90..=u8::MAX => "High",
            80..=89 => "Good",
            70..=79 => "Fair",
            _ => "Experimental",
        }
        .into(),
        details,
    }
}

fn default_title(path: &Path) -> String {
    path.file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Unknown")
        .replace(['_', '-'], " ")
}

fn cover_id(path: &Path) -> String {
    let digest = Sha256::digest(normalize(path).as_bytes());
    digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn effective_cover(
    custom: Option<PathBuf>,
    retro: Option<PathBuf>,
) -> (Option<PathBuf>, Option<CoverSource>) {
    if let Some(custom) = custom.filter(|image| image.is_file()) {
        (Some(custom), Some(CoverSource::Custom))
    } else if let Some(retro) = retro.filter(|image| image.is_file()) {
        (Some(retro), Some(CoverSource::RetroAchievements))
    } else {
        (None, None)
    }
}

fn remove_managed_cover(path: &Path) {
    if path.starts_with(cover_directory()) {
        let _ = fs::remove_file(path);
    }
}

fn normalize(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_lowercase()
}

pub fn library_path() -> PathBuf {
    persistence::app_directory().join("library.json")
}

pub fn cover_directory() -> PathBuf {
    persistence::app_directory().join("library-covers")
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn test_rom() -> Vec<u8> {
        let mut rom = vec![0; 16 + 0x4000 + 0x2000];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 1;
        rom[5] = 1;
        rom[16..19].copy_from_slice(&[0x4c, 0x00, 0x80]);
        rom[16 + 0x3ffa..16 + 0x4000].copy_from_slice(&[0x00, 0x80, 0x00, 0x80, 0x00, 0x80]);
        rom
    }

    #[test]
    fn scan_marks_invalid_files_and_deduplicates_identical_roms() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("my-own-nes-library-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("game.nes"), test_rom()).unwrap();
        fs::write(dir.join("duplicate.nes"), test_rom()).unwrap();
        fs::write(dir.join("broken.nes"), b"not a ROM").unwrap();

        let mut library = RomLibrary {
            stored: Vec::new(),
            hidden: Vec::new(),
            entries: Vec::new(),
        };
        library.refresh(Some(&dir));
        assert_eq!(library.entries.len(), 2);
        assert_eq!(
            library
                .entries
                .iter()
                .filter(|entry| matches!(&entry.status, EntryStatus::Ready))
                .count(),
            1
        );
        let ready = library
            .entries
            .iter()
            .find(|entry| matches!(&entry.status, EntryStatus::Ready))
            .unwrap();
        assert_eq!(ready.accuracy.as_ref().unwrap().score, 94);
        assert_eq!(ready.accuracy.as_ref().unwrap().rating, "High");
        assert!(
            library
                .entries
                .iter()
                .any(|entry| matches!(&entry.status, EntryStatus::Invalid(_)))
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn refresh_recomputes_accuracy_from_the_current_rom() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "crabnes-accuracy-refresh-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("game.nes");
        let mut rom = test_rom();
        fs::write(&path, &rom).unwrap();

        let mut library = RomLibrary {
            stored: Vec::new(),
            hidden: Vec::new(),
            entries: Vec::new(),
        };
        library.refresh(Some(&dir));
        assert_eq!(library.entries[0].accuracy.as_ref().unwrap().score, 94);

        rom[9] = 1;
        fs::write(&path, rom).unwrap();
        library.refresh(Some(&dir));
        assert_eq!(library.entries[0].accuracy.as_ref().unwrap().score, 92);
        assert!(
            library.entries[0]
                .accuracy
                .as_ref()
                .unwrap()
                .details
                .iter()
                .any(|detail| detail.contains("recomputed"))
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn nes2_submapper_without_specialized_board_logic_lowers_confidence() {
        let mut rom = vec![0; 16 + 0x8000 + 0x2000];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 2;
        rom[5] = 1;
        rom[6] = 0x20;
        rom[7] = 0x08;
        rom[8] = 0x10;
        rom[16 + 0x7ffa..16 + 0x8000].copy_from_slice(&[0x00, 0x80, 0x00, 0x80, 0x00, 0x80]);
        let nes = Nes::from_ines(&rom).unwrap();
        let estimate = estimate_accuracy(&nes, &rom);
        assert_eq!(estimate.score, 86);
        assert_eq!(estimate.rating, "Good");
        assert!(
            estimate
                .details
                .iter()
                .any(|detail| detail.contains("submapper variant"))
        );
    }

    #[test]
    fn custom_artwork_overrides_retro_artwork() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "crabnes-cover-priority-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let custom = dir.join("custom.png");
        let retro = dir.join("retro.png");
        fs::write(&custom, b"custom").unwrap();
        fs::write(&retro, b"retro").unwrap();

        assert_eq!(
            effective_cover(Some(custom.clone()), Some(retro.clone())),
            (Some(custom.clone()), Some(CoverSource::Custom))
        );
        fs::remove_file(&custom).unwrap();
        assert_eq!(
            effective_cover(Some(custom), Some(retro.clone())),
            (Some(retro), Some(CoverSource::RetroAchievements))
        );

        let _ = fs::remove_dir_all(dir);
    }
}
