use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::persistence;

const LIBRARY_VERSION: u32 = 2;

#[derive(Clone)]
pub struct LibraryEntry {
    pub title: String,
    pub file_name: String,
    pub path: PathBuf,
    pub last_played: u64,
    pub cover_image: Option<PathBuf>,
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
        let mut paths: Vec<(PathBuf, u64, Option<String>, Option<PathBuf>)> = self
            .stored
            .iter()
            .map(|e| {
                (
                    e.path.clone(),
                    e.last_played,
                    e.custom_title.clone(),
                    e.cover_image.clone(),
                )
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
                    paths.push((path, 0, None, None));
                }
            }
        }
        let mut seen = HashSet::new();
        let mut seen_roms = HashSet::new();
        self.entries.clear();
        for (path, last_played, custom_title, cover_image) in paths {
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
            let (status, rom_hash) = if !path.is_file() {
                (EntryStatus::Missing, None)
            } else {
                match fs::read(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| {
                        nes_core::Nes::from_ines(&bytes)
                            .map(|nes| nes.rom_hash())
                            .map_err(|e| e.to_string())
                    }) {
                    Ok(hash) => (EntryStatus::Ready, Some(hash)),
                    Err(error) => (EntryStatus::Invalid(error), None),
                }
            };
            if rom_hash.is_some_and(|hash| !seen_roms.insert(hash)) {
                continue;
            }
            self.entries.push(LibraryEntry {
                title,
                file_name,
                path,
                last_played,
                cover_image: cover_image.filter(|image| image.is_file()),
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
                cover_image: existing.and_then(|entry| entry.cover_image),
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
        let cover = self
            .stored
            .iter()
            .find(|entry| normalize(&entry.path) == key)
            .and_then(|entry| entry.cover_image.clone());
        self.stored.retain(|entry| normalize(&entry.path) != key);
        self.entries.retain(|entry| normalize(&entry.path) != key);
        if !self.hidden.iter().any(|hidden| normalize(hidden) == key) {
            self.hidden.push(path.to_path_buf());
        }
        self.save()?;
        if let Some(cover) = cover
            && cover.starts_with(cover_directory())
        {
            let _ = fs::remove_file(cover);
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
        let old_cover = self.ensure_stored(path).cover_image.take();
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| normalize(&entry.path) == normalize(path))
        {
            entry.cover_image = None;
        }
        self.save()?;
        if let Some(old_cover) = old_cover
            && old_cover.starts_with(cover_directory())
        {
            let _ = fs::remove_file(old_cover);
        }
        Ok(())
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
        });
        self.stored.last_mut().unwrap()
    }
}

fn default_title(path: &Path) -> String {
    path.file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Unknown")
        .replace(['_', '-'], " ")
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
        assert!(
            library
                .entries
                .iter()
                .any(|entry| matches!(&entry.status, EntryStatus::Invalid(_)))
        );
        let _ = fs::remove_dir_all(dir);
    }
}
