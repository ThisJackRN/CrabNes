use std::{
    fs, io,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::persistence;

const ARCHIVE_VERSION: u32 = 1;
const MAX_ARCHIVE_ENTRIES: usize = 500;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UnlockEntry {
    pub game_id: u32,
    pub achievement_id: u32,
    pub game_title: String,
    pub title: String,
    pub description: String,
    pub points: u32,
    pub badge_url: String,
    pub unlocked_at: i64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Archive {
    version: u32,
    pub entries: Vec<UnlockEntry>,
}

impl Archive {
    pub fn load() -> Self {
        fs::read(path())
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_else(|| Self {
                version: ARCHIVE_VERSION,
                entries: Vec::new(),
            })
    }

    pub fn record(&mut self, mut entry: UnlockEntry) -> io::Result<()> {
        if entry.unlocked_at == 0 {
            entry.unlocked_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
        }
        self.entries.retain(|existing| {
            existing.game_id != entry.game_id || existing.achievement_id != entry.achievement_id
        });
        self.entries.insert(0, entry);
        self.entries.truncate(MAX_ARCHIVE_ENTRIES);
        self.version = ARCHIVE_VERSION;
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        let data = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        persistence::atomic_write(&path(), &data)
    }
}

fn path() -> std::path::PathBuf {
    persistence::app_directory().join("achievement-archive.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_unlocks_replace_the_old_archive_entry() {
        let mut archive = Archive::default();
        let first = UnlockEntry {
            game_id: 1,
            achievement_id: 2,
            title: "First title".into(),
            ..Default::default()
        };
        archive.entries.push(first);
        let replacement = UnlockEntry {
            game_id: 1,
            achievement_id: 2,
            title: "Updated title".into(),
            unlocked_at: 10,
            ..Default::default()
        };
        archive.entries.retain(|existing| {
            existing.game_id != replacement.game_id
                || existing.achievement_id != replacement.achievement_id
        });
        archive.entries.insert(0, replacement);
        assert_eq!(archive.entries.len(), 1);
        assert_eq!(archive.entries[0].title, "Updated title");
    }
}
