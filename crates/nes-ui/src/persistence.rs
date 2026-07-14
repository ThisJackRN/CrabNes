use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

const MAX_RECENT_ROMS: usize = 20;

pub fn app_directory() -> PathBuf {
    let base = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    base.join("MyOwnNesEmulator")
}

pub fn load_recent_roms() -> Vec<PathBuf> {
    fs::read_to_string(recent_file())
        .unwrap_or_default()
        .lines()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .take(MAX_RECENT_ROMS)
        .collect()
}

pub fn battery_path(rom_path: &Path) -> PathBuf {
    rom_path.with_extension("sav")
}

pub fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, data)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)
}

fn recent_file() -> PathBuf {
    app_directory().join("recent-roms.txt")
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
