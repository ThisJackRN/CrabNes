use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

const MAX_RECENT_ROMS: usize = 9;

pub fn load_recent_roms() -> Vec<PathBuf> {
    fs::read_to_string(recent_file())
        .unwrap_or_default()
        .lines()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .take(MAX_RECENT_ROMS)
        .collect()
}

pub fn remember_rom(recent: &mut Vec<PathBuf>, path: &Path) -> io::Result<()> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    recent.retain(|existing| existing != &path);
    recent.insert(0, path);
    recent.truncate(MAX_RECENT_ROMS);

    let file = recent_file();
    if let Some(directory) = file.parent() {
        fs::create_dir_all(directory)?;
    }
    let contents = recent
        .iter()
        .map(|entry| entry.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");
    atomic_write(&file, contents.as_bytes())
}

pub fn battery_path(rom_path: &Path) -> PathBuf {
    rom_path.with_extension("sav")
}

pub fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, data)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)
}

fn recent_file() -> PathBuf {
    let base = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    base.join("MyOwnNesEmulator").join("recent-roms.txt")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_save_sits_next_to_rom() {
        assert_eq!(
            battery_path(Path::new(r"C:\Games\example.nes")),
            PathBuf::from(r"C:\Games\example.sav")
        );
    }
}
