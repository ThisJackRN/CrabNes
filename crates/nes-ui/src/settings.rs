use std::{collections::BTreeMap, fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

use gilrs::{Axis, Button as GamepadButton, ev::Code};

use crate::persistence;

pub const SETTINGS_VERSION: u32 = 6;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct KeyBinding(String);

impl KeyBinding {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn label(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum GamepadBinding {
    Button(GamepadButton),
    Axis {
        axis: Axis,
        direction: i8,
    },
    ExactButton {
        button: GamepadButton,
        code: Code,
    },
    ExactButtonLow {
        button: GamepadButton,
        code: Code,
    },
    ExactAxis {
        axis: Axis,
        code: Code,
        direction: i8,
    },
    ExactAxisLow {
        axis: Axis,
        code: Code,
    },
    RawButton(Code),
    RawAxis {
        code: Code,
        direction: i8,
    },
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
    pub achievements: AchievementSettings,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    pub reopen_last_game: bool,
    pub play_mode: PlayMode,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum PlayMode {
    #[default]
    Standard,
    Speedrun,
    Achievement,
}

impl PlayMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Speedrun => "Speedrun",
            Self::Achievement => "Achievements",
        }
    }

    pub const fn restricts_assists(self) -> bool {
        !matches!(self, Self::Standard)
    }
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
    pub crop_overscan: bool,
    pub crop_overscan_horizontal: bool,
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
            crop_overscan: true,
            crop_overscan_horizontal: false,
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
    pub gamepad_bindings: [Option<GamepadBinding>; 8],
    pub player2_gamepad_bindings: [Option<GamepadBinding>; 8],
    pub vs_coin_binding: KeyBinding,
    pub vs_coin_gamepad_binding: Option<GamepadBinding>,
    pub fds_swap_binding: KeyBinding,
    pub fds_swap_gamepad_binding: Option<GamepadBinding>,
    /// Connected-controller index for each player. `None` assigns controllers in player order.
    pub gamepad_slots: [Option<usize>; 2],
    pub gamepad_axis_threshold: f32,
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
    pub fds_bios_path: Option<PathBuf>,
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
pub struct AchievementSettings {
    pub username: String,
    pub token: String,
    /// Re-display unlock events that were already earned when a game loaded.
    /// Disabled by default so completed games do not flood the player with
    /// historical notifications after a reset or restart.
    pub show_replayed_unlocks: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PerGameSettings {
    pub volume: Option<f32>,
    pub muted: Option<bool>,
    pub speed_index: Option<usize>,
    pub cheats: Vec<CheatSetting>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CheatSetting {
    pub name: String,
    pub code: String,
    pub enabled: bool,
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
            achievements: AchievementSettings::default(),
        }
    }
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            reopen_last_game: true,
            play_mode: PlayMode::Standard,
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
                KeyBinding::new("Z"),
                KeyBinding::new("X"),
                KeyBinding::new("Shift"),
                KeyBinding::new("Enter"),
                KeyBinding::new("Up"),
                KeyBinding::new("Down"),
                KeyBinding::new("Left"),
                KeyBinding::new("Right"),
            ],
            player2_bindings: [
                KeyBinding::new("C"),
                KeyBinding::new("V"),
                KeyBinding::new("Q"),
                KeyBinding::new("E"),
                KeyBinding::new("I"),
                KeyBinding::new("K"),
                KeyBinding::new("J"),
                KeyBinding::new("L"),
            ],
            gamepad_bindings: default_gamepad_bindings(),
            player2_gamepad_bindings: default_gamepad_bindings(),
            vs_coin_binding: KeyBinding::new("5"),
            vs_coin_gamepad_binding: None,
            fds_swap_binding: KeyBinding::new("6"),
            fds_swap_gamepad_binding: None,
            gamepad_slots: [None, None],
            gamepad_axis_threshold: 0.5,
            allow_opposite_directions: false,
        }
    }
}

fn default_gamepad_bindings() -> [Option<GamepadBinding>; 8] {
    use GamepadBinding::Button;
    [
        Some(Button(GamepadButton::South)),
        Some(Button(GamepadButton::East)),
        Some(Button(GamepadButton::Select)),
        Some(Button(GamepadButton::Start)),
        Some(Button(GamepadButton::DPadUp)),
        Some(Button(GamepadButton::DPadDown)),
        Some(Button(GamepadButton::DPadLeft)),
        Some(Button(GamepadButton::DPadRight)),
    ]
}
impl Default for EmulationSettings {
    fn default() -> Self {
        Self {
            speed_index: 2,
            rewind_seconds: 120,
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
            fds_bios_path: std::env::current_dir()
                .ok()
                .map(|directory| directory.join("disksys.rom"))
                .filter(|path| path.is_file()),
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
    serde_json::from_slice(&data)
        .map(migrate)
        .unwrap_or_default()
}

fn migrate(mut settings: Settings) -> Settings {
    if settings.version < 2 {
        // Five seconds was the original default. Upgrade that value once, but
        // preserve any duration the player deliberately selected.
        if settings.emulation.rewind_seconds == 5 {
            settings.emulation.rewind_seconds = EmulationSettings::default().rewind_seconds;
        }
        settings.version = 2;
    }
    if settings.version < 3 {
        settings.version = 3;
    }
    if settings.version < 4 {
        settings.version = 4;
    }
    if settings.version < 5 {
        settings.version = 5;
    }
    if settings.version < 6 {
        settings.version = 6;
    }
    settings
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
        assert_eq!(
            settings.input.gamepad_bindings,
            InputSettings::default().gamepad_bindings
        );
        assert_eq!(settings.input.gamepad_slots, [None, None]);
        assert_eq!(settings.input.vs_coin_binding.label(), "5");
        assert_eq!(settings.input.vs_coin_gamepad_binding, None);
        assert_eq!(settings.input.gamepad_axis_threshold, 0.5);
        assert!(!settings.input.allow_opposite_directions);
        assert_eq!(settings.tas.checkpoint_interval, 300);
        assert!(!settings.video.crt_enabled);
        assert!(settings.video.crop_overscan);
        assert!(!settings.video.crop_overscan_horizontal);
        assert_eq!(
            settings.video.crt_scanline_strength,
            VideoSettings::default().crt_scanline_strength
        );
        assert_eq!(settings.video.crt_profile, CrtProfile::Royale);
        assert_eq!(settings.video.crt_mask, CrtMask::ApertureGrille);
        assert_eq!(settings.general.play_mode, PlayMode::Standard);
        assert!(settings.achievements.username.is_empty());
        assert!(settings.achievements.token.is_empty());
        assert!(!settings.achievements.show_replayed_unlocks);
        assert_eq!(
            settings.video.crt_halation_strength,
            VideoSettings::default().crt_halation_strength
        );
    }

    #[test]
    fn keyboard_bindings_remain_string_compatible_and_accept_new_keys() {
        let binding: KeyBinding = serde_json::from_str(r#""Backspace""#).unwrap();
        assert_eq!(binding.label(), "Backspace");
        assert_eq!(serde_json::to_string(&binding).unwrap(), r#""Backspace""#);
    }

    #[test]
    fn migration_extends_only_the_old_five_second_default() {
        let old_default = Settings {
            version: 1,
            emulation: EmulationSettings {
                rewind_seconds: 5,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(migrate(old_default).emulation.rewind_seconds, 120);

        let customized = Settings {
            version: 1,
            emulation: EmulationSettings {
                rewind_seconds: 30,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(migrate(customized).emulation.rewind_seconds, 30);
    }

    #[test]
    fn restricted_profiles_disable_assists_and_round_trip() {
        assert!(!PlayMode::Standard.restricts_assists());
        assert!(PlayMode::Speedrun.restricts_assists());
        assert!(PlayMode::Achievement.restricts_assists());

        for mode in [
            PlayMode::Standard,
            PlayMode::Speedrun,
            PlayMode::Achievement,
        ] {
            let encoded = serde_json::to_string(&mode).unwrap();
            assert_eq!(serde_json::from_str::<PlayMode>(&encoded).unwrap(), mode);
        }
    }

    #[test]
    fn per_game_cheats_round_trip_and_old_files_default_to_none() {
        let old: PerGameSettings = serde_json::from_str(r#"{"volume":0.5}"#).unwrap();
        assert!(old.cheats.is_empty());

        let settings = PerGameSettings {
            cheats: vec![CheatSetting {
                name: "Infinite lives".into(),
                code: "SXIOPO".into(),
                enabled: true,
            }],
            ..Default::default()
        };
        let decoded: PerGameSettings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(decoded.cheats.len(), 1);
        assert_eq!(decoded.cheats[0].code, "SXIOPO");
        assert!(decoded.cheats[0].enabled);
    }
}
