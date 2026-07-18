use std::{
    collections::{HashMap, HashSet, VecDeque},
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    thread,
    time::{Duration, Instant},
};

use eframe::egui::{self, ColorImage, Key, TextureHandle, TextureOptions, Vec2};
use gilrs::{Axis, Button as GamepadButton, EventType, Gamepad, Gilrs};
use lz4_flex::block::{compress_prepend_size, decompress_size_prepended};
use nes_achievements_native::{
    AchievementBucket, Event as AchievementEvent, EventKind as AchievementEventKind,
};
use nes_core::{
    ApuChannel, Button, Cheat, CheatActivity, FRAME_HEIGHT, FRAME_WIDTH, MemorySpace,
    NTSC_2C02_PALETTE, NTSC_FRAME_RATE, Nes, OutputPalette, RGB_2C03_PALETTE,
    RGB_2C04_0004_PALETTE, Region,
};
use rfd::FileDialog;

use crate::{
    achievement_archive::{Archive as AchievementArchive, UnlockEntry},
    achievements,
    audio::AudioOutput,
    crt::{CrtParameters, CrtRenderer},
    library::{CoverSource, EntryStatus, LibraryEntry, RomLibrary},
    palettes, persistence, save_states, screenshot,
    settings::{
        self, CheatSetting, CrtMask, CrtProfile, GamepadBinding, KeyBinding, PaletteMode,
        PerGameSettings, PlayMode, Settings, VideoSettings,
    },
    tas::{self, TasEditor, TasFrame, TasManager, TasMode, TasMovie, TasStartType},
    tas_control::{self, ControlMovie},
};

mod achievements_ui;
mod emulation;
mod input;
mod pages;
mod rewind;
mod tas_ui;
mod windows;

use achievements_ui::*;
use emulation::*;
use input::*;
use rewind::*;

const SPEEDS: &[f64] = &[0.25, 0.5, 1.0, 1.5, 2.0, 3.0, 4.0];
const NORMAL_SPEED_INDEX: usize = 2;
const REWIND_UPDATES_PER_SECOND: f64 = 60.0;
// Permit a normal-speed frame to begin up to this fraction early. This keeps
// presentation callbacks phase-locked to the console's native cadence instead
// of alternating between zero frames and a two-frame catch-up due to timer jitter.
const NORMAL_SPEED_FRAME_TOLERANCE: f64 = 0.08;

#[derive(Clone, Copy, Eq, PartialEq)]
enum MainPage {
    Game,
    Library,
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum LibrarySort {
    Title,
    Recent,
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum SettingsCategory {
    General,
    Video,
    Audio,
    Input,
    Emulation,
    Paths,
    SaveStates,
    Tas,
    Debugging,
}

#[derive(Clone, Copy)]
enum TasTimelineAction {
    InsertBlank,
    Duplicate,
    Delete,
    Fill,
    Clear,
    Copy,
    Paste,
    InsertPaste,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TasCheckpointRecovery {
    RefreshedChecksum,
    Resynchronized,
    Unrecoverable,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum BindingCapture {
    Keyboard { player: usize, button: usize },
    Gamepad { player: usize, button: usize },
    VsCoinKeyboard,
    VsCoinGamepad,
    FdsSwapKeyboard,
    FdsSwapGamepad,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum AchievementPanel {
    CurrentSet,
    Archive,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum AchievementFilter {
    All,
    Locked,
    Unlocked,
}

pub struct App {
    nes: Option<Nes>,
    rom_path: Option<PathBuf>,
    rom_bytes: Vec<u8>,
    texture: TextureHandle,
    crt_renderer: CrtRenderer,
    frame_dirty: bool,
    paused: bool,
    powered: bool,
    fullscreen: bool,
    speed_index: usize,
    fast_forward: bool,
    frame_budget: f64,
    last_tick: Instant,
    fps_window_start: Instant,
    presented_frames_in_window: u64,
    measured_fps: f64,
    next_rewind_step: Instant,
    rewind_active: bool,
    resume_after_rewind: bool,
    last_held_frame_advance: Instant,
    frame_advance_hold_started: Option<Instant>,
    frame_advance_held: bool,
    frame_advance_repeated: bool,
    status: String,
    audio: Option<AudioOutput>,
    audio_error: Option<String>,
    audio_scratch: Vec<f32>,
    gamepads: Option<Gilrs>,
    gamepad_error: Option<String>,
    last_gamepad_activity: Option<String>,
    binding_capture: Option<BindingCapture>,
    achievements: achievements::Manager,
    show_achievements: bool,
    achievement_password: String,
    achievement_feed: VecDeque<String>,
    achievement_panel: AchievementPanel,
    achievement_filter: AchievementFilter,
    achievement_archive: AchievementArchive,
    achievement_badges: HashMap<String, TextureHandle>,
    achievement_badges_requested: HashSet<String>,
    achievement_toasts: VecDeque<AchievementToast>,
    achievement_known_unlocked: HashSet<u32>,
    achievement_game_mastered: bool,
    settings: Settings,
    active_play_mode: PlayMode,
    per_game: PerGameSettings,
    settings_dirty: bool,
    settings_category: SettingsCategory,
    library: RomLibrary,
    library_search: String,
    library_sort: LibrarySort,
    library_cover_textures: HashMap<PathBuf, TextureHandle>,
    library_artwork: achievements::LibraryArtworkLoader,
    library_rename_path: Option<PathBuf>,
    library_rename_text: String,
    page: MainPage,
    show_states: bool,
    show_time: bool,
    show_tas: bool,
    show_tas_control: bool,
    show_input: bool,
    show_av: bool,
    show_debugger: bool,
    show_hex: bool,
    show_cheats: bool,
    show_settings: bool,
    selected_slot: usize,
    state_slots: Vec<Option<save_states::SlotInfo>>,
    state_preview: Option<TextureHandle>,
    preview_slot: Option<usize>,
    tas: TasManager,
    tas_held_input: TasFrame,
    tas_timeline_scroll: Option<usize>,
    tas_control_movie: Option<ControlMovie>,
    tas_control_selected: usize,
    tas_control_scroll: Option<usize>,
    tas_control_start: TasStartType,
    tas_control_include_cheats: bool,
    tas_control_fceux_timing: bool,
    tas_control_status: String,
    rewind: VecDeque<RewindPoint>,
    rewind_compressor: RewindCompressor,
    rewind_generation: u64,
    lag_frames: u64,
    last_controller_reads: u64,
    hex_space: MemorySpace,
    hex_start: usize,
    hex_jump: String,
    hex_selected: Option<usize>,
    hex_value: String,
    cheat_name: String,
    cheat_code: String,
    cheat_error: Option<String>,
    cheat_flash: Vec<CheatFlash>,
    fds_swap_was_down: bool,
}

/// Per-cheat presentation state that lights an activity dot for a short time
/// after the substitution counter advances.
pub(super) struct CheatFlash {
    hits: u64,
    flash_until: Option<Instant>,
}

impl App {
    pub fn new(path: Option<PathBuf>, cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        let settings = settings::load();
        let play_mode = settings.general.play_mode;
        let image = ColorImage::from_rgb(
            [FRAME_WIDTH, FRAME_HEIGHT],
            &vec![0; FRAME_WIDTH * FRAME_HEIGHT * 3],
        );
        let texture = cc
            .egui_ctx
            .load_texture("nes-frame", image, TextureOptions::NEAREST);
        let (audio, audio_error) = match AudioOutput::new(48_000, settings.audio.startup_buffer_ms)
        {
            Ok(audio) => (Some(audio), None),
            Err(error) => (None, Some(error)),
        };
        let (gamepads, gamepad_error) = match Gilrs::new() {
            Ok(gamepads) => (Some(gamepads), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let achievements = achievements::Manager::new()?;
        let library_artwork = achievements::LibraryArtworkLoader::new()?;
        let mut tas_manager = TasManager::default();
        tas_manager.checkpoint_interval = settings.tas.checkpoint_interval.max(1);
        let migrate_legacy_recents = !crate::library::library_path().is_file();
        let mut library = RomLibrary::load();
        if migrate_legacy_recents {
            for recent in persistence::load_recent_roms().into_iter().rev() {
                let _ = library.remember(&recent);
            }
        }
        library.refresh(Some(&settings.paths.rom_folder));
        let initial = path.or_else(|| {
            settings
                .general
                .reopen_last_game
                .then(|| {
                    library
                        .recent()
                        .into_iter()
                        .find(|entry| matches!(&entry.status, EntryStatus::Ready))
                        .map(|entry| entry.path.clone())
                })
                .flatten()
        });
        let mut app = Self {
            nes: None,
            rom_path: None,
            rom_bytes: Vec::new(),
            texture,
            crt_renderer: CrtRenderer::default(),
            frame_dirty: false,
            paused: true,
            powered: false,
            fullscreen: settings.video.fullscreen_on_start,
            speed_index: if play_mode.restricts_assists() {
                NORMAL_SPEED_INDEX
            } else {
                settings.emulation.speed_index.min(SPEEDS.len() - 1)
            },
            fast_forward: false,
            frame_budget: 0.0,
            last_tick: Instant::now(),
            fps_window_start: Instant::now(),
            presented_frames_in_window: 0,
            measured_fps: 0.0,
            next_rewind_step: Instant::now(),
            rewind_active: false,
            resume_after_rewind: false,
            last_held_frame_advance: Instant::now(),
            frame_advance_hold_started: None,
            frame_advance_held: false,
            frame_advance_repeated: false,
            status: "Choose a game from the library or open a ROM".into(),
            audio,
            audio_error,
            audio_scratch: Vec::with_capacity(1_024),
            gamepads,
            gamepad_error,
            last_gamepad_activity: None,
            binding_capture: None,
            achievements,
            show_achievements: false,
            achievement_password: String::new(),
            achievement_feed: VecDeque::new(),
            achievement_panel: AchievementPanel::CurrentSet,
            achievement_filter: AchievementFilter::Locked,
            achievement_archive: AchievementArchive::load(),
            achievement_badges: HashMap::new(),
            achievement_badges_requested: HashSet::new(),
            achievement_toasts: VecDeque::new(),
            achievement_known_unlocked: HashSet::new(),
            achievement_game_mastered: false,
            settings,
            active_play_mode: play_mode,
            per_game: PerGameSettings::default(),
            settings_dirty: false,
            settings_category: SettingsCategory::General,
            library,
            library_search: String::new(),
            library_sort: LibrarySort::Title,
            library_cover_textures: HashMap::new(),
            library_artwork,
            library_rename_path: None,
            library_rename_text: String::new(),
            page: if initial.is_some() {
                MainPage::Game
            } else {
                MainPage::Library
            },
            show_states: false,
            show_time: false,
            show_tas: false,
            show_tas_control: false,
            show_input: false,
            show_av: false,
            show_debugger: false,
            show_hex: false,
            show_cheats: false,
            show_settings: false,
            selected_slot: 0,
            state_slots: Vec::new(),
            state_preview: None,
            preview_slot: None,
            tas: tas_manager,
            tas_held_input: TasFrame::default(),
            tas_timeline_scroll: None,
            tas_control_movie: None,
            tas_control_selected: 0,
            tas_control_scroll: None,
            tas_control_start: TasStartType::PowerOn,
            tas_control_include_cheats: true,
            tas_control_fceux_timing: false,
            tas_control_status: "Open an external TAS movie to inspect its inputs".into(),
            rewind: VecDeque::new(),
            rewind_compressor: RewindCompressor::new(),
            rewind_generation: 0,
            lag_frames: 0,
            last_controller_reads: 0,
            hex_space: MemorySpace::CpuRam,
            hex_start: 0,
            hex_jump: String::new(),
            hex_selected: None,
            hex_value: String::new(),
            cheat_name: String::new(),
            cheat_code: String::new(),
            cheat_error: None,
            cheat_flash: Vec::new(),
            fds_swap_was_down: false,
        };
        if play_mode == PlayMode::Achievement {
            app.start_achievement_session();
        }
        app.queue_library_artwork();
        if let Some(path) = initial
            && let Err(error) = app.load_rom(path)
        {
            app.status = format!("Could not reopen ROM: {error}");
            app.page = MainPage::Library;
        }
        if app.fullscreen {
            cc.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
        Ok(app)
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.emulate(ctx);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // UI is drawn after emulation logic. Rebuild this flag from every visible
        // hold button so two instances (top bar and TAS window) cannot cancel one another.
        self.frame_advance_held = false;
        self.top_bar(ui);
        self.status_bar(ui);
        self.central(ui);
        self.feature_windows(ui);
        if self.binding_capture.is_some()
            && !self.show_input
            && !(self.show_settings && self.settings_category == SettingsCategory::Input)
        {
            self.binding_capture = None;
        }
        if !ui.ctx().input(|input| input.pointer.any_down()) {
            self.frame_advance_repeated = false;
            self.frame_advance_hold_started = None;
        }
    }
    fn on_exit(&mut self) {
        if self.play_mode() == PlayMode::Standard && self.settings.save_states.autosave_on_exit {
            self.quick_save();
        }
        let _ = self.save_battery();
        let _ = settings::save(&self.settings);
    }
}
impl Drop for App {
    fn drop(&mut self) {
        let _ = self.save_battery();
        let _ = settings::save(&self.settings);
    }
}

fn floating_window_max_size(ctx: &egui::Context) -> Vec2 {
    let available = ctx.content_rect().size();
    Vec2::new(
        (available.x - 24.0).max(280.0),
        (available.y - 24.0).max(180.0),
    )
}

fn speed_ui(ui: &mut egui::Ui, index: &mut usize) -> bool {
    let old = *index;
    ui.horizontal_wrapped(|ui| {
        for (i, speed) in SPEEDS.iter().enumerate() {
            ui.selectable_value(index, i, format!("{speed}x"));
        }
    });
    *index = (*index).min(SPEEDS.len() - 1);
    old != *index
}

fn format_bytes(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    if bytes as f64 >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB)
    } else if bytes as f64 >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB)
    } else {
        format!("{bytes} B")
    }
}
