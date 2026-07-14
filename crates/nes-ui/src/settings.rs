use std::{collections::BTreeMap, fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::persistence;

pub const SETTINGS_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum KeyBinding {
    Z,
    X,
    Enter,
    Shift,
    Up,
    Down,
    Left,
    Right,
    A,
    S,
    Q,
    W,
    C,
    V,
    E,
    I,
    J,
    K,
    L,
}

impl KeyBinding {
    pub const ALL: [Self; 19] = [
        Self::Z,
        Self::X,
        Self::Enter,
        Self::Shift,
        Self::Up,
        Self::Down,
        Self::Left,
        Self::Right,
        Self::A,
        Self::S,
        Self::Q,
        Self::W,
        Self::C,
        Self::V,
        Self::E,
        Self::I,
        Self::J,
        Self::K,
        Self::L,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Z => "Z",
            Self::X => "X",
            Self::Enter => "Enter",
            Self::Shift => "Shift",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::A => "A",
            Self::S => "S",
            Self::Q => "Q",
            Self::W => "W",
            Self::C => "C",
            Self::V => "V",
            Self::E => "E",
            Self::I => "I",
            Self::J => "J",
            Self::K => "K",
            Self::L => "L",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub general: GeneralSettings,
    pub video: VideoSettings,
    pub audio: AudioSettings,
    pub input: InputSettings,
    pub emulation: EmulationSettings,
    pub paths: PathSettings,
    pub save_states: SaveStateSettings,
    pub tas: TasSettings,
    pub debugging: DebugSettings,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    pub reopen_last_game: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum PaletteMode {
    #[default]
    Ntsc2c02,
    Rgb2c03,
    Custom,
}

impl PaletteMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ntsc2c02 => "NTSC 2C02 (default)",
            Self::Rgb2c03 => "RGB 2C03 / PlayChoice-10",
            Self::Custom => "Custom imported palette",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum CrtProfile {
    Lightweight,
    Flat,
    #[default]
    Royale,
}

impl CrtProfile {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Lightweight => "Lightweight CRT",
            Self::Flat => "Flat CRT (no screen geometry)",
            Self::Royale => "Royale-style advanced",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum CrtMask {
    #[default]
    ApertureGrille,
    SlotMask,
    ShadowMask,
}

impl CrtMask {
    pub const fn label(self) -> &'static str {
        match self {
            Self::ApertureGrille => "Aperture grille / PVM",
            Self::SlotMask => "Slot mask / consumer TV",
            Self::ShadowMask => "Shadow mask",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoSettings {
    pub integer_scaling: bool,
    pub show_fps: bool,
    pub fullscreen_on_start: bool,
    pub palette_mode: PaletteMode,
    pub custom_palette_path: Option<PathBuf>,
    pub crt_enabled: bool,
    pub crt_profile: CrtProfile,
    pub crt_mask: CrtMask,
    pub crt_scanline_strength: f32,
    pub crt_mask_strength: f32,
    pub crt_bloom_strength: f32,
    pub crt_curvature: f32,
    pub crt_halation_strength: f32,
    pub crt_diffusion_strength: f32,
    pub crt_convergence: f32,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            integer_scaling: false,
            show_fps: false,
            fullscreen_on_start: false,
            palette_mode: PaletteMode::default(),
            custom_palette_path: None,
            crt_enabled: false,
            crt_profile: CrtProfile::Royale,
            crt_mask: CrtMask::ApertureGrille,
            crt_scanline_strength: 0.38,
            crt_mask_strength: 0.32,
            crt_bloom_strength: 0.22,
            crt_curvature: 0.055,
            crt_halation_strength: 0.18,
            crt_diffusion_strength: 0.10,
            crt_convergence: 0.12,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    pub volume: f32,
    pub muted: bool,
    pub soft_clip: bool,
    pub startup_buffer_ms: u32,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InputSettings {
    pub bindings: [KeyBinding; 8],
    pub player2_bindings: [KeyBinding; 8],
    pub allow_opposite_directions: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmulationSettings {
    pub speed_index: usize,
    pub rewind_seconds: usize,
    pub rewind_interval_frames: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PathSettings {
    pub rom_folder: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveStateSettings {
    pub slots: usize,
    pub selected_slot: usize,
    pub autosave_on_exit: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TasSettings {
    pub pause_when_playback_ends: bool,
    pub checkpoint_interval: usize,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DebugSettings {
    pub hex_rows: usize,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PerGameSettings {
    pub volume: Option<f32>,
    pub muted: Option<bool>,
    pub speed_index: Option<usize>,
}

#[derive(Default, Serialize, Deserialize)]
struct PerGameFile {
    version: u32,
    games: BTreeMap<String, PerGameSettings>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            general: GeneralSettings::default(),
            video: VideoSettings::default(),
            audio: AudioSettings::default(),
            input: InputSettings::default(),
            emulation: EmulationSettings::default(),
            paths: PathSettings::default(),
            save_states: SaveStateSettings::default(),
            tas: TasSettings::default(),
            debugging: DebugSettings::default(),
        }
    }
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            reopen_last_game: true,
        }
    }
}
impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            volume: 0.75,
            muted: false,
            soft_clip: false,
            startup_buffer_ms: 40,
        }
    }
}
impl Default for InputSettings {
    fn default() -> Self {
        Self {
            bindings: [
                KeyBinding::Z,
                KeyBinding::X,
                KeyBinding::Shift,
                KeyBinding::Enter,
                KeyBinding::Up,
                KeyBinding::Down,
                KeyBinding::Left,
                KeyBinding::Right,
            ],
            player2_bindings: [
                KeyBinding::C,
                KeyBinding::V,
                KeyBinding::Q,
                KeyBinding::E,
                KeyBinding::I,
                KeyBinding::K,
                KeyBinding::J,
                KeyBinding::L,
            ],
            allow_opposite_directions: false,
        }
    }
}
impl Default for EmulationSettings {
    fn default() -> Self {
        Self {
            speed_index: 2,
            rewind_seconds: 5,
            rewind_interval_frames: 2,
        }
    }
}
impl Default for PathSettings {
    fn default() -> Self {
        let root = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_default();
        Self {
            rom_folder: root.join("Documents").join("NES ROMs"),
        }
    }
}
impl Default for SaveStateSettings {
    fn default() -> Self {
        Self {
            slots: 10,
            selected_slot: 0,
            autosave_on_exit: false,
        }
    }
}
impl Default for TasSettings {
    fn default() -> Self {
        Self {
            pause_when_playback_ends: true,
            checkpoint_interval: 300,
        }
    }
}
impl Default for DebugSettings {
    fn default() -> Self {
        Self { hex_rows: 16 }
    }
}

pub fn load() -> Settings {
    let path = settings_path();
    let Ok(data) = fs::read(&path) else {
        return Settings::default();
    };
    serde_json::from_slice(&data).unwrap_or_default()
}

pub fn save(settings: &Settings) -> io::Result<()> {
    let data = serde_json::to_vec_pretty(settings).map_err(io::Error::other)?;
    persistence::atomic_write(&settings_path(), &data)
}

pub fn load_per_game(hash: u64) -> PerGameSettings {
    load_per_game_file()
        .games
        .remove(&format!("{hash:016x}"))
        .unwrap_or_default()
}

pub fn save_per_game(hash: u64, value: &PerGameSettings) -> io::Result<()> {
    let mut file = load_per_game_file();
    file.version = SETTINGS_VERSION;
    file.games.insert(format!("{hash:016x}"), value.clone());
    let data = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
    persistence::atomic_write(&per_game_path(), &data)
}

fn load_per_game_file() -> PerGameFile {
    fs::read(per_game_path())
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

pub fn settings_path() -> PathBuf {
    persistence::app_directory().join("settings.json")
}
pub fn per_game_path() -> PathBuf {
    persistence::app_directory().join("per-game-settings.json")
}
pub fn state_root() -> PathBuf {
    persistence::app_directory().join("states")
}
pub fn tas_root() -> PathBuf {
    persistence::app_directory().join("tas")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_settings_keep_known_values_and_default_new_fields() {
        let settings: Settings = serde_json::from_str(
            r#"{"version":1,"audio":{"volume":0.25},"general":{"reopen_last_game":false}}"#,
        )
        .unwrap();
        assert_eq!(settings.audio.volume, 0.25);
        assert!(!settings.general.reopen_last_game);
        assert_eq!(settings.audio.startup_buffer_ms, 40);
        assert_eq!(settings.input.bindings, InputSettings::default().bindings);
        assert_eq!(
            settings.input.player2_bindings,
            InputSettings::default().player2_bindings
        );
        assert!(!settings.input.allow_opposite_directions);
        assert_eq!(settings.tas.checkpoint_interval, 300);
        assert!(!settings.video.crt_enabled);
        assert_eq!(
            settings.video.crt_scanline_strength,
            VideoSettings::default().crt_scanline_strength
        );
        assert_eq!(settings.video.crt_profile, CrtProfile::Royale);
        assert_eq!(settings.video.crt_mask, CrtMask::ApertureGrille);
        assert_eq!(
            settings.video.crt_halation_strength,
            VideoSettings::default().crt_halation_strength
        );
    }
}
