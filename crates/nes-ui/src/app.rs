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
    ApuChannel, Button, FRAME_HEIGHT, FRAME_WIDTH, MemorySpace, NTSC_2C02_PALETTE, NTSC_FRAME_RATE,
    Nes, OutputPalette, RGB_2C03_PALETTE, Region,
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
        self, CrtMask, CrtProfile, GamepadBinding, KeyBinding, PaletteMode, PerGameSettings,
        PlayMode, Settings, VideoSettings,
    },
    tas::{self, TasEditor, TasFrame, TasManager, TasMode, TasMovie, TasStartType},
    tas_control::{self, ControlMovie},
};

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

#[derive(Clone)]
struct AchievementToast {
    title: String,
    description: String,
    points: u32,
    badge_url: String,
    started_at: Option<Instant>,
}

struct RewindPoint {
    compressed_machine: Vec<u8>,
    uncompressed_len: usize,
    generation: u64,
    tas_cursor: usize,
    lag_frames: u64,
    controller_reads: u64,
}

struct RewindCapture {
    machine: Vec<u8>,
    generation: u64,
    tas_cursor: usize,
    lag_frames: u64,
    controller_reads: u64,
}

impl RewindPoint {
    fn compress(capture: RewindCapture) -> Self {
        Self {
            compressed_machine: compress_prepend_size(&capture.machine),
            uncompressed_len: capture.machine.len(),
            generation: capture.generation,
            tas_cursor: capture.tas_cursor,
            lag_frames: capture.lag_frames,
            controller_reads: capture.controller_reads,
        }
    }

    fn decompress(&self) -> Result<Vec<u8>, lz4_flex::block::DecompressError> {
        decompress_size_prepended(&self.compressed_machine)
    }
}

struct RewindCompressor {
    captures: SyncSender<RewindCapture>,
    points: Receiver<RewindPoint>,
}

impl RewindCompressor {
    fn new() -> Self {
        // A short bounded queue prevents compression from ever building an
        // unbounded latency or memory backlog behind live emulation.
        let (capture_tx, capture_rx) = mpsc::sync_channel::<RewindCapture>(2);
        let (point_tx, point_rx) = mpsc::channel();
        thread::Builder::new()
            .name("rewind-compressor".into())
            .spawn(move || {
                while let Ok(capture) = capture_rx.recv() {
                    if point_tx.send(RewindPoint::compress(capture)).is_err() {
                        break;
                    }
                }
            })
            .expect("could not start rewind compression worker");
        Self {
            captures: capture_tx,
            points: point_rx,
        }
    }

    fn submit(&self, capture: RewindCapture) {
        match self.captures.try_send(capture) {
            Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
        }
    }
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

    fn emulate(&mut self, ctx: &egui::Context) {
        self.collect_library_artwork();
        if self.library_artwork.has_pending() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.play_mode() == PlayMode::Achievement {
            let events = self.achievements.pump(self.paused || !self.powered);
            self.handle_achievement_events(events);
            self.collect_achievement_badges(ctx);
        }
        self.collect_compressed_rewind_points();
        self.poll_input_devices(ctx);
        self.handle_hotkeys(ctx);
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick).as_secs_f64().min(0.1);
        self.last_tick = now;
        let speed = self.current_speed();
        let frame_rate = self.emulation_frame_rate();
        let held_frame_advance =
            self.frame_advance_held && ctx.input(|input| input.pointer.any_down()) && self.powered;
        let rewind_held = self.play_mode() == PlayMode::Standard
            && !self.key_is_controller_binding(Key::Backspace)
            && !ctx.egui_wants_keyboard_input()
            && ctx.input(|input| input.key_down(Key::Backspace))
            && self.powered
            && self.nes.is_some();
        self.update_continuous_rewind(rewind_held);
        if rewind_held {
            self.frame_budget = 0.0;
            ctx.request_repaint_after(Duration::from_millis(1));
        } else if held_frame_advance {
            self.frame_budget = 0.0;
            let interval = Duration::from_secs_f64(1.0 / frame_rate);
            let repeating = self
                .frame_advance_hold_started
                .is_some_and(|started| started.elapsed() >= Duration::from_millis(275));
            if repeating && self.last_held_frame_advance.elapsed() >= interval {
                self.last_held_frame_advance = Instant::now();
                self.frame_advance_repeated = true;
                self.advance_frame(ctx);
            }
            ctx.request_repaint_after(Duration::from_millis(1));
        } else if !self.paused && self.powered && self.nes.is_some() {
            let coin = self.vs_coin_down(ctx);
            if let Some(controller) = self.nes.as_mut().and_then(|nes| nes.controller_mut(0)) {
                controller.set_coin(coin);
            }
            self.frame_budget += elapsed * frame_rate * speed;
            let mut frames = 0;
            let pacing_tolerance = if speed == 1.0 {
                NORMAL_SPEED_FRAME_TOLERANCE
            } else {
                0.0
            };
            while self.frame_budget + pacing_tolerance >= 1.0 && frames < 8 {
                let live = self.host_input_frame(ctx);
                if !self.run_one_frame(live, speed == 1.0) {
                    break;
                }
                self.frame_budget -= 1.0;
                frames += 1;
            }
            if frames == 8 {
                self.frame_budget = self.frame_budget.min(1.0);
            }
        }
        let fps_elapsed = self.fps_window_start.elapsed();
        if fps_elapsed >= Duration::from_secs(2) {
            let sample = self.presented_frames_in_window as f64 / fps_elapsed.as_secs_f64();
            self.measured_fps = if self.measured_fps == 0.0 {
                sample
            } else {
                self.measured_fps * 0.35 + sample * 0.65
            };
            self.presented_frames_in_window = 0;
            self.fps_window_start = Instant::now();
        }
        let volume = self.effective_volume();
        let muted = self.effective_muted() || self.paused || !self.powered || speed != 1.0;
        if let Some(audio) = &mut self.audio {
            audio.set_volume(volume);
            audio.set_muted(muted);
            audio.set_reference_mastering(self.settings.audio.soft_clip);
            if speed != 1.0 {
                audio.clear();
            }
        }
        if self.settings_dirty {
            if let Err(error) = settings::save(&self.settings) {
                self.status = format!("Could not save settings: {error}");
            }
            self.settings_dirty = false;
        }
        ctx.request_repaint_after(Duration::from_millis(2));
    }

    fn run_one_frame(&mut self, live_input: TasFrame, present_audio: bool) -> bool {
        let achievement_mode = self.play_mode() == PlayMode::Achievement;
        let interval = self.settings.emulation.rewind_interval_frames.max(1);
        if self.play_mode() == PlayMode::Standard
            && let Some(nes) = &self.nes
            && nes.frame().number.is_multiple_of(interval)
            && let Ok(machine) = nes.save_state()
        {
            self.rewind_compressor.submit(RewindCapture {
                machine,
                generation: self.rewind_generation,
                tas_cursor: self.tas.cursor,
                lag_frames: self.lag_frames,
                controller_reads: self.last_controller_reads,
            });
        }
        if self.play_mode() == PlayMode::Standard
            && self.tas.mode != TasMode::Inactive
            && self
                .tas
                .cursor
                .is_multiple_of(self.tas.checkpoint_interval.max(1))
            && let Some(nes) = &self.nes
            && let Ok(state) = nes.save_state()
        {
            let frame = self.tas.cursor;
            if self.tas.maybe_checkpoint(frame, state) {
                match self.reconcile_tas_checkpoint(frame) {
                    TasCheckpointRecovery::RefreshedChecksum => {
                        self.status = format!(
                            "Refreshed stale TAS checkpoint at frame {frame}; save the movie to persist it"
                        );
                    }
                    TasCheckpointRecovery::Resynchronized => {
                        self.status =
                            format!("TAS playback resynchronized automatically at frame {frame}");
                    }
                    TasCheckpointRecovery::Unrecoverable => {
                        self.status = self.tas.last_desync.clone().unwrap_or_default();
                        self.tas.pause();
                        self.paused = true;
                        self.follow_tas_cursor();
                        return false;
                    }
                }
            }
        }
        let Some(input) = self.tas.input_for_frame(live_input) else {
            self.tas.stop();
            if let Some(nes) = &mut self.nes {
                set_controller_mask(nes, 0, 0);
                set_controller_mask(nes, 1, 0);
            }
            if self.settings.tas.pause_when_playback_ends {
                self.paused = true;
            }
            self.follow_tas_cursor();
            self.status = "TAS playback complete".into();
            return false;
        };
        if let Some(nes) = &mut self.nes {
            set_controller_mask(nes, 0, input.player1);
            set_controller_mask(nes, 1, input.player2);
            let reads_before = nes
                .controller_reads(0)
                .wrapping_add(nes.controller_reads(1));
            if let Err(error) = nes.run_frame() {
                self.paused = true;
                self.status = format!("Emulation stopped: {error}");
                return false;
            }
            let reads_after = nes
                .controller_reads(0)
                .wrapping_add(nes.controller_reads(1));
            if reads_after == reads_before {
                self.lag_frames = self.lag_frames.wrapping_add(1);
            }
            self.last_controller_reads = reads_after;
            self.audio_scratch.clear();
            nes.drain_audio_samples(&mut self.audio_scratch);
            if achievement_mode {
                self.achievements.do_frame(nes);
            }
        }
        if present_audio && let Some(audio) = &mut self.audio {
            audio.push(&self.audio_scratch);
        }
        self.frame_dirty = true;
        self.presented_frames_in_window = self.presented_frames_in_window.wrapping_add(1);
        if self.tas.mode != TasMode::Inactive {
            self.follow_tas_cursor();
        }
        true
    }

    fn handle_hotkeys(&mut self, ctx: &egui::Context) {
        if self.binding_capture.is_some() {
            return;
        }
        let (ctrl, shift) = ctx.input(|input| (input.modifiers.ctrl, input.modifiers.shift));
        if ctrl && ctx.input(|i| i.key_pressed(Key::O)) {
            self.open_rom_dialog();
        }
        if self.show_settings && ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.show_settings = false;
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let assists_allowed = self.play_mode() == PlayMode::Standard;
        if assists_allowed && self.hotkey_pressed(ctx, Key::Space) {
            self.toggle_pause();
        }
        if self.hotkey_pressed(ctx, Key::R) {
            self.reset();
        }
        if self.hotkey_pressed(ctx, Key::P) {
            if ctrl {
                self.power_cycle();
            } else {
                self.toggle_power();
            }
        }
        if assists_allowed && self.hotkey_pressed(ctx, Key::F5) {
            self.quick_save();
        }
        if assists_allowed && self.hotkey_pressed(ctx, Key::F8) {
            self.quick_load();
        }
        if assists_allowed && self.hotkey_pressed(ctx, Key::N) {
            self.advance_frame(ctx);
        }
        if self.hotkey_pressed(ctx, Key::F11) {
            self.toggle_fullscreen(ctx);
        }
        if self.hotkey_pressed(ctx, Key::F12) {
            self.take_screenshot();
        }
        if assists_allowed && self.hotkey_pressed(ctx, Key::Num0) {
            self.speed_index = NORMAL_SPEED_INDEX;
        }
        if assists_allowed && self.hotkey_pressed(ctx, Key::F1) {
            self.show_debugger = true;
        }
        if assists_allowed && self.hotkey_pressed(ctx, Key::F2) {
            self.show_hex = true;
            self.paused = true;
        }
        self.fast_forward = assists_allowed
            && !self.key_is_controller_binding(Key::Tab)
            && ctx.input(|i| i.key_down(Key::Tab));
        if assists_allowed && shift && self.hotkey_pressed(ctx, Key::F5) {
            self.show_states = true;
        }
    }

    fn hotkey_pressed(&self, ctx: &egui::Context, key: Key) -> bool {
        !self.key_is_controller_binding(key) && ctx.input(|input| input.key_pressed(key))
    }

    fn key_is_controller_binding(&self, key: Key) -> bool {
        let name = key.name();
        self.settings.input.vs_coin_binding.label() == name
            || self
                .settings
                .input
                .bindings
                .iter()
                .chain(&self.settings.input.player2_bindings)
                .any(|binding| binding.label() == name)
    }

    fn host_input_frame(&self, ctx: &egui::Context) -> TasFrame {
        if self.binding_capture.is_some() {
            return TasFrame::default();
        }
        if ctx.egui_wants_keyboard_input() {
            return self.gamepad_input_frame();
        }
        self.bound_input_frame(ctx)
    }

    fn bound_input_frame(&self, ctx: &egui::Context) -> TasFrame {
        let gamepad = self.gamepad_input_frame();
        self.filter_live_dpad(TasFrame {
            player1: binding_mask(ctx, &self.settings.input.bindings) | gamepad.player1,
            player2: binding_mask(ctx, &self.settings.input.player2_bindings) | gamepad.player2,
        })
    }

    fn gamepad_input_frame(&self) -> TasFrame {
        self.filter_live_dpad(TasFrame {
            player1: self.gamepad_mask(0, &self.settings.input.gamepad_bindings),
            player2: self.gamepad_mask(1, &self.settings.input.player2_gamepad_bindings),
        })
    }

    fn vs_coin_down(&self, ctx: &egui::Context) -> bool {
        if self.binding_capture.is_some() || ctx.egui_wants_keyboard_input() {
            return false;
        }
        let keyboard = binding_down(ctx, &self.settings.input.vs_coin_binding);
        keyboard || self.vs_coin_gamepad_down()
    }

    fn vs_coin_gamepad_down(&self) -> bool {
        let Some(gamepads) = &self.gamepads else {
            return false;
        };
        let slot = self.settings.input.gamepad_slots[0].unwrap_or(0);
        let Some((_, gamepad)) = gamepads.gamepads().nth(slot) else {
            return false;
        };
        if let Some(binding) = self.settings.input.vs_coin_gamepad_binding {
            gamepad_binding_down(
                &gamepad,
                binding,
                self.settings.input.gamepad_axis_threshold.clamp(0.1, 0.9),
            )
        } else {
            let mask = self.gamepad_mask(0, &self.settings.input.gamepad_bindings);
            mask & 0x0c == 0x0c
        }
    }

    fn filter_live_dpad(&self, input: TasFrame) -> TasFrame {
        if self.play_mode() == PlayMode::Standard && self.settings.input.allow_opposite_directions {
            input
        } else {
            TasFrame {
                player1: neutralize_opposite_directions(input.player1),
                player2: neutralize_opposite_directions(input.player2),
            }
        }
    }

    fn poll_input_devices(&mut self, ctx: &egui::Context) {
        if self.binding_capture.is_some() && ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.binding_capture = None;
            self.status = "Input binding cancelled".into();
        }
        if let Some(BindingCapture::Keyboard { player, button }) = self.binding_capture {
            let key = ctx.input(|input| {
                input.events.iter().find_map(|event| match event {
                    egui::Event::Key {
                        key,
                        physical_key,
                        pressed: true,
                        repeat: false,
                        ..
                    } => Some(physical_key.unwrap_or(*key)),
                    _ => None,
                })
            });
            if let Some(key) = key {
                let binding = KeyBinding::new(key.name());
                let label = binding.label().to_owned();
                if player == 0 {
                    self.settings.input.bindings[button] = binding;
                } else {
                    self.settings.input.player2_bindings[button] = binding;
                }
                self.binding_capture = None;
                self.settings_dirty = true;
                self.status = format!(
                    "Player {} {} is now bound to {label}",
                    player + 1,
                    nes_button_label(button)
                );
            }
        }
        if self.binding_capture == Some(BindingCapture::VsCoinKeyboard) {
            let key = ctx.input(|input| {
                input.events.iter().find_map(|event| match event {
                    egui::Event::Key {
                        key,
                        physical_key,
                        pressed: true,
                        repeat: false,
                        ..
                    } => Some(physical_key.unwrap_or(*key)),
                    _ => None,
                })
            });
            if let Some(key) = key {
                self.settings.input.vs_coin_binding = KeyBinding::new(key.name());
                self.binding_capture = None;
                self.settings_dirty = true;
                self.status = format!("VS insert coin is now bound to {}", key.name());
            }
        }

        let mut captured = None;
        if let Some(gamepads) = &mut self.gamepads {
            while let Some(event) = gamepads.next_event() {
                let slot = gamepads.gamepads().position(|(id, _)| id == event.id);
                if let Some(description) = describe_gamepad_event(event.event) {
                    let device = gamepads
                        .connected_gamepad(event.id)
                        .map(|gamepad| gamepad.name().to_owned())
                        .unwrap_or_else(|| "disconnected controller".into());
                    self.last_gamepad_activity = Some(match slot {
                        Some(slot) => format!("#{} {device}: {description}", slot + 1),
                        None => format!("{device}: {description}"),
                    });
                }
                if let Some(
                    capture @ (BindingCapture::Gamepad { .. } | BindingCapture::VsCoinGamepad),
                ) = self.binding_capture
                {
                    let threshold = self.settings.input.gamepad_axis_threshold.clamp(0.1, 0.9);
                    let binding = match event.event {
                        EventType::ButtonPressed(gamepad_button, code) => {
                            Some(GamepadBinding::ExactButton {
                                button: gamepad_button,
                                code,
                            })
                        }
                        EventType::ButtonChanged(gamepad_button, value, code)
                            if value >= threshold =>
                        {
                            Some(GamepadBinding::ExactButton {
                                button: gamepad_button,
                                code,
                            })
                        }
                        EventType::ButtonChanged(gamepad_button, value, code) if value <= 0.05 => {
                            Some(GamepadBinding::ExactButtonLow {
                                button: gamepad_button,
                                code,
                            })
                        }
                        EventType::AxisChanged(axis, value, code) if value.abs() >= threshold => {
                            let direction = if value.is_sign_positive() { 1 } else { -1 };
                            Some(GamepadBinding::ExactAxis {
                                axis,
                                code,
                                direction,
                            })
                        }
                        EventType::AxisChanged(axis, value, code) if value.abs() <= 0.05 => {
                            Some(GamepadBinding::ExactAxisLow { axis, code })
                        }
                        _ => None,
                    };
                    if let Some(binding) = binding {
                        captured = Some((capture, binding, slot));
                        break;
                    }
                }
            }
        }
        if let Some((capture, binding, slot)) = captured {
            let status = match capture {
                BindingCapture::Gamepad { player, button } => {
                    if player == 0 {
                        self.settings.input.gamepad_bindings[button] = Some(binding);
                    } else {
                        self.settings.input.player2_gamepad_bindings[button] = Some(binding);
                    }
                    self.settings.input.gamepad_slots[player] = slot;
                    format!(
                        "Player {} {} is now bound to {}",
                        player + 1,
                        nes_button_label(button),
                        gamepad_binding_label(binding)
                    )
                }
                BindingCapture::VsCoinGamepad => {
                    self.settings.input.vs_coin_gamepad_binding = Some(binding);
                    self.settings.input.gamepad_slots[0] = slot;
                    format!(
                        "VS insert coin is now bound to {}",
                        gamepad_binding_label(binding)
                    )
                }
                _ => unreachable!("only gamepad captures are queued here"),
            };
            self.binding_capture = None;
            self.settings_dirty = true;
            self.status = status;
        }
    }

    fn gamepad_mask(&self, player: usize, bindings: &[Option<GamepadBinding>; 8]) -> u8 {
        let Some(gamepads) = &self.gamepads else {
            return 0;
        };
        let slot = self.settings.input.gamepad_slots[player].unwrap_or(player);
        let Some((_, gamepad)) = gamepads.gamepads().nth(slot) else {
            return 0;
        };
        bindings
            .iter()
            .enumerate()
            .fold(0, |mask, (index, binding)| {
                mask | (u8::from(binding.is_some_and(|binding| {
                    gamepad_binding_down(
                        &gamepad,
                        binding,
                        self.settings.input.gamepad_axis_threshold.clamp(0.1, 0.9),
                    )
                })) << index)
            })
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        enum MenuAction {
            OpenRom,
            LoadRom(PathBuf),
            TogglePause,
            FrameAdvance,
            RewindFrame,
            QuickSave,
            QuickLoad,
            Reset,
            PowerCycle,
            TogglePower,
            ToggleFullscreen,
            Exit,
        }

        let mut action = None;
        let assists_allowed = self.play_mode() == PlayMode::Standard;
        egui::Panel::top("top-bar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open ROM…    Ctrl+O").clicked() {
                        action = Some(MenuAction::OpenRom);
                        ui.close();
                    }
                    if ui.button("ROM Library").clicked() {
                        self.page = MainPage::Library;
                        ui.close();
                    }
                    ui.menu_button("Recent Games", |ui| {
                        let recent = self.library.recent();
                        if recent.is_empty() {
                            ui.add_enabled(false, egui::Button::new("No recent games"));
                        }
                        for entry in recent.into_iter().take(10) {
                            let ready = matches!(&entry.status, EntryStatus::Ready);
                            let response = ui
                                .add_enabled(ready, egui::Button::new(&entry.title))
                                .on_hover_text(entry.path.display().to_string());
                            if response.clicked() {
                                action = Some(MenuAction::LoadRom(entry.path.clone()));
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("Exit    Alt+F4").clicked() {
                        action = Some(MenuAction::Exit);
                        ui.close();
                    }
                });

                ui.menu_button("Emulation", |ui| {
                    if assists_allowed {
                        let pause_label = if self.paused {
                            "Resume    Space"
                        } else {
                            "Pause    Space"
                        };
                        if ui
                            .add_enabled(self.nes.is_some(), egui::Button::new(pause_label))
                            .clicked()
                        {
                            action = Some(MenuAction::TogglePause);
                            ui.close();
                        }
                        if ui
                            .add_enabled(self.powered, egui::Button::new("Frame Advance    N"))
                            .clicked()
                        {
                            action = Some(MenuAction::FrameAdvance);
                            ui.close();
                        }
                        let can_rewind = if self.tas.movie.is_some() {
                            self.tas.cursor > 0
                        } else {
                            !self.rewind.is_empty()
                        };
                        if ui
                            .add_enabled(
                                can_rewind,
                                egui::Button::new("Rewind One Step    Backspace"),
                            )
                            .clicked()
                        {
                            action = Some(MenuAction::RewindFrame);
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(self.nes.is_some(), egui::Button::new("Quick Save    F5"))
                            .clicked()
                        {
                            action = Some(MenuAction::QuickSave);
                            ui.close();
                        }
                        if ui
                            .add_enabled(self.nes.is_some(), egui::Button::new("Quick Load    F8"))
                            .clicked()
                        {
                            action = Some(MenuAction::QuickLoad);
                            ui.close();
                        }
                        ui.separator();
                    }
                    if ui
                        .add_enabled(self.nes.is_some(), egui::Button::new("Reset    R"))
                        .clicked()
                    {
                        action = Some(MenuAction::Reset);
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            self.nes.is_some(),
                            egui::Button::new("Restart / Power Cycle    Ctrl+P"),
                        )
                        .clicked()
                    {
                        action = Some(MenuAction::PowerCycle);
                        ui.close();
                    }
                    let power_label = if self.powered {
                        "Power Off    P"
                    } else {
                        "Power On    P"
                    };
                    if ui
                        .add_enabled(self.nes.is_some(), egui::Button::new(power_label))
                        .clicked()
                    {
                        action = Some(MenuAction::TogglePower);
                        ui.close();
                    }
                });

                ui.menu_button("View", |ui| {
                    if ui
                        .selectable_label(self.page == MainPage::Game, "Game Display")
                        .clicked()
                    {
                        self.page = MainPage::Game;
                        ui.close();
                    }
                    if ui
                        .selectable_label(self.page == MainPage::Library, "ROM Library")
                        .clicked()
                    {
                        self.page = MainPage::Library;
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .selectable_label(self.fullscreen, "Fullscreen    F11")
                        .clicked()
                    {
                        action = Some(MenuAction::ToggleFullscreen);
                        ui.close();
                    }
                    if ui
                        .checkbox(&mut self.settings.video.show_fps, "Show FPS")
                        .changed()
                    {
                        self.settings_dirty = true;
                    }
                });

                ui.menu_button("Config", |ui| {
                    if ui.button("Settings…").clicked() {
                        self.show_settings = true;
                        ui.close();
                    }
                    if ui.button("Input Configuration…").clicked() {
                        self.show_input = true;
                        ui.close();
                    }
                    if ui.button("Audio / Video…").clicked() {
                        self.show_av = true;
                        ui.close();
                    }
                });

                if assists_allowed {
                    ui.menu_button("Tools", |ui| {
                        if ui.button("Save States…").clicked() {
                            self.show_states = true;
                            ui.close();
                        }
                        if ui.button("Rewind & Speed…").clicked() {
                            self.show_time = true;
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("TAS Editor…").clicked() {
                            self.show_tas = true;
                            ui.close();
                        }
                        if ui.button("TAS Control View…").clicked() {
                            self.show_tas_control = true;
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Debugger…    F1").clicked() {
                            self.show_debugger = true;
                            ui.close();
                        }
                        if ui.button("Hex Editor…    F2").clicked() {
                            self.show_hex = true;
                            self.paused = true;
                            ui.close();
                        }
                    });
                }
                if self.play_mode() == PlayMode::Achievement && ui.button("Achievements").clicked()
                {
                    self.show_achievements = true;
                }

                ui.separator();
                ui.selectable_value(&mut self.page, MainPage::Game, "Game");
                ui.selectable_value(&mut self.page, MainPage::Library, "Library");
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(if self.paused { "Paused" } else { "Running" });
                ui.separator();
                ui.label(self.play_mode().label());
                ui.separator();
                let frame = self.nes.as_ref().map_or(0, |nes| nes.frame().number);
                ui.label(format!("Frame {frame} · Lag {}", self.lag_frames));
                if let Some(nes) = &self.nes {
                    ui.separator();
                    let region = match nes.region() {
                        Region::Ntsc => "NTSC",
                        Region::Pal => "PAL",
                    };
                    ui.label(format!("{region} · {:.2} Hz", nes.frame_rate()));
                }
                if self.settings.video.show_fps {
                    ui.separator();
                    ui.label(format!("{:.1} FPS", self.measured_fps));
                }
            });
        });

        match action {
            Some(MenuAction::OpenRom) => self.open_rom_dialog(),
            Some(MenuAction::LoadRom(path)) => self.try_load_rom(path),
            Some(MenuAction::TogglePause) => self.toggle_pause(),
            Some(MenuAction::FrameAdvance) => self.advance_frame(ui.ctx()),
            Some(MenuAction::RewindFrame) => self.rewind_step(),
            Some(MenuAction::QuickSave) => self.quick_save(),
            Some(MenuAction::QuickLoad) => self.quick_load(),
            Some(MenuAction::Reset) => self.reset(),
            Some(MenuAction::PowerCycle) => self.power_cycle(),
            Some(MenuAction::TogglePower) => self.toggle_power(),
            Some(MenuAction::ToggleFullscreen) => self.toggle_fullscreen(ui.ctx()),
            Some(MenuAction::Exit) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            None => {}
        }
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        match self.page {
            MainPage::Game => self.game_page(ui),
            MainPage::Library => self.library_page(ui),
        }
    }

    fn game_page(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            if self.frame_dirty
                && let Some(nes) = &self.nes
            {
                if self.settings.video.crt_enabled {
                    let image = self.crt_renderer.render(
                        &nes.frame().pixels,
                        CrtParameters {
                            profile: self.settings.video.crt_profile,
                            mask: self.settings.video.crt_mask,
                            scanline_strength: self.settings.video.crt_scanline_strength,
                            mask_strength: self.settings.video.crt_mask_strength,
                            bloom_strength: self.settings.video.crt_bloom_strength,
                            curvature: self.settings.video.crt_curvature,
                            halation_strength: self.settings.video.crt_halation_strength,
                            diffusion_strength: self.settings.video.crt_diffusion_strength,
                            convergence: self.settings.video.crt_convergence,
                        },
                    );
                    self.texture.set(image, TextureOptions::LINEAR);
                } else {
                    self.texture.set(
                        ColorImage::from_rgb([FRAME_WIDTH, FRAME_HEIGHT], &nes.frame().pixels),
                        TextureOptions::NEAREST,
                    );
                }
                self.frame_dirty = false;
            }
            if self.nes.is_none() {
                ui.vertical_centered(|ui| {
                    ui.heading("No ROM loaded");
                    if ui.button("Open Library").clicked() {
                        self.page = MainPage::Library;
                    }
                });
                return;
            }
            let available = ui.available_size();
            let aspect = FRAME_WIDTH as f32 / FRAME_HEIGHT as f32;
            let mut size = Vec2::new(available.x, available.x / aspect);
            if size.y > available.y {
                size = Vec2::new(available.y * aspect, available.y);
            }
            if self.settings.video.integer_scaling {
                let available_scale =
                    (size.x / FRAME_WIDTH as f32).min(size.y / FRAME_HEIGHT as f32);
                let scale = if available_scale >= 1.0 {
                    available_scale.floor()
                } else {
                    // A fractional fallback keeps the complete frame visible when the
                    // native resolution cannot fit in a small or snapped window.
                    available_scale.max(0.01)
                };
                size = Vec2::new(FRAME_WIDTH as f32 * scale, FRAME_HEIGHT as f32 * scale);
            }
            ui.vertical_centered(|ui| {
                ui.add(egui::Image::new(&self.texture).fit_to_exact_size(size));
            });
        });
        self.achievement_toast_overlay(ui.ctx());
    }

    fn library_page(&mut self, ui: &mut egui::Ui) {
        enum LibraryAction {
            Launch(PathBuf),
            Rename(PathBuf, String),
            ChooseCover(PathBuf),
            RemoveCover(PathBuf),
            Remove(PathBuf),
        }

        let mut action = None;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("Library");
                if ui.button("Open ROM…").clicked() {
                    self.open_rom_dialog();
                }
                if ui.button("Choose ROM folder…").clicked()
                    && let Some(folder) = FileDialog::new()
                        .set_directory(&self.settings.paths.rom_folder)
                        .pick_folder()
                {
                    self.settings.paths.rom_folder = folder;
                    self.settings_dirty = true;
                    self.refresh_library_and_artwork();
                }
                if ui.button("Refresh").clicked() {
                    self.refresh_library_and_artwork();
                }
                if self.library_artwork.has_pending() {
                    ui.spinner();
                    ui.small("Fetching artwork…");
                }
                ui.separator();
                ui.add(
                    egui::TextEdit::singleline(&mut self.library_search)
                        .hint_text("Search games…")
                        .desired_width(180.0),
                );
                egui::ComboBox::from_label("Sort")
                    .selected_text(match self.library_sort {
                        LibrarySort::Title => "Title",
                        LibrarySort::Recent => "Recently played",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.library_sort, LibrarySort::Title, "Title");
                        ui.selectable_value(
                            &mut self.library_sort,
                            LibrarySort::Recent,
                            "Recently played",
                        );
                    });
            });
            let query = self.library_search.to_lowercase();
            let mut entries: Vec<LibraryEntry> = self
                .library
                .entries
                .iter()
                .filter(|entry| {
                    query.is_empty()
                        || entry.title.to_lowercase().contains(&query)
                        || entry.file_name.to_lowercase().contains(&query)
                })
                .cloned()
                .collect();
            match self.library_sort {
                LibrarySort::Title => entries.sort_by_key(|e| e.title.to_lowercase()),
                LibrarySort::Recent => entries.sort_by_key(|e| std::cmp::Reverse(e.last_played)),
            }
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                if entries.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(40.0);
                        ui.heading("No games found");
                        ui.label("Open a ROM or choose a folder to add games.");
                    });
                }
                for entry in entries {
                    ui.group(|ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let cover = entry
                                .cover_image
                                .as_ref()
                                .and_then(|path| self.library_cover_texture(ui.ctx(), path));
                            if let Some(cover) = cover {
                                ui.add(
                                    egui::Image::new(&cover)
                                        .fit_to_exact_size(Vec2::new(72.0, 96.0)),
                                );
                            } else {
                                let (rect, _) = ui.allocate_exact_size(
                                    Vec2::new(72.0, 96.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter()
                                    .rect_filled(rect, 5.0, ui.visuals().faint_bg_color);
                                ui.painter().text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "NES",
                                    egui::FontId::proportional(18.0),
                                    ui.visuals().weak_text_color(),
                                );
                            }
                            ui.add_space(8.0);
                            ui.vertical(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&entry.title).strong().size(18.0),
                                    )
                                    .wrap(),
                                );
                                if entry.cover_source == Some(CoverSource::RetroAchievements) {
                                    ui.small("Artwork from RetroAchievements");
                                } else if entry.cover_source == Some(CoverSource::Custom) {
                                    ui.small("Custom artwork");
                                }
                                if let Some(accuracy) = &entry.accuracy {
                                    ui.add_space(6.0);
                                    ui.small("Estimated ROM compatibility (AccuracyCoin-weighted)");
                                    let color = match accuracy.score {
                                        90..=u8::MAX => egui::Color32::from_rgb(72, 184, 104),
                                        80..=89 => egui::Color32::from_rgb(64, 155, 210),
                                        70..=79 => egui::Color32::from_rgb(216, 170, 55),
                                        _ => egui::Color32::from_rgb(220, 112, 52),
                                    };
                                    ui.add(
                                        egui::ProgressBar::new(f32::from(accuracy.score) / 100.0)
                                            .desired_width(ui.available_width().clamp(130.0, 220.0))
                                            .fill(color)
                                            .text(format!(
                                                "{}% — {}",
                                                accuracy.score, accuracy.rating
                                            )),
                                    )
                                    .on_hover_ui(|ui| {
                                        ui.set_max_width(380.0);
                                        ui.strong("AccuracyCoin-weighted ROM estimate");
                                        ui.label(
                                            "This follows likely code from the ROM's reset, NMI, and IRQ vectors, detects the hardware features that code touches, weights the measured AccuracyCoin results to those features, and then applies mapper and header risk. It is a static estimate, not an automated playthrough.",
                                        );
                                        ui.add_space(4.0);
                                        ui.strong(format!(
                                            "Selected mapper coverage: {}",
                                            accuracy.mapper_coverage
                                        ));
                                        ui.add_space(4.0);
                                        for detail in &accuracy.details {
                                            ui.label(format!("- {detail}"));
                                        }
                                    });
                                    ui.small(format!(
                                        "AccuracyCoin: {}/{} passed · Mapper coverage: {}",
                                        accuracy.passed,
                                        accuracy.total,
                                        accuracy.mapper_coverage
                                    ));
                                }
                                ui.add_space(8.0);
                                ui.horizontal_wrapped(|ui| {
                                    let (ready, unavailable) = match &entry.status {
                                        EntryStatus::Ready => (true, String::new()),
                                        EntryStatus::Missing => {
                                            (false, "ROM file was moved or deleted".to_owned())
                                        }
                                        EntryStatus::Invalid(error) => {
                                            (false, format!("Invalid ROM: {error}"))
                                        }
                                    };
                                    let play = ui
                                        .add_enabled(ready, egui::Button::new("▶ Play"))
                                        .on_disabled_hover_text(unavailable);
                                    if play.clicked() {
                                        action = Some(LibraryAction::Launch(entry.path.clone()));
                                    }
                                    ui.menu_button("...", |ui| {
                                        if ui.button("Rename…").clicked() {
                                            action = Some(LibraryAction::Rename(
                                                entry.path.clone(),
                                                entry.title.clone(),
                                            ));
                                            ui.close();
                                        }
                                        if ui.button("Set Custom Artwork…").clicked() {
                                            action = Some(LibraryAction::ChooseCover(
                                                entry.path.clone(),
                                            ));
                                            ui.close();
                                        }
                                        if entry.cover_source == Some(CoverSource::Custom)
                                            && ui.button("Remove Custom Cover").clicked()
                                        {
                                            action = Some(LibraryAction::RemoveCover(
                                                entry.path.clone(),
                                            ));
                                            ui.close();
                                        }
                                        ui.separator();
                                        if ui.button("Remove from Library").clicked() {
                                            action =
                                                Some(LibraryAction::Remove(entry.path.clone()));
                                            ui.close();
                                        }
                                    });
                                });
                            });
                        })
                    });
                    ui.add_space(4.0);
                }
            });
        });

        match action {
            Some(LibraryAction::Launch(path)) => self.try_load_rom(path),
            Some(LibraryAction::Rename(path, title)) => {
                self.library_rename_path = Some(path);
                self.library_rename_text = title;
            }
            Some(LibraryAction::ChooseCover(path)) => {
                if let Some(image_path) = FileDialog::new()
                    .set_title("Choose custom game artwork")
                    .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp"])
                    .pick_file()
                {
                    match decode_cover_image(&image_path).and_then(|_| {
                        self.library
                            .set_cover_image(&path, &image_path)
                            .map_err(|error| error.to_string())
                    }) {
                        Ok(_) => {
                            self.library_cover_textures.clear();
                            self.status = "Custom library artwork updated".into();
                        }
                        Err(error) => {
                            self.status = format!("Could not use cover image: {error}");
                        }
                    }
                }
            }
            Some(LibraryAction::RemoveCover(path)) => {
                match self.library.remove_cover_image(&path) {
                    Ok(()) => {
                        self.library_cover_textures.clear();
                        self.status = "Custom artwork removed".into();
                    }
                    Err(error) => self.status = format!("Could not remove cover: {error}"),
                }
            }
            Some(LibraryAction::Remove(path)) => match self.library.forget(&path) {
                Ok(()) => {
                    self.library_cover_textures.clear();
                    self.status = "Removed game from library (ROM file was not deleted)".into();
                }
                Err(error) => self.status = format!("Could not update library: {error}"),
            },
            None => {}
        }

        if let Some(path) = self.library_rename_path.clone() {
            let mut save = false;
            let mut cancel = false;
            egui::Window::new("Rename Library Game")
                .collapsible(false)
                .resizable(false)
                .max_size(floating_window_max_size(ui.ctx()))
                .show(ui, |ui| {
                    ui.label("Library title");
                    let response = ui.text_edit_singleline(&mut self.library_rename_text);
                    if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
                        save = true;
                    }
                    ui.small("Leave blank to restore the title from the ROM file name.");
                    ui.horizontal(|ui| {
                        save |= ui.button("Save").clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
            if save {
                match self.library.rename(&path, &self.library_rename_text) {
                    Ok(()) => self.status = "Library title updated".into(),
                    Err(error) => self.status = format!("Could not rename game: {error}"),
                }
                self.library_rename_path = None;
            } else if cancel {
                self.library_rename_path = None;
            }
        }
    }

    fn library_cover_texture(&mut self, ctx: &egui::Context, path: &Path) -> Option<TextureHandle> {
        if let Some(texture) = self.library_cover_textures.get(path) {
            return Some(texture.clone());
        }
        let image = decode_cover_image(path).ok()?;
        let texture = ctx.load_texture(
            format!("library-cover:{}", path.display()),
            image,
            TextureOptions::LINEAR,
        );
        self.library_cover_textures
            .insert(path.to_path_buf(), texture.clone());
        Some(texture)
    }

    fn refresh_library_and_artwork(&mut self) {
        let folder = self.settings.paths.rom_folder.clone();
        self.library.refresh(Some(&folder));
        self.queue_library_artwork();
    }

    fn queue_library_artwork(&mut self) {
        let paths: Vec<_> = self
            .library
            .entries
            .iter()
            .filter(|entry| matches!(&entry.status, EntryStatus::Ready))
            .filter(|entry| !self.library.has_retro_cover(&entry.path))
            .map(|entry| entry.path.clone())
            .collect();
        for path in paths {
            self.library_artwork.request(path);
        }
    }

    fn collect_library_artwork(&mut self) {
        for result in self.library_artwork.take_results() {
            let Some(image) = result.image else {
                continue;
            };
            if self
                .library
                .set_retro_cover_image(&result.path, image.size, &image.rgba)
                .is_ok()
            {
                self.library_cover_textures.clear();
            }
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(&self.status);
                ui.separator();
                ui.label(format!("{}x", self.current_speed()));
                ui.separator();
                ui.label(if self.paused { "Paused" } else { "Running" });
                if let Some(path) = &self.rom_path {
                    ui.separator();
                    ui.label(path.file_name().and_then(|n| n.to_str()).unwrap_or("ROM"));
                }
            })
        });
    }

    fn feature_windows(&mut self, ui: &mut egui::Ui) {
        self.settings_window(ui);
        self.achievements_window(ui);
        self.input_window(ui);
        self.av_window(ui);
        if self.play_mode() == PlayMode::Standard {
            self.states_window(ui);
            self.time_window(ui);
            self.tas_window(ui);
            self.tas_control_window(ui);
            self.debugger_window(ui);
            self.hex_window(ui);
        }
    }

    fn achievements_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_achievements || self.play_mode() != PlayMode::Achievement {
            return;
        }

        let user = self.achievements.user();
        let game = self.achievements.game();
        let mut achievement_rows = if game.is_some() {
            self.achievements.achievements()
        } else {
            Vec::new()
        };
        let client_warnings: Vec<_> = achievement_rows
            .iter()
            .filter(|achievement| is_achievement_client_warning(achievement))
            .cloned()
            .collect();
        achievement_rows.retain(|achievement| !is_achievement_client_warning(achievement));
        achievement_rows.sort_by_key(|achievement| !achievement.unlocked);
        for achievement in &achievement_rows {
            let url = if achievement.unlocked {
                &achievement.badge_url
            } else {
                &achievement.badge_locked_url
            };
            self.ensure_achievement_badge(url);
        }
        let archive_entries: Vec<_> = self
            .achievement_archive
            .entries
            .iter()
            .filter(|entry| !is_archived_achievement_warning(entry))
            .cloned()
            .collect();
        for entry in &archive_entries {
            self.ensure_achievement_badge(&entry.badge_url);
        }
        let hardcore = self.achievements.is_hardcore();
        let game_loaded = self.achievements.is_game_loaded();
        let mut open = self.show_achievements;
        let mut sign_in = false;
        let mut logout = false;
        egui::Window::new("RetroAchievements")
            .open(&mut open)
            .default_width(680.0)
            .default_height(620.0)
            .min_size([320.0, 260.0])
            .max_size(floating_window_max_size(ui.ctx()))
            .resizable(true)
            .show(ui, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(27, 29, 36))
                    .corner_radius(8)
                    .inner_margin(12)
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.heading("RetroAchievements");
                            if let Some(user) = &user {
                                ui.label(format!(
                                    "{}  ·  {} hardcore points",
                                    user.display_name, user.score
                                ));
                            } else {
                                ui.label("Sign in to unlock achievements and leaderboards");
                            }
                            ui.add_space(3.0);
                            let color = if !client_warnings.is_empty() {
                                egui::Color32::from_rgb(232, 174, 61)
                            } else if hardcore && game_loaded {
                                egui::Color32::from_rgb(82, 196, 112)
                            } else {
                                egui::Color32::from_rgb(230, 174, 64)
                            };
                            ui.colored_label(
                                color,
                                if !client_warnings.is_empty() {
                                    "HARDCORE UNAVAILABLE"
                                } else if hardcore && game_loaded {
                                    "HARDCORE ACTIVE"
                                } else if hardcore {
                                    "HARDCORE READY"
                                } else {
                                    "OFFLINE"
                                },
                            );
                        });
                    });

                if user.is_some() {
                    if ui
                        .checkbox(
                            &mut self.settings.achievements.show_replayed_unlocks,
                            "Show previously earned unlock popups",
                        )
                        .changed()
                    {
                        self.settings_dirty = true;
                    }
                    ui.small(
                        "Off by default. Completed achievements remain available under All and Unlocked below.",
                    );
                }

                for warning in &client_warnings {
                    achievement_warning_banner(ui, warning);
                }

                if user.is_none() {
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Username");
                        ui.add(
                            egui::TextEdit::singleline(
                                &mut self.settings.achievements.username,
                            )
                            .desired_width(150.0),
                        );
                        ui.label("Password");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.achievement_password)
                                .password(true)
                                .desired_width(150.0),
                        );
                        if ui
                            .add_enabled(
                                !self.settings.achievements.username.trim().is_empty()
                                    && !self.achievement_password.is_empty(),
                                egui::Button::new("Sign in"),
                            )
                            .clicked()
                        {
                            sign_in = true;
                        }
                    });
                    ui.small("Your password is used only for sign-in. Only the returned token is saved locally.");
                }

                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(
                        &mut self.achievement_panel,
                        AchievementPanel::CurrentSet,
                        "Current game",
                    );
                    ui.selectable_value(
                        &mut self.achievement_panel,
                        AchievementPanel::Archive,
                        format!("Unlock archive ({})", archive_entries.len()),
                    );
                    if user.is_some() {
                        logout = ui.small_button("Sign out").clicked();
                    }
                });
                ui.separator();

                match self.achievement_panel {
                    AchievementPanel::CurrentSet => {
                        if let Some(game) = &game {
                            let unlocked = achievement_rows
                                .iter()
                                .filter(|achievement| achievement.unlocked)
                                .count();
                            let earned_points: u32 = achievement_rows
                                .iter()
                                .filter(|achievement| achievement.unlocked)
                                .map(|achievement| achievement.points)
                                .sum();
                            let total_points: u32 = achievement_rows
                                .iter()
                                .map(|achievement| achievement.points)
                                .sum();
                            ui.heading(&game.title);
                            ui.horizontal_wrapped(|ui| {
                                ui.label(format!(
                                    "{unlocked}/{} unlocked",
                                    achievement_rows.len()
                                ));
                                ui.separator();
                                ui.label(format!("{earned_points}/{total_points} points"));
                                ui.separator();
                                ui.small(format!("Game ID {}", game.id));
                            });
                            let progress = if achievement_rows.is_empty() {
                                0.0
                            } else {
                                unlocked as f32 / achievement_rows.len() as f32
                            };
                            ui.add(egui::ProgressBar::new(progress).show_percentage());
                            ui.horizontal_wrapped(|ui| {
                                ui.label("Show:");
                                ui.selectable_value(
                                    &mut self.achievement_filter,
                                    AchievementFilter::All,
                                    "All",
                                );
                                ui.selectable_value(
                                    &mut self.achievement_filter,
                                    AchievementFilter::Locked,
                                    "Locked",
                                );
                                ui.selectable_value(
                                    &mut self.achievement_filter,
                                    AchievementFilter::Unlocked,
                                    "Unlocked",
                                );
                            });
                            ui.add_space(4.0);
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                for unlocked_group in [true, false] {
                                    if matches!(
                                        (self.achievement_filter, unlocked_group),
                                        (AchievementFilter::Locked, true)
                                            | (AchievementFilter::Unlocked, false)
                                    ) {
                                        continue;
                                    }
                                    let count = achievement_rows
                                        .iter()
                                        .filter(|achievement| {
                                            achievement.unlocked == unlocked_group
                                        })
                                        .count();
                                    if count == 0 {
                                        continue;
                                    }
                                    ui.label(
                                        egui::RichText::new(if unlocked_group {
                                            format!("UNLOCKED  {count}")
                                        } else {
                                            format!("LOCKED  {count}")
                                        })
                                        .small()
                                        .strong()
                                        .color(egui::Color32::from_gray(165)),
                                    );
                                    for achievement in achievement_rows.iter().filter(
                                        |achievement| achievement.unlocked == unlocked_group,
                                    ) {
                                        let badge_url = if achievement.unlocked {
                                            &achievement.badge_url
                                        } else {
                                            &achievement.badge_locked_url
                                        };
                                        achievement_card(
                                            ui,
                                            achievement,
                                            self.achievement_badges.get(badge_url),
                                        );
                                        ui.add_space(5.0);
                                    }
                                }
                            });
                        } else if user.is_some() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(50.0);
                                ui.heading("No achievement set loaded");
                                ui.label("Load a supported NES game to see its achievements.");
                            });
                        }
                    }
                    AchievementPanel::Archive => {
                        let archived_points: u32 =
                            archive_entries.iter().map(|entry| entry.points).sum();
                        ui.heading("Unlock archive");
                        ui.label(format!(
                            "{} unlocks · {} points earned in this emulator",
                            archive_entries.len(), archived_points
                        ));
                        ui.small("This local history is kept even when you switch games.");
                        ui.add_space(6.0);
                        if archive_entries.is_empty() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(50.0);
                                ui.heading("No archived unlocks yet");
                                ui.label("Achievements earned here will appear in this list.");
                            });
                        } else {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                for entry in &archive_entries {
                                    archive_card(
                                        ui,
                                        entry,
                                        self.achievement_badges.get(&entry.badge_url),
                                    );
                                    ui.add_space(5.0);
                                }
                            });
                        }
                    }
                }
            });
        self.show_achievements = open;

        if sign_in {
            let username = self.settings.achievements.username.trim().to_owned();
            match self
                .achievements
                .login_password(&username, &self.achievement_password)
            {
                Ok(()) => self.status = "Signing in to RetroAchievements…".into(),
                Err(error) => self.status = format!("RetroAchievements sign-in failed: {error}"),
            }
        }
        if logout {
            self.achievements.logout();
            self.achievements.unload_game();
            self.settings.achievements.token.clear();
            self.achievement_password.clear();
            self.settings_dirty = true;
            self.status = "Signed out of RetroAchievements".into();
        }
    }

    fn settings_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_settings {
            return;
        }
        let mut open = self.show_settings;
        let mut close_requested = false;
        let old_play_mode = self.settings.general.play_mode;
        let old_default_speed = self.settings.emulation.speed_index;
        let old_slot_count = self.settings.save_states.slots;
        let old_palette = (
            self.settings.video.palette_mode,
            self.settings.video.custom_palette_path.clone(),
        );
        let old_crt = crt_signature(&self.settings.video);
        let mut import_palette = false;
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(true)
            .default_size([640.0, 520.0])
            .min_size([320.0, 240.0])
            .max_size(floating_window_max_size(ui.ctx()))
            .vscroll(true)
            .constrain(true)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Resize the window or scroll to reach every setting.");
                    if ui.button("Close Settings").clicked() {
                        close_requested = true;
                    }
                });
                ui.separator();
                let restricted = self.settings.general.play_mode.restricts_assists();
                if restricted
                    && matches!(
                        self.settings_category,
                        SettingsCategory::Emulation
                            | SettingsCategory::SaveStates
                            | SettingsCategory::Tas
                            | SettingsCategory::Debugging
                    )
                {
                    self.settings_category = SettingsCategory::General;
                }
                ui.horizontal_wrapped(|ui| {
                    for (cat, label) in [
                        (SettingsCategory::General, "General"),
                        (SettingsCategory::Video, "Video"),
                        (SettingsCategory::Audio, "Audio"),
                        (SettingsCategory::Input, "Input"),
                        (SettingsCategory::Emulation, "Emulation"),
                        (SettingsCategory::Paths, "Paths"),
                        (SettingsCategory::SaveStates, "Save States"),
                        (SettingsCategory::Tas, "TAS"),
                        (SettingsCategory::Debugging, "Debugging"),
                    ] {
                        if restricted
                            && matches!(
                                cat,
                                SettingsCategory::Emulation
                                    | SettingsCategory::SaveStates
                                    | SettingsCategory::Tas
                                    | SettingsCategory::Debugging
                            )
                        {
                            continue;
                        }
                        ui.selectable_value(&mut self.settings_category, cat, label);
                    }
                });
                ui.separator();
                let mut changed = false;
                match self.settings_category {
                    SettingsCategory::General => {
                        ui.strong("Play profile");
                        ui.horizontal_wrapped(|ui| {
                            for mode in [
                                PlayMode::Standard,
                                PlayMode::Speedrun,
                                PlayMode::Achievement,
                            ] {
                                changed |= ui
                                    .selectable_value(
                                        &mut self.settings.general.play_mode,
                                        mode,
                                        mode.label(),
                                    )
                                    .changed();
                            }
                        });
                        ui.label(match self.settings.general.play_mode {
                            PlayMode::Standard => {
                                "All emulator tools are available, including rewind, save states, speed controls, TAS, pause, and debugging."
                            }
                            PlayMode::Speedrun => {
                                "Clean 1x play: save states, rewind, speed controls, pause/frame advance, TAS, and debugging are disabled."
                            }
                            PlayMode::Achievement => {
                                "RetroAchievements hardcore play at 1x with emulator assists and debugging disabled."
                            }
                        });
                        ui.separator();
                        changed |= ui
                            .checkbox(
                                &mut self.settings.general.reopen_last_game,
                                "Reopen last game on startup",
                            )
                            .changed();
                        if ui.button("Restore General defaults").clicked() {
                            self.settings.general = Default::default();
                            changed = true;
                        }
                    }
                    SettingsCategory::Video => {
                        changed |= palette_settings_ui(
                            ui,
                            "settings-palette",
                            &mut self.settings.video,
                            &mut import_palette,
                        );
                        changed |= ui
                            .checkbox(&mut self.settings.video.integer_scaling, "Integer scaling")
                            .changed();
                        changed |= ui
                            .checkbox(&mut self.settings.video.show_fps, "Show FPS")
                            .changed();
                        changed |= ui
                            .checkbox(
                                &mut self.settings.video.fullscreen_on_start,
                                "Fullscreen on startup",
                            )
                            .changed();
                        ui.separator();
                        changed |= crt_settings_ui(ui, &mut self.settings.video);
                        ui.label("Fullscreen-on-start changes apply on the next launch.");
                        if ui.button("Restore Video defaults").clicked() {
                            self.settings.video = Default::default();
                            changed = true;
                        }
                    }
                    SettingsCategory::Audio => {
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut self.settings.audio.volume, 0.0..=1.0)
                                    .text("Volume"),
                            )
                            .changed();
                        changed |= ui
                            .checkbox(&mut self.settings.audio.muted, "Mute")
                            .changed();
                        changed |= ui
                            .checkbox(&mut self.settings.audio.soft_clip, "Optional soft clipping")
                            .changed();
                        changed |= ui
                            .add(
                                egui::Slider::new(
                                    &mut self.settings.audio.startup_buffer_ms,
                                    10..=100,
                                )
                                .text("Startup buffer (ms)"),
                            )
                            .changed();
                        ui.label("Startup buffer changes require restart.");
                        if ui.button("Restore Audio defaults").clicked() {
                            self.settings.audio = Default::default();
                            changed = true;
                        }
                    }
                    SettingsCategory::Input => {
                        changed |= self.input_mapping_ui(ui);
                        if ui.button("Restore Input defaults").clicked() {
                            self.settings.input = Default::default();
                            self.binding_capture = None;
                            changed = true;
                        }
                    }
                    SettingsCategory::Emulation => {
                        changed |= speed_ui(ui, &mut self.settings.emulation.speed_index);
                        changed |= ui
                            .add(
                                egui::Slider::new(
                                    &mut self.settings.emulation.rewind_seconds,
                                    5..=600,
                                )
                                .logarithmic(true)
                                .text("Rewind history (seconds)"),
                            )
                            .changed();
                        changed |= ui
                            .add(
                                egui::Slider::new(
                                    &mut self.settings.emulation.rewind_interval_frames,
                                    1..=10,
                                )
                                .text("Snapshot every N frames"),
                            )
                            .changed();
                        if ui.button("Restore Emulation defaults").clicked() {
                            self.settings.emulation = Default::default();
                            changed = true;
                        }
                    }
                    SettingsCategory::Paths => {
                        ui.label(format!(
                            "ROM folder: {}",
                            self.settings.paths.rom_folder.display()
                        ));
                        if ui.button("Choose…").clicked()
                            && let Some(path) = FileDialog::new().pick_folder()
                        {
                            self.settings.paths.rom_folder = path;
                            self.refresh_library_and_artwork();
                            changed = true;
                        }
                        ui.label(format!("States: {}", settings::state_root().display()));
                        ui.label(format!("TAS: {}", settings::tas_root().display()));
                        if ui.button("Restore Paths defaults").clicked() {
                            self.settings.paths = Default::default();
                            self.refresh_library_and_artwork();
                            changed = true;
                        }
                    }
                    SettingsCategory::SaveStates => {
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut self.settings.save_states.slots, 1..=20)
                                    .text("Slots"),
                            )
                            .changed();
                        changed |= ui
                            .checkbox(
                                &mut self.settings.save_states.autosave_on_exit,
                                "Autosave selected slot on exit",
                            )
                            .changed();
                        if ui.button("Restore Save State defaults").clicked() {
                            self.settings.save_states = Default::default();
                            changed = true;
                        }
                    }
                    SettingsCategory::Tas => {
                        changed |= ui
                            .checkbox(
                                &mut self.settings.tas.pause_when_playback_ends,
                                "Pause when playback ends",
                            )
                            .changed();
                        changed |= ui
                            .add(
                                egui::Slider::new(
                                    &mut self.settings.tas.checkpoint_interval,
                                    60..=1_200,
                                )
                                .text("Checkpoint interval (frames)"),
                            )
                            .changed();
                        if ui.button("Restore TAS defaults").clicked() {
                            self.settings.tas = Default::default();
                            changed = true;
                        }
                    }
                    SettingsCategory::Debugging => {
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut self.settings.debugging.hex_rows, 4..=32)
                                    .text("Hex rows per page"),
                            )
                            .changed();
                        if ui.button("Restore Debugging defaults").clicked() {
                            self.settings.debugging = Default::default();
                            changed = true;
                        }
                    }
                }
                if self.nes.is_some() {
                    ui.separator();
                    ui.collapsing("Per-game overrides", |ui| {
                        let mut v = self.per_game.volume.unwrap_or(self.settings.audio.volume);
                        let mut use_v = self.per_game.volume.is_some();
                        if ui.checkbox(&mut use_v, "Override volume").changed() {
                            self.per_game.volume = use_v.then_some(v);
                            self.save_per_game();
                        }
                        if use_v && ui.add(egui::Slider::new(&mut v, 0.0..=1.0)).changed() {
                            self.per_game.volume = Some(v);
                            self.save_per_game();
                        }
                        let mut mute = self.per_game.muted.unwrap_or(self.settings.audio.muted);
                        let mut use_mute = self.per_game.muted.is_some();
                        if ui.checkbox(&mut use_mute, "Override mute").changed() {
                            self.per_game.muted = use_mute.then_some(mute);
                            self.save_per_game();
                        }
                        if use_mute && ui.checkbox(&mut mute, "Muted for this game").changed() {
                            self.per_game.muted = Some(mute);
                            self.save_per_game();
                        }
                        if self.settings.general.play_mode == PlayMode::Standard {
                            let mut use_s = self.per_game.speed_index.is_some();
                            if ui.checkbox(&mut use_s, "Override speed").changed() {
                                self.per_game.speed_index = use_s.then_some(self.speed_index);
                                self.save_per_game();
                            }
                            if use_s {
                                let mut speed = self
                                    .per_game
                                    .speed_index
                                    .unwrap_or(self.speed_index)
                                    .min(SPEEDS.len() - 1);
                                if speed_ui(ui, &mut speed) {
                                    self.per_game.speed_index = Some(speed);
                                    self.save_per_game();
                                }
                            }
                        }
                    });
                }
                if changed {
                    self.settings_dirty = true;
                    self.tas.checkpoint_interval = self.settings.tas.checkpoint_interval.max(1);
                }
            });
        self.show_settings = open && !close_requested;
        if import_palette {
            self.import_custom_palette();
        } else if old_palette
            != (
                self.settings.video.palette_mode,
                self.settings.video.custom_palette_path.clone(),
            )
        {
            self.apply_video_palette_with_status();
        }
        if old_crt != crt_signature(&self.settings.video) {
            self.frame_dirty = true;
        }
        if self.settings.general.play_mode != old_play_mode {
            self.apply_play_mode(self.settings.general.play_mode);
        }
        if self.play_mode() == PlayMode::Standard
            && self.settings.emulation.speed_index != old_default_speed
        {
            self.speed_index = self.settings.emulation.speed_index.min(SPEEDS.len() - 1);
        }
        if self.settings.save_states.slots != old_slot_count {
            self.selected_slot = self
                .selected_slot
                .min(self.settings.save_states.slots.saturating_sub(1));
            self.settings.save_states.selected_slot = self.selected_slot;
            self.refresh_slots();
            self.preview_slot = None;
        }
    }

    fn states_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_states {
            return;
        }
        let mut open = self.show_states;
        let mut select = None;
        let mut save = false;
        let mut load = false;
        let mut delete = false;
        egui::Window::new("Save States")
            .open(&mut open)
            .default_size([360.0, 420.0])
            .min_size([280.0, 220.0])
            .max_size(floating_window_max_size(ui.ctx()))
            .resizable(true)
            .vscroll(true)
            .show(ui, |ui| {
                if self.nes.is_none() {
                    ui.label("Load a ROM first.");
                    return;
                }
                ui.horizontal_wrapped(|ui| {
                    for slot in 0..self.settings.save_states.slots {
                        if ui
                            .selectable_label(self.selected_slot == slot, format!("Slot {slot}"))
                            .clicked()
                        {
                            select = Some(slot);
                        }
                    }
                });
                if let Some(Some(info)) = self.state_slots.get(self.selected_slot) {
                    ui.label(format!(
                        "Created {}",
                        save_states::format_timestamp(info.created)
                    ));
                    ui.small(info.path.display().to_string());
                } else {
                    ui.label("Empty slot");
                }
                if let Some(texture) = &self.state_preview {
                    let scale = (ui.available_width() / 256.0).min(1.0).max(0.1);
                    ui.add(
                        egui::Image::new(texture)
                            .fit_to_exact_size(Vec2::new(256.0 * scale, 240.0 * scale)),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    save = ui.button("Save").clicked();
                    load = ui
                        .add_enabled(
                            self.state_slots
                                .get(self.selected_slot)
                                .is_some_and(Option::is_some),
                            egui::Button::new("Load"),
                        )
                        .clicked();
                    delete = ui
                        .add_enabled(
                            self.state_slots
                                .get(self.selected_slot)
                                .is_some_and(Option::is_some),
                            egui::Button::new("Delete"),
                        )
                        .clicked();
                });
                ui.label("Quick save: F5   Quick load: F8");
            });
        self.show_states = open;
        if let Some(slot) = select {
            self.select_slot(slot, ui.ctx());
        } else if self.preview_slot != Some(self.selected_slot) {
            self.select_slot(self.selected_slot, ui.ctx());
        }
        if save {
            self.quick_save();
            self.select_slot(self.selected_slot, ui.ctx());
        }
        if load {
            self.quick_load();
        }
        if delete && let Some(nes) = &self.nes {
            let _ = save_states::delete_slot(nes.rom_hash(), self.selected_slot);
            self.refresh_slots();
            self.state_preview = None;
        }
    }

    fn time_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_time {
            return;
        }
        let frame_rate = self.emulation_frame_rate();
        let mut open = self.show_time;
        egui::Window::new("Rewind & Speed")
            .open(&mut open)
            .default_size([460.0, 300.0])
            .min_size([280.0, 180.0])
            .max_size(floating_window_max_size(ui.ctx()))
            .resizable(true)
            .vscroll(true)
            .show(ui, |ui| {
                speed_ui(ui, &mut self.speed_index);
                ui.checkbox(&mut self.fast_forward, "Fast forward (audio muted)");
                self.held_frame_advance_button(ui, "Hold to frame advance");
                if ui
                    .add_enabled(
                        !self.rewind.is_empty(),
                        egui::Button::new("Rewind one snapshot"),
                    )
                    .clicked()
                {
                    self.rewind_step();
                }
                ui.label(format!(
                    "{} compressed snapshots ({:.1} s, {})",
                    self.rewind.len(),
                    self.rewind.len() as f64
                        * self.settings.emulation.rewind_interval_frames.max(1) as f64
                        / frame_rate,
                    format_bytes(
                        self.rewind
                            .iter()
                            .map(|point| point.compressed_machine.len())
                            .sum()
                    )
                ));
                let uncompressed: usize =
                    self.rewind.iter().map(|point| point.uncompressed_len).sum();
                let compressed: usize = self
                    .rewind
                    .iter()
                    .map(|point| point.compressed_machine.len())
                    .sum();
                if uncompressed > 0 {
                    ui.label(format!(
                        "Stored at {:.1}% of the original state size",
                        compressed as f64 * 100.0 / uncompressed as f64
                    ));
                }
                ui.label("Hold Backspace for continuous reverse playback; release to resume.");
            });
        self.show_time = open;
    }

    fn held_frame_advance_button(&mut self, ui: &mut egui::Ui, label: &str) {
        let response = ui
            .add_enabled(self.powered, egui::Button::new(label))
            .on_hover_text("Click for one frame, or hold for continuous native-rate frame advance");
        if response.is_pointer_button_down_on() {
            self.frame_advance_held = true;
            self.paused = true;
            self.tas.pause();
            self.frame_budget = 0.0;
            if self.frame_advance_hold_started.is_none() {
                let now = Instant::now();
                self.frame_advance_hold_started = Some(now);
                self.last_held_frame_advance = now;
                // Step once on the initial press. The release-click is suppressed
                // below, then repeat begins only after the deliberate hold delay.
                self.frame_advance_repeated = true;
                self.advance_frame(ui.ctx());
            }
            ui.ctx().request_repaint();
        }
        if response.clicked() && !self.frame_advance_repeated {
            self.advance_frame(ui.ctx());
        }
    }

    fn tas_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_tas {
            return;
        }
        let mut open = self.show_tas;
        let mut new_movie = None;
        let mut play = None;
        let mut pause_toggle = false;
        let mut stop = false;
        let mut import = false;
        let mut export = false;
        let mut seek = None;
        let mut edit_seek = None;
        let mut held_input_changed = false;
        let mut rerecord = false;
        let mut rewind = false;
        let mut action = None;
        let mut edited_from: Option<usize> = None;
        let mut add_marker = false;
        let mut remove_marker = None;
        let read_only = self.tas.read_only();
        let mut timeline_scroll = self.tas_timeline_scroll.take();

        egui::Window::new("TAS Editor")
            .open(&mut open)
            .resizable(true)
            .default_size([940.0, 720.0])
            .min_size([360.0, 280.0])
            .max_size(floating_window_max_size(ui.ctx()))
            .vscroll(true)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.menu_button("New movie", |ui| {
                        for (label, start) in [
                            ("From power-on", TasStartType::PowerOn),
                            ("From reset", TasStartType::Reset),
                            ("From current save state", TasStartType::SaveState),
                        ] {
                            if ui.button(label).clicked() {
                                new_movie = Some(start);
                                ui.close();
                            }
                        }
                    });
                    if ui
                        .add_enabled(
                            self.tas.movie.is_some(),
                            egui::Button::new("Play from start"),
                        )
                        .clicked()
                    {
                        play = Some(false);
                    }
                    if ui
                        .add_enabled(
                            self.tas.movie.is_some(),
                            egui::Button::new("Play read-only"),
                        )
                        .clicked()
                    {
                        play = Some(true);
                    }
                    pause_toggle = ui
                        .add_enabled(
                            self.nes.is_some(),
                            egui::Button::new(if self.paused { "Resume" } else { "Pause" }),
                        )
                        .clicked();
                    stop = ui.button("Stop").clicked();
                    import = ui.button("Load .tas…").clicked();
                    export = ui
                        .add_enabled(self.tas.movie.is_some(), egui::Button::new("Save .tas…"))
                        .clicked();
                });

                let total = self
                    .tas
                    .movie
                    .as_ref()
                    .map_or(0, |movie| movie.frames.len());
                ui.label(format!(
                    "Mode: {:?}    Playhead (next input): {} / {}    Lag: {}    Checkpoints: {}",
                    self.tas.mode,
                    self.tas.cursor.min(total),
                    total,
                    self.lag_frames,
                    self.tas.checkpoints.len()
                ));
                if let Some(desync) = &self.tas.last_desync {
                    ui.colored_label(egui::Color32::RED, desync);
                }

                ui.horizontal_wrapped(|ui| {
                    self.held_frame_advance_button(ui, "Hold to frame advance");
                    rewind = ui
                        .add_enabled(
                            if self.tas.movie.is_some() {
                                self.tas.cursor > 0
                            } else {
                                !self.rewind.is_empty()
                            },
                            egui::Button::new(if self.tas.recording_context() {
                                "Rewind 1 frame + remove future input"
                            } else {
                                "Rewind 1 frame"
                            }),
                        )
                        .on_hover_text(if self.tas.recording_context() {
                            "Move back exactly one input frame and delete input from that frame onward"
                        } else {
                            "Move back exactly one input frame without changing the movie"
                        })
                        .clicked();
                    if ui
                        .add_enabled(self.tas.cursor > 0, egui::Button::new("Seek previous"))
                        .clicked()
                    {
                        seek = Some(self.tas.cursor.saturating_sub(1));
                    }
                    if ui
                        .add_enabled(self.tas.cursor < total, egui::Button::new("Seek next"))
                        .clicked()
                    {
                        seek = Some((self.tas.cursor + 1).min(total));
                    }
                    if ui
                        .add_enabled(total > 0, egui::Button::new("Seek selected"))
                        .clicked()
                    {
                        seek = Some(self.tas.selected_frame.min(total));
                    }
                    rerecord = ui
                        .add_enabled(
                            self.tas.movie.is_some() && !read_only,
                            egui::Button::new("Rerecord from here"),
                        )
                        .clicked();
                });
                ui.small(
                    "Seek uses the closest earlier checkpoint, deterministically replays to the \
                     target, pauses there, and rebuilds presentation audio.",
                );

                if let Some(movie) = &mut self.tas.movie {
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "Start: {:?}  Region: {:?}  Rerecords: {}",
                            movie.start_type, movie.region, movie.rerecord_count
                        ));
                        ui.monospace(format!("ROM {}…", &movie.rom_sha256[..12]));
                    });
                    ui.add_enabled_ui(!read_only, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Author");
                            ui.add(
                                egui::TextEdit::singleline(
                                    movie.author.get_or_insert_with(String::new),
                                )
                                .desired_width(180.0),
                            );
                            ui.label("Description");
                            ui.add(
                                egui::TextEdit::singleline(
                                    movie.description.get_or_insert_with(String::new),
                                )
                                .desired_width(260.0),
                            );
                        });
                    });

                    {
                        let end_frame = movie.frames.len();
                        self.tas.selected_frame = self.tas.selected_frame.min(end_frame);
                        self.tas.range_end_frame = self.tas.range_end_frame.min(end_frame);
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Selected");
                            if ui
                                .add(
                                    egui::DragValue::new(&mut self.tas.selected_frame)
                                        .range(0..=end_frame),
                                )
                                .changed()
                            {
                                self.tas.range_end_frame = self.tas.selected_frame;
                                timeline_scroll = Some(self.tas.selected_frame);
                            }
                            ui.label("Range end");
                            ui.add(
                                egui::DragValue::new(&mut self.tas.range_end_frame)
                                    .range(0..=end_frame),
                            );
                            ui.separator();
                            ui.label(format!(
                                "Paused machine frame: {}",
                                self.tas.cursor.min(end_frame)
                            ));
                        });
                        ui.group(|ui| {
                            let selected = self.tas.selected_frame;
                            ui.horizontal_wrapped(|ui| {
                                ui.strong(format!("Edit current input frame {selected}"));
                                if selected == self.tas.cursor {
                                    ui.colored_label(egui::Color32::LIGHT_GREEN, "CURRENT");
                                } else {
                                    ui.colored_label(
                                        egui::Color32::YELLOW,
                                        format!("machine is at {}", self.tas.cursor),
                                    );
                                }
                                if ui
                                    .add_enabled(selected > 0, egui::Button::new("Select previous"))
                                    .clicked()
                                {
                                    self.tas.selected_frame = selected - 1;
                                    self.tas.range_end_frame = selected - 1;
                                    timeline_scroll = Some(selected - 1);
                                }
                                if ui
                                    .add_enabled(
                                        selected < end_frame,
                                        egui::Button::new("Select next"),
                                    )
                                    .clicked()
                                {
                                    self.tas.selected_frame = selected + 1;
                                    self.tas.range_end_frame = selected + 1;
                                    timeline_scroll = Some(selected + 1);
                                }
                            });

                            let original = movie.frames.get(selected).copied().unwrap_or_default();
                            let is_end_frame = selected == movie.frames.len();
                            let displayed_input = if is_end_frame {
                                self.tas_held_input
                            } else {
                                original
                            };
                            let mut input = displayed_input;
                            ui.add_enabled_ui(!read_only, |ui| {
                                input_mask_editor(ui, &mut input.player1, "Player 1", selected);
                                input_mask_editor(ui, &mut input.player2, "Player 2", selected);
                                ui.horizontal_wrapped(|ui| {
                                    if ui.button("Copy previous frame").clicked() && selected > 0 {
                                        input = movie.frames[selected - 1];
                                    }
                                    if ui.button("Clear this frame").clicked() {
                                        input = TasFrame::default();
                                    }
                                });
                            });
                            if input != displayed_input {
                                self.tas_held_input = input;
                                held_input_changed = true;
                                if !is_end_frame {
                                    TasEditor::set_frame(movie, selected, input);
                                    edited_from = Some(
                                        edited_from.map_or(selected, |old| old.min(selected)),
                                    );
                                    if selected != self.tas.cursor {
                                        edit_seek = Some(selected);
                                    }
                                }
                            }
                            ui.small(
                                "Selected buttons are held for every new or rerecorded frame until \
                                 you unselect them. The end row is written when frame advance runs.",
                            );
                        });
                        ui.add_enabled_ui(!read_only, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                for (label, requested) in [
                                    ("Insert blank", TasTimelineAction::InsertBlank),
                                    ("Duplicate", TasTimelineAction::Duplicate),
                                    ("Delete range", TasTimelineAction::Delete),
                                    ("Hold through range", TasTimelineAction::Fill),
                                    ("Clear range", TasTimelineAction::Clear),
                                    ("Copy", TasTimelineAction::Copy),
                                    ("Paste", TasTimelineAction::Paste),
                                    ("Insert paste", TasTimelineAction::InsertPaste),
                                ] {
                                    if ui.button(label).clicked() {
                                        action = Some(requested);
                                    }
                                }
                            });
                        });
                        ui.small(
                            "Timeline (clicking only selects; Seek selected moves the emulator)",
                        );
                        // `show_rows` wants the widget height without item spacing, while its
                        // scroll offset uses height plus spacing. Keeping these two values in
                        // agreement is essential at large frame numbers; the old hard-coded
                        // value accumulated several pixels of error per row and the playhead
                        // could outrun the virtualized viewport.
                        let row_height = ui.spacing().interact_size.y;
                        let row_stride = row_height + ui.spacing().item_spacing.y;
                        let mut timeline = egui::ScrollArea::both()
                            .id_salt("tas-v1-timeline")
                            .max_height(380.0)
                            .auto_shrink([false, false])
                            .animated(false);
                        if let Some(frame) = timeline_scroll {
                            timeline = timeline.vertical_scroll_offset(
                                (frame as f32 * row_stride - 190.0).max(0.0),
                            );
                        }
                        timeline.show_rows(ui, row_height, movie.frames.len() + 1, |ui, rows| {
                                for frame in rows {
                                    let marker =
                                        if frame == self.tas.cursor { "▶" } else { " " };
                                    if frame == movie.frames.len() {
                                        let response = ui
                                            .selectable_label(
                                                frame == self.tas.cursor,
                                                egui::RichText::new(format!(
                                                    "{marker}{frame:06}  [next new frame / end of movie]"
                                                ))
                                                .monospace(),
                                            );
                                        if timeline_scroll == Some(frame) {
                                            // The offset materializes this virtual row; anchoring
                                            // its real response makes egui retain the exact jump.
                                            response.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        if response.clicked() {
                                            self.tas.selected_frame = frame;
                                            self.tas.range_end_frame = frame;
                                            response.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        continue;
                                    }
                                    let input = movie.frames[frame];
                                    let row = format!(
                                        "{marker}{frame:06}  P1 {:02X} {:<28}  P2 {:02X} {}",
                                        input.player1,
                                        input_mask_label(input.player1),
                                        input.player2,
                                        input_mask_label(input.player2)
                                    );
                                    let response = ui
                                        .selectable_label(
                                            frame == self.tas.selected_frame,
                                            egui::RichText::new(row).monospace(),
                                        );
                                    if timeline_scroll == Some(frame) {
                                        response.scroll_to_me(Some(egui::Align::Center));
                                    }
                                    if response.clicked() {
                                        self.tas.selected_frame = frame;
                                        self.tas.range_end_frame = frame;
                                        response.scroll_to_me(Some(egui::Align::Center));
                                    }
                                }
                            });
                    }

                    ui.separator();
                    ui.strong("Bookmarks / markers");
                    ui.horizontal_wrapped(|ui| {
                        ui.text_edit_singleline(&mut self.tas.marker_label);
                        add_marker = ui
                            .add_enabled(!read_only, egui::Button::new("Add at selected"))
                            .clicked();
                    });
                    for marker in movie.markers.clone() {
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .link(format!("Frame {} — {}", marker.frame, marker.label))
                                .clicked()
                            {
                                seek = Some(marker.frame.min(movie.frames.len()));
                            }
                            if ui
                                .add_enabled(!read_only, egui::Button::new("Remove"))
                                .clicked()
                            {
                                remove_marker = Some(marker.frame);
                            }
                        });
                    }
                }

                ui.collapsing("TAS debug log", |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(140.0)
                        .show(ui, |ui| {
                            for entry in self.tas.logs() {
                                ui.monospace(entry);
                            }
                        });
                });
            });
        self.show_tas = open;

        if let Some(start) = new_movie {
            self.new_tas_movie(start);
        }
        if pause_toggle {
            self.toggle_pause();
        }
        if stop {
            self.tas.stop();
        }
        if rewind {
            self.rewind_step();
        }
        if rerecord && self.tas.resume_recording() {
            self.fast_forward = false;
            self.paused = false;
            self.status = format!("Rerecording from frame {}", self.tas.cursor);
        }
        if let Some(frame) = edited_from {
            self.paused = true;
            self.tas.pause();
            if self.tas.cursor == frame {
                // Manual GUI input is movie input, not host input. Preview it on
                // the next frame advance even if the movie was previously idle
                // or recording.
                self.tas.set_cursor_paused_for_preview(frame);
            }
            self.tas.invalidate_after(frame);
            self.frame_budget = 0.0;
            self.clear_audio_pipeline();
            self.status = format!("Wrote TAS input at current frame {frame}; emulation paused");
        }
        if held_input_changed && edited_from.is_none() {
            self.paused = true;
            self.tas.pause();
            self.frame_budget = 0.0;
            self.clear_audio_pipeline();
            self.status = format!(
                "Held TAS input set to P1 {:02X}, P2 {:02X}",
                self.tas_held_input.player1, self.tas_held_input.player2
            );
        }
        if let Some(action) = action {
            self.apply_tas_timeline_action(action);
        }
        if add_marker {
            let label = std::mem::take(&mut self.tas.marker_label);
            self.tas.add_marker(self.tas.selected_frame, label);
        }
        if let Some(frame) = remove_marker {
            self.tas.remove_marker(frame);
        }
        if let Some(target) = seek {
            self.seek_tas(target);
        }
        if let Some(target) = edit_seek {
            self.seek_tas(target);
            // The selected row was already visible when it was edited. Keep the
            // user's exact scroll position while aligning the machine state.
            self.tas_timeline_scroll = None;
        }
        if let Some(read_only) = play {
            self.start_tas_playback(read_only);
        }
        if export {
            self.export_tas();
        }
        if import {
            self.import_tas();
        }
    }

    fn tas_control_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_tas_control {
            return;
        }
        let mut open = self.show_tas_control;
        let mut load = false;
        let mut clear = false;
        let mut convert = false;
        let mut scroll_to_selected = self.tas_control_scroll.take();
        egui::Window::new("TAS Control View")
            .open(&mut open)
            .resizable(true)
            .default_size([900.0, 680.0])
            .min_size([360.0, 280.0])
            .max_size(floating_window_max_size(ui.ctx()))
            .vscroll(true)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    load = ui.button("Open movie…").clicked();
                    clear = ui
                        .add_enabled(
                            self.tas_control_movie.is_some(),
                            egui::Button::new("Clear"),
                        )
                        .clicked();
                    ui.label("FM2 • BK2 • BizHawk Input Log.txt • native TAS");
                });
                ui.label(&self.tas_control_status);

                if let Some(movie) = &self.tas_control_movie {
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(movie.format.to_string());
                        ui.label(format!("{} frames", movie.frames.len()));
                        ui.label(format!("{} rerecords", movie.rerecord_count));
                        ui.label(format!("{} event(s)", movie.events.len()));
                    });
                    ui.monospace(movie.source_path.display().to_string());
                    if let Some(author) = &movie.author {
                        ui.label(format!("Author: {author}"));
                    }
                    if let Some(description) = &movie.description {
                        ui.collapsing("Description", |ui| {
                            ui.label(description);
                        });
                    }
                    for warning in &movie.warnings {
                        ui.colored_label(egui::Color32::YELLOW, format!("Warning: {warning}"));
                    }
                    if !movie.events.is_empty() {
                        ui.collapsing("External reset / power / system events", |ui| {
                            for event in &movie.events {
                                ui.monospace(format!(
                                    "Frame {:06}: {}",
                                    event.frame, event.description
                                ));
                            }
                            ui.small(
                                "These events are preserved as warning markers but are not executed \
                                 by the converted controller-only movie.",
                            );
                        });
                    }

                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Converted movie starts from");
                        ui.selectable_value(
                            &mut self.tas_control_start,
                            TasStartType::PowerOn,
                            "Power-on",
                        );
                        ui.selectable_value(
                            &mut self.tas_control_start,
                            TasStartType::Reset,
                            "Reset",
                        );
                        ui.selectable_value(
                            &mut self.tas_control_start,
                            TasStartType::SaveState,
                            if movie.embedded_fceux_state.is_some() {
                                "Embedded FCEUX state"
                            } else {
                                "Current state"
                            },
                        );
                        convert = ui
                            .add_enabled(
                                self.nes.is_some(),
                                egui::Button::new("Convert and open in TAS Editor"),
                            )
                            .clicked();
                    });
                    if self.nes.is_none() {
                        ui.small("Load the matching NES ROM before converting.");
                    }

                    if movie.frames.is_empty() {
                        ui.label("This movie has no controller frames.");
                    } else {
                        self.tas_control_selected =
                            self.tas_control_selected.min(movie.frames.len() - 1);
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Inspect frame");
                            if ui
                                .add(
                                    egui::DragValue::new(&mut self.tas_control_selected)
                                        .range(0..=movie.frames.len() - 1),
                                )
                                .changed()
                            {
                                scroll_to_selected = Some(self.tas_control_selected);
                            }
                            let frame = movie.frames[self.tas_control_selected];
                            ui.monospace(format!(
                                "P1 {:02X} {}    P2 {:02X} {}",
                                frame.player1,
                                input_mask_label(frame.player1),
                                frame.player2,
                                input_mask_label(frame.player2)
                            ));
                        });
                        let row_height = ui.spacing().interact_size.y;
                        let row_stride = row_height + ui.spacing().item_spacing.y;
                        let mut inputs = egui::ScrollArea::both()
                            .id_salt("tas-control-inputs")
                            .max_height(420.0)
                            .auto_shrink([false, false]);
                        if let Some(frame) = scroll_to_selected {
                            inputs = inputs.vertical_scroll_offset(
                                (frame as f32 * row_stride - 210.0).max(0.0),
                            );
                        }
                        inputs.show_rows(ui, row_height, movie.frames.len(), |ui, rows| {
                                for index in rows {
                                    let frame = movie.frames[index];
                                    let event = if movie
                                        .events
                                        .iter()
                                        .any(|event| event.frame == index)
                                    {
                                        " !"
                                    } else {
                                        ""
                                    };
                                    let text = format!(
                                        "{index:06}{event:2}  P1 {:02X} {:<28}  P2 {:02X} {}",
                                        frame.player1,
                                        input_mask_label(frame.player1),
                                        frame.player2,
                                        input_mask_label(frame.player2)
                                    );
                                    let response = ui
                                        .selectable_label(
                                            self.tas_control_selected == index,
                                            egui::RichText::new(text).monospace(),
                                        );
                                    if response.clicked() {
                                        self.tas_control_selected = index;
                                        response.scroll_to_me(Some(egui::Align::Center));
                                    }
                                }
                            });
                    }
                }
            });
        self.show_tas_control = open;

        if clear {
            self.tas_control_movie = None;
            self.tas_control_status = "Control view cleared".into();
        }
        if load
            && let Some(path) = FileDialog::new()
                .add_filter("TAS movies", &["fm2", "bk2", "txt", "log", "tas"])
                .pick_file()
        {
            let expected_hash = self
                .nes
                .as_ref()
                .map(|nes| tas::rom_sha256_hex(nes.rom_sha256()));
            match tas_control::load(&path, expected_hash.as_deref()) {
                Ok(movie) => {
                    self.tas_control_start = movie.suggested_start;
                    self.tas_control_selected = 0;
                    self.tas_control_scroll = Some(0);
                    self.tas_control_status = format!(
                        "Loaded {} controller frames from {}",
                        movie.frames.len(),
                        movie.format
                    );
                    self.tas_control_movie = Some(movie);
                }
                Err(error) => {
                    self.tas_control_status = format!("Could not load movie: {error}");
                }
            }
        }
        if convert {
            self.convert_tas_control_movie();
        }
    }

    fn convert_tas_control_movie(&mut self) {
        if self.play_mode().restricts_assists() {
            self.status = format!(
                "TAS tools are disabled in {} mode",
                self.play_mode().label()
            );
            return;
        }
        let Some(source) = self.tas_control_movie.clone() else {
            return;
        };
        if self.nes.is_none() {
            self.tas_control_status = "Load the matching NES ROM before converting".into();
            return;
        }
        let start_type = self.tas_control_start;
        let imported_fceux =
            start_type == TasStartType::SaveState && source.embedded_fceux_state.is_some();
        if imported_fceux {
            if let Err(error) = self.prepare_fceux_tas_start(&source) {
                self.tas_control_status = format!("Could not import embedded state: {error}");
                self.status = self.tas_control_status.clone();
                return;
            }
        } else {
            self.new_tas_movie(start_type);
        }
        let Some(base) = self.tas.movie.as_ref() else {
            self.tas_control_status = "Could not create the native starting condition".into();
            return;
        };
        let mut converted = source.to_native_movie(
            base.rom_sha256.clone(),
            base.start_type,
            base.starting_state.clone(),
        );
        converted.state_checksums = base.state_checksums.clone();
        let frame_count = converted.frames.len();
        self.tas.movie = Some(converted);
        self.tas.set_cursor_paused_for_preview(0);
        self.tas_held_input = TasFrame::default();
        self.tas_timeline_scroll = Some(0);
        self.paused = true;
        self.fast_forward = false;
        self.frame_budget = 0.0;
        self.clear_audio_pipeline();
        self.show_tas = true;
        self.status = format!(
            "Converted {frame_count} {} frames into the native TAS editor{}",
            source.format,
            if imported_fceux {
                " from its embedded FCEUX state"
            } else {
                ""
            }
        );
        self.tas_control_status = self.status.clone();
    }

    fn prepare_fceux_tas_start(&mut self, source: &ControlMovie) -> Result<(), String> {
        source.verify_fceux_rom(&self.rom_bytes)?;
        let fcs = source
            .embedded_fceux_state
            .as_deref()
            .ok_or_else(|| "movie has no embedded FCEUX state".to_owned())?;
        let mut nes = nes_from_rom_path(&self.rom_bytes, self.rom_path.as_deref())
            .map_err(|error| error.to_string())?;
        nes.import_fceux_state(fcs)
            .map_err(|error| error.to_string())?;
        let initial_state = nes.save_state().map_err(|error| error.to_string())?;
        let movie = TasMovie::new(
            tas::rom_sha256_hex(nes.rom_sha256()),
            TasStartType::SaveState,
            Some(initial_state.clone()),
        );
        self.last_controller_reads = nes
            .controller_reads(0)
            .wrapping_add(nes.controller_reads(1));
        self.nes = Some(nes);
        let _ = self.apply_video_palette();
        self.tas.new_movie(movie, initial_state);
        self.tas_held_input = TasFrame::default();
        self.powered = true;
        self.paused = false;
        self.fast_forward = false;
        self.last_controller_reads = self
            .nes
            .as_ref()
            .map(|nes| {
                nes.controller_reads(0)
                    .wrapping_add(nes.controller_reads(1))
            })
            .unwrap_or(0);
        self.clear_rewind_history();
        self.lag_frames = 0;
        self.frame_dirty = true;
        self.clear_audio_pipeline();
        Ok(())
    }

    fn input_mapping_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let connected = self
            .gamepads
            .as_ref()
            .map(|gamepads| {
                gamepads
                    .gamepads()
                    .enumerate()
                    .map(|(slot, (_, gamepad))| (slot, gamepad.name().to_owned()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if let Some(error) = &self.gamepad_error {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!("Gamepad support could not start: {error}"),
            );
        } else if connected.is_empty() {
            ui.label("No controller detected. Plug one in; it can be mapped without restarting.");
        } else {
            ui.label(format!(
                "{} controller{} detected: {}",
                connected.len(),
                if connected.len() == 1 { "" } else { "s" },
                connected
                    .iter()
                    .map(|(slot, name)| format!("#{} {name}", slot + 1))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(activity) = &self.last_gamepad_activity {
            ui.monospace(format!("Last raw input: {activity}"));
        } else if !connected.is_empty() {
            ui.small("Press something on the controller to inspect its raw input.");
        }

        ui.horizontal_wrapped(|ui| {
            for player in 0..2 {
                ui.label(format!("Player {} controller:", player + 1));
                let automatic = format!("Automatic (controller #{})", player + 1);
                let selected = self.settings.input.gamepad_slots[player]
                    .and_then(|slot| {
                        connected
                            .iter()
                            .find(|(candidate, _)| *candidate == slot)
                            .map(|(slot, name)| format!("#{} {name}", slot + 1))
                    })
                    .unwrap_or_else(|| automatic.clone());
                egui::ComboBox::from_id_salt(("gamepad-slot", player, ui.id()))
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        changed |= ui
                            .selectable_value(
                                &mut self.settings.input.gamepad_slots[player],
                                None,
                                &automatic,
                            )
                            .changed();
                        for (slot, name) in &connected {
                            changed |= ui
                                .selectable_value(
                                    &mut self.settings.input.gamepad_slots[player],
                                    Some(*slot),
                                    format!("#{} {name}", slot + 1),
                                )
                                .changed();
                        }
                    });
            }
        });

        changed |= ui
            .add(
                egui::Slider::new(&mut self.settings.input.gamepad_axis_threshold, 0.1..=0.9)
                    .text("Stick / axis activation threshold"),
            )
            .changed();
        ui.small(
            "Lower this if a direction will not capture or activate. Raise it if an axis drifts.",
        );
        if self.play_mode() == PlayMode::Standard {
            changed |= ui
                .checkbox(
                    &mut self.settings.input.allow_opposite_directions,
                    "Allow opposite D-pad directions",
                )
                .on_hover_text(
                    "Off matches a stock NES rocker D-pad: Left+Right and Up+Down cancel to neutral. Enable only for TAS/debug input that intentionally needs impossible combinations.",
                )
                .changed();
        }
        if self.play_mode().restricts_assists() || !self.settings.input.allow_opposite_directions {
            ui.small("Hardware-accurate D-pad: opposite directions cancel to neutral.");
        }

        if let Some(capture) = self.binding_capture {
            ui.horizontal_wrapped(|ui| {
                let (kind, target) = match capture {
                    BindingCapture::Keyboard { player, button } => (
                        "key",
                        format!("Player {} {}", player + 1, nes_button_label(button)),
                    ),
                    BindingCapture::Gamepad { player, button } => (
                        "controller button or direction",
                        format!("Player {} {}", player + 1, nes_button_label(button)),
                    ),
                    BindingCapture::VsCoinKeyboard => ("key", "VS insert coin".into()),
                    BindingCapture::VsCoinGamepad => {
                        ("controller button or direction", "VS insert coin".into())
                    }
                };
                ui.strong(format!("Press a {kind} for {target}…"));
                if ui.button("Cancel").clicked() {
                    self.binding_capture = None;
                }
                ui.small("Esc also cancels capture.");
            });
        } else {
            ui.small(
                "Click any mapping, then press the key or controller input you want. Controller capture stores the exact hardware input for compatibility.",
            );
        }

        egui::ScrollArea::horizontal().show(ui, |ui| {
            egui::Grid::new(ui.id().with("input-map"))
                .striped(true)
                .min_col_width(105.0)
                .show(ui, |ui| {
                    ui.strong("NES button");
                    ui.strong("P1 keyboard");
                    ui.strong("P1 controller");
                    ui.strong("P2 keyboard");
                    ui.strong("P2 controller");
                    ui.end_row();
                    for button in 0..8 {
                        ui.label(nes_button_label(button));
                        for player in 0..2 {
                            let keyboard_capture = BindingCapture::Keyboard { player, button };
                            let key_label = if self.binding_capture == Some(keyboard_capture) {
                                "Press a key…".to_owned()
                            } else if player == 0 {
                                self.settings.input.bindings[button].label().to_owned()
                            } else {
                                self.settings.input.player2_bindings[button]
                                    .label()
                                    .to_owned()
                            };
                            if ui.button(key_label).clicked() {
                                self.binding_capture = Some(keyboard_capture);
                            }

                            let gamepad_capture = BindingCapture::Gamepad { player, button };
                            let binding = if player == 0 {
                                self.settings.input.gamepad_bindings[button]
                            } else {
                                self.settings.input.player2_gamepad_bindings[button]
                            };
                            let gamepad_label = if self.binding_capture == Some(gamepad_capture) {
                                "Press input…".to_owned()
                            } else {
                                binding
                                    .map(gamepad_binding_label)
                                    .unwrap_or_else(|| "Not bound".into())
                            };
                            let response = ui
                                .button(gamepad_label)
                                .on_hover_text("Click to capture. Right-click to clear.");
                            if response.clicked() {
                                self.binding_capture = Some(gamepad_capture);
                            }
                            if response.secondary_clicked() {
                                if player == 0 {
                                    self.settings.input.gamepad_bindings[button] = None;
                                } else {
                                    self.settings.input.player2_gamepad_bindings[button] = None;
                                }
                                if self.binding_capture == Some(gamepad_capture) {
                                    self.binding_capture = None;
                                }
                                changed = true;
                            }
                        }
                        ui.end_row();
                    }
                });
        });

        ui.separator();
        ui.strong("VS System arcade controls");
        ui.small("These apply automatically to supported Nintendo VS arcade games.");
        egui::ScrollArea::horizontal().show(ui, |ui| {
            egui::Grid::new(ui.id().with("vs-arcade-inputs"))
                .striped(true)
                .show(ui, |ui| {
                    ui.strong("Arcade input");
                    ui.strong("Keyboard");
                    ui.strong("Controller");
                    ui.end_row();

                    ui.label("Insert coin");
                    let coin_key_capture = BindingCapture::VsCoinKeyboard;
                    let coin_key_label = if self.binding_capture == Some(coin_key_capture) {
                        "Press a key…"
                    } else {
                        self.settings.input.vs_coin_binding.label()
                    };
                    if ui.button(coin_key_label).clicked() {
                        self.binding_capture = Some(coin_key_capture);
                    }
                    let coin_gamepad_capture = BindingCapture::VsCoinGamepad;
                    let coin_gamepad_label = if self.binding_capture == Some(coin_gamepad_capture) {
                        "Press input…".into()
                    } else {
                        self.settings
                            .input
                            .vs_coin_gamepad_binding
                            .map(gamepad_binding_label)
                            .unwrap_or_else(|| "Select + Start chord".into())
                    };
                    let response = ui.button(coin_gamepad_label).on_hover_text(
                        "Click to capture. Right-click to restore the Select+Start chord.",
                    );
                    if response.clicked() {
                        self.binding_capture = Some(coin_gamepad_capture);
                    }
                    if response.secondary_clicked() {
                        self.settings.input.vs_coin_gamepad_binding = None;
                        if self.binding_capture == Some(coin_gamepad_capture) {
                            self.binding_capture = None;
                        }
                        changed = true;
                    }
                    ui.end_row();

                    ui.label("Start 1 player");
                    let select_key_capture = BindingCapture::Keyboard {
                        player: 0,
                        button: 2,
                    };
                    let select_key_label = if self.binding_capture == Some(select_key_capture) {
                        "Press a key…"
                    } else {
                        self.settings.input.bindings[2].label()
                    };
                    if ui.button(select_key_label).clicked() {
                        self.binding_capture = Some(select_key_capture);
                    }
                    let select_gamepad_capture = BindingCapture::Gamepad {
                        player: 0,
                        button: 2,
                    };
                    let select_gamepad_label =
                        if self.binding_capture == Some(select_gamepad_capture) {
                            "Press input…".into()
                        } else {
                            self.settings.input.gamepad_bindings[2]
                                .map(gamepad_binding_label)
                                .unwrap_or_else(|| "Not bound".into())
                        };
                    if ui.button(select_gamepad_label).clicked() {
                        self.binding_capture = Some(select_gamepad_capture);
                    }
                    ui.end_row();

                    ui.label("Start 2 players");
                    let start_key_capture = BindingCapture::Keyboard {
                        player: 0,
                        button: 3,
                    };
                    let start_key_label = if self.binding_capture == Some(start_key_capture) {
                        "Press a key…"
                    } else {
                        self.settings.input.bindings[3].label()
                    };
                    if ui.button(start_key_label).clicked() {
                        self.binding_capture = Some(start_key_capture);
                    }
                    let start_gamepad_capture = BindingCapture::Gamepad {
                        player: 0,
                        button: 3,
                    };
                    let start_gamepad_label = if self.binding_capture == Some(start_gamepad_capture)
                    {
                        "Press input…".into()
                    } else {
                        self.settings.input.gamepad_bindings[3]
                            .map(gamepad_binding_label)
                            .unwrap_or_else(|| "Not bound".into())
                    };
                    if ui.button(start_gamepad_label).clicked() {
                        self.binding_capture = Some(start_gamepad_capture);
                    }
                    ui.end_row();
                });
        });

        let player1 = self.gamepad_mask(0, &self.settings.input.gamepad_bindings);
        let player2 = self.gamepad_mask(1, &self.settings.input.player2_gamepad_bindings);
        ui.monospace(format!(
            "Mapped input test — P1: {}   P2: {}",
            input_mask_label(player1),
            input_mask_label(player2)
        ));
        changed
    }

    fn input_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_input {
            return;
        }
        let mut open = self.show_input;
        egui::Window::new("Input Configuration")
            .open(&mut open)
            .default_size([760.0, 660.0])
            .min_size([320.0, 260.0])
            .max_size(floating_window_max_size(ui.ctx()))
            .resizable(true)
            .vscroll(true)
            .show(ui, |ui| {
                if self.input_mapping_ui(ui) {
                    self.settings_dirty = true;
                }
                ui.label("Mappings apply immediately and are saved globally.");
            });
        self.show_input = open;
    }

    fn av_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_av {
            return;
        }
        let mut open = self.show_av;
        let old_palette = (
            self.settings.video.palette_mode,
            self.settings.video.custom_palette_path.clone(),
        );
        let old_crt = crt_signature(&self.settings.video);
        let mut import_palette = false;
        egui::Window::new("Audio / Video")
            .open(&mut open)
            .default_size([520.0, 600.0])
            .min_size([300.0, 240.0])
            .max_size(floating_window_max_size(ui.ctx()))
            .resizable(true)
            .vscroll(true)
            .show(ui, |ui| {
                let mut changed = false;
                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.settings.audio.volume, 0.0..=1.0)
                            .text("Volume"),
                    )
                    .changed();
                changed |= ui
                    .checkbox(&mut self.settings.audio.muted, "Mute")
                    .changed();
                changed |= ui
                    .checkbox(&mut self.settings.audio.soft_clip, "Soft clipping")
                    .changed();
                changed |= ui
                    .checkbox(&mut self.settings.video.integer_scaling, "Integer scaling")
                    .changed();
                ui.separator();
                changed |= palette_settings_ui(
                    ui,
                    "av-palette",
                    &mut self.settings.video,
                    &mut import_palette,
                );
                changed |= crt_settings_ui(ui, &mut self.settings.video);
                if changed {
                    self.settings_dirty = true;
                }
                if let Some(audio) = &self.audio {
                    ui.label(format!(
                        "{} / {} Hz / queued {} / underruns {} / overflows {}",
                        audio.device_name(),
                        audio.device_sample_rate(),
                        audio.queued_samples(),
                        audio.underflows(),
                        audio.overflows()
                    ));
                } else if let Some(error) = &self.audio_error {
                    ui.colored_label(egui::Color32::YELLOW, error);
                }
                if let Some(nes) = &mut self.nes {
                    let apu = nes.apu_state();
                    ui.separator();
                    ui.label("Channel isolation");
                    ui.horizontal_wrapped(|ui| {
                        for (index, channel, label) in [
                            (0, ApuChannel::Pulse1, "P1"),
                            (1, ApuChannel::Pulse2, "P2"),
                            (2, ApuChannel::Triangle, "Triangle"),
                            (3, ApuChannel::Noise, "Noise"),
                            (4, ApuChannel::Dmc, "DMC"),
                        ] {
                            let mut enabled = apu.channel_output_enabled[index];
                            if ui.checkbox(&mut enabled, label).changed() {
                                nes.set_apu_channel_output_enabled(channel, enabled);
                            }
                        }
                    });
                }
            });
        self.show_av = open;
        if import_palette {
            self.import_custom_palette();
        } else if old_palette
            != (
                self.settings.video.palette_mode,
                self.settings.video.custom_palette_path.clone(),
            )
        {
            self.apply_video_palette_with_status();
        }
        if old_crt != crt_signature(&self.settings.video) {
            self.frame_dirty = true;
        }
    }

    fn debugger_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_debugger {
            return;
        }
        let mut open = self.show_debugger;
        egui::Window::new("Debugger")
            .open(&mut open)
            .default_size([520.0, 240.0])
            .min_size([300.0, 180.0])
            .max_size(floating_window_max_size(ui.ctx()))
            .resizable(true)
            .vscroll(true)
            .show(ui, |ui| {
                if let Some(nes) = &self.nes {
                    let c = nes.cpu_state();
                    let p = nes.ppu_state();
                    ui.monospace(format!(
                        "PC {:04X} A {:02X} X {:02X} Y {:02X} SP {:02X} P {:02X}",
                        c.program_counter, c.a, c.x, c.y, c.stack_pointer, c.status
                    ));
                    ui.monospace(format!(
                        "Instructions {} CPU cycles {}",
                        c.instructions,
                        nes.cpu_cycles()
                    ));
                    ui.monospace(format!(
                        "PPU scanline {} dot {} v {:04X} t {:04X}",
                        p.scanline, p.dot, p.vram_address, p.temp_address
                    ));
                    ui.label(format!(
                        "Frame {}  Lag frames {}  Controller reads {}",
                        nes.frame().number,
                        self.lag_frames,
                        self.last_controller_reads
                    ));
                } else {
                    ui.label("No ROM loaded");
                }
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button(if self.paused { "Resume" } else { "Pause" })
                        .clicked()
                    {
                        self.toggle_pause();
                    }
                    if ui.button("Frame step").clicked() {
                        self.advance_frame(ui.ctx());
                    }
                    if ui.button("Hex editor").clicked() {
                        self.show_hex = true;
                        self.paused = true;
                    }
                });
            });
        self.show_debugger = open;
    }

    fn hex_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_hex {
            return;
        }
        let mut open = self.show_hex;
        egui::Window::new("Hex Editor")
            .open(&mut open)
            .default_size([820.0, 560.0])
            .min_size([340.0, 260.0])
            .max_size(floating_window_max_size(ui.ctx()))
            .resizable(true)
            .vscroll(true)
            .show(ui, |ui| {
                let Some(nes) = &mut self.nes else {
                    ui.label("No ROM loaded");
                    return;
                };
                self.paused = true;
                egui::ComboBox::from_label("Memory")
                    .selected_text(memory_label(self.hex_space))
                    .show_ui(ui, |ui| {
                        for (space, label) in [
                            (MemorySpace::CpuRam, "CPU RAM"),
                            (MemorySpace::PpuNametable, "PPU nametables"),
                            (MemorySpace::Palette, "Palette RAM"),
                            (MemorySpace::Oam, "OAM"),
                            (MemorySpace::PrgRom, "PRG ROM"),
                            (MemorySpace::Chr, "CHR ROM/RAM"),
                        ] {
                            if ui
                                .selectable_value(&mut self.hex_space, space, label)
                                .changed()
                            {
                                self.hex_start = 0;
                                self.hex_selected = None;
                            }
                        }
                    });
                let image = nes.memory_image(self.hex_space);
                ui.label(if image.writable {
                    "Writable (emulation paused while editing)"
                } else {
                    "Read-only"
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Address");
                    ui.text_edit_singleline(&mut self.hex_jump);
                    if ui.button("Jump").clicked()
                        && let Ok(address) =
                            usize::from_str_radix(self.hex_jump.trim().trim_start_matches("0x"), 16)
                    {
                        self.hex_start = address
                            .saturating_sub(image.base_address)
                            .min(image.bytes.len().saturating_sub(1))
                            & !15;
                    }
                    if ui.button("Prev").clicked() {
                        self.hex_start = self
                            .hex_start
                            .saturating_sub(self.settings.debugging.hex_rows * 16);
                    }
                    if ui.button("Next").clicked() {
                        self.hex_start = (self.hex_start + self.settings.debugging.hex_rows * 16)
                            .min(image.bytes.len().saturating_sub(1))
                            & !15;
                    }
                });
                egui::ScrollArea::both()
                    .max_height(380.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for row in 0..self.settings.debugging.hex_rows {
                            let offset = self.hex_start + row * 16;
                            if offset >= image.bytes.len() {
                                break;
                            }
                            ui.horizontal(|ui| {
                                ui.monospace(format!("{:04X}:", image.base_address + offset));
                                for col in 0..16 {
                                    let index = offset + col;
                                    if let Some(value) = image.bytes.get(index)
                                        && ui
                                            .selectable_label(
                                                self.hex_selected == Some(index),
                                                format!("{value:02X}"),
                                            )
                                            .clicked()
                                    {
                                        self.hex_selected = Some(index);
                                        self.hex_value = format!("{value:02X}");
                                    }
                                }
                                let ascii: String = image.bytes
                                    [offset..(offset + 16).min(image.bytes.len())]
                                    .iter()
                                    .map(|b| {
                                        if b.is_ascii_graphic() {
                                            char::from(*b)
                                        } else {
                                            '.'
                                        }
                                    })
                                    .collect();
                                ui.monospace(ascii);
                            });
                        }
                    });
                if let Some(offset) = self.hex_selected {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("Offset {offset:04X}"));
                        ui.text_edit_singleline(&mut self.hex_value);
                        if ui
                            .add_enabled(image.writable, egui::Button::new("Write byte"))
                            .clicked()
                        {
                            match u8::from_str_radix(self.hex_value.trim(), 16) {
                                Ok(value)
                                    if nes.debug_write_memory(self.hex_space, offset, value) =>
                                {
                                    self.status = format!(
                                        "Wrote {value:02X} at {:04X}",
                                        image.base_address + offset
                                    )
                                }
                                _ => self.status = "Invalid or read-only hex edit".into(),
                            }
                        }
                    });
                }
            });
        self.show_hex = open;
    }

    /// Keep the editor anchored to the next TAS input that will execute.
    ///
    /// Playback calls this after every emulated frame. Once paused, no further
    /// automatic updates occur, so the user can still select and edit any row.
    fn follow_tas_cursor(&mut self) {
        let total = self
            .tas
            .movie
            .as_ref()
            .map_or(0, |movie| movie.frames.len());
        let current = self.tas.cursor.min(total);
        self.tas.selected_frame = current;
        self.tas.range_end_frame = current;
        self.tas_timeline_scroll = Some(current);
    }

    /// Replay the segment a second time from the preceding known checkpoint.
    /// A matching live/replay pair proves stale metadata and refreshes it. If
    /// replay instead matches the movie's expected hash, restore that verified
    /// state so playback can continue without carrying a transient divergence.
    fn reconcile_tas_checkpoint(&mut self, frame: usize) -> TasCheckpointRecovery {
        let Some(current) = self
            .tas
            .checkpoints
            .iter()
            .find(|point| point.frame == frame)
            .cloned()
        else {
            return TasCheckpointRecovery::Unrecoverable;
        };
        let Some(previous) = self
            .tas
            .checkpoints
            .iter()
            .rev()
            .find(|point| point.frame < frame)
            .cloned()
        else {
            return TasCheckpointRecovery::Unrecoverable;
        };
        let Some((inputs, expected)) = self.tas.movie.as_ref().and_then(|movie| {
            Some((
                movie.frames.get(previous.frame..frame)?.to_vec(),
                movie.state_checksums.get(&frame)?.clone(),
            ))
        }) else {
            return TasCheckpointRecovery::Unrecoverable;
        };
        let Ok(mut verifier) = nes_from_rom_path(&self.rom_bytes, self.rom_path.as_deref()) else {
            return TasCheckpointRecovery::Unrecoverable;
        };
        if verifier.load_state(&previous.state).is_err() {
            return TasCheckpointRecovery::Unrecoverable;
        }
        let mut audio = Vec::new();
        for input in inputs {
            set_controller_mask(&mut verifier, 0, input.player1);
            set_controller_mask(&mut verifier, 1, input.player2);
            if verifier.run_frame().is_err() {
                return TasCheckpointRecovery::Unrecoverable;
            }
            audio.clear();
            verifier.drain_audio_samples(&mut audio);
        }
        let Ok(verified) = verifier.save_state() else {
            return TasCheckpointRecovery::Unrecoverable;
        };
        match tas::reconcile_checkpoint(&expected, &current.state, &verified) {
            tas::CheckpointReconciliation::RefreshChecksum => {
                self.tas.repair_checkpoint_checksum(frame, &current.state);
                TasCheckpointRecovery::RefreshedChecksum
            }
            tas::CheckpointReconciliation::RestoreReplay => {
                let controller_reads = {
                    let Some(nes) = &mut self.nes else {
                        return TasCheckpointRecovery::Unrecoverable;
                    };
                    if nes.load_state(&verified).is_err() {
                        return TasCheckpointRecovery::Unrecoverable;
                    }
                    nes.controller_reads(0)
                        .wrapping_add(nes.controller_reads(1))
                };
                self.last_controller_reads = controller_reads;
                self.tas.accept_resynchronized_checkpoint(frame, verified);
                self.clear_rewind_history();
                self.clear_audio_pipeline();
                self.frame_dirty = true;
                TasCheckpointRecovery::Resynchronized
            }
            tas::CheckpointReconciliation::Unrecoverable => TasCheckpointRecovery::Unrecoverable,
        }
    }

    fn toggle_pause(&mut self) {
        if self.play_mode().restricts_assists() {
            self.status = format!("Pause is disabled in {} mode", self.play_mode().label());
            return;
        }
        if self.nes.is_some() {
            self.paused = !self.paused;
            if self.paused {
                self.tas.pause();
                if self.tas.mode != TasMode::Inactive {
                    self.follow_tas_cursor();
                }
            } else {
                self.tas.resume();
            }
            self.frame_budget = 0.0;
            self.clear_audio_pipeline();
            self.status = if self.paused { "Paused" } else { "Running" }.into();
        }
    }
    fn advance_frame(&mut self, ctx: &egui::Context) {
        if self.play_mode().restricts_assists() {
            self.status = format!(
                "Frame advance is disabled in {} mode",
                self.play_mode().label()
            );
            return;
        }
        if self.powered {
            if !self.tas.prepare_frame_advance() {
                self.paused = true;
                self.status = "Read-only TAS is at the end of the movie".into();
                return;
            }
            self.paused = true;
            self.tas.pause();
            let tas_active = self.tas.mode != TasMode::Inactive;
            // A focused egui button can claim keyboard focus. Frame advance still
            // needs the physical controller bindings, so do not suppress them here.
            let live = if self.tas.movie.is_some() {
                self.bound_input_frame(ctx)
                    .with_held_input(self.tas_held_input)
            } else {
                self.bound_input_frame(ctx)
            };
            let live = self.filter_live_dpad(live);
            if self.run_one_frame(live, false) {
                if tas_active {
                    self.follow_tas_cursor();
                }
                self.status = "Frame advanced".into();
            }
        }
    }
    fn reset(&mut self) {
        let achievement_mode = self.play_mode() == PlayMode::Achievement;
        if let Some(nes) = &mut self.nes {
            nes.reset();
            self.powered = true;
            self.paused = false;
            self.clear_rewind_history();
            self.tas.stop();
            self.lag_frames = 0;
            self.frame_dirty = true;
            self.clear_audio_pipeline();
            self.status = "Reset".into();
        }
        if achievement_mode {
            self.achievements.reset();
        }
    }
    fn power_cycle(&mut self) {
        if self.nes.is_some() {
            self.reset();
            self.status = "Power cycled".into();
        }
    }
    fn toggle_power(&mut self) {
        self.clear_audio_pipeline();
        self.tas.stop();
        if let Some(nes) = &mut self.nes {
            if self.powered {
                nes.power_off();
                self.powered = false;
                self.paused = true;
                self.status = "Powered off".into();
            } else {
                nes.power_on();
                self.powered = true;
                self.paused = false;
                self.status = "Powered on".into();
            }
        }
    }

    fn collect_compressed_rewind_points(&mut self) {
        loop {
            match self.rewind_compressor.points.try_recv() {
                Ok(point) if point.generation == self.rewind_generation => {
                    self.rewind.push_back(point);
                }
                Ok(_) => {}
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        let interval = self.settings.emulation.rewind_interval_frames.max(1) as usize;
        let native_frames_per_second = self.emulation_frame_rate().ceil() as usize;
        let max = self
            .settings
            .emulation
            .rewind_seconds
            .saturating_mul(native_frames_per_second)
            / interval;
        while self.rewind.len() > max.max(1) {
            self.rewind.pop_front();
        }
    }

    fn invalidate_pending_rewind_captures(&mut self) {
        self.rewind_generation = self.rewind_generation.wrapping_add(1);
    }

    fn clear_rewind_history(&mut self) {
        self.invalidate_pending_rewind_captures();
        self.rewind.clear();
    }

    fn update_continuous_rewind(&mut self, held: bool) {
        let interval = Duration::from_secs_f64(1.0 / REWIND_UPDATES_PER_SECOND);
        let now = Instant::now();
        if held {
            if !self.rewind_active {
                self.rewind_active = true;
                self.resume_after_rewind = !self.paused;
                self.paused = true;
                self.frame_budget = 0.0;
                self.next_rewind_step = now;
                self.invalidate_pending_rewind_captures();
                self.clear_audio_pipeline();
            }
            if now >= self.next_rewind_step {
                self.rewind_step();
                // Advance the deadline instead of resetting it to `now`, so
                // timer jitter cannot accumulate into a lower rewind rate.
                self.next_rewind_step =
                    advance_rewind_deadline(self.next_rewind_step, now, interval);
            }
        } else if self.rewind_active {
            self.rewind_active = false;
            if self.resume_after_rewind && self.powered {
                self.paused = false;
                self.status = "Resumed after rewind".into();
            } else {
                self.status = "Rewind stopped".into();
            }
            self.resume_after_rewind = false;
            self.frame_budget = 0.0;
            self.clear_audio_pipeline();
        }
    }

    fn rewind_step(&mut self) {
        if self.play_mode().restricts_assists() {
            self.status = format!("Rewind is disabled in {} mode", self.play_mode().label());
            return;
        }
        if !self.rewind_active {
            self.invalidate_pending_rewind_captures();
        }
        if self.tas.movie.is_some() {
            self.rewind_tas_one_frame();
        } else {
            self.rewind_once();
        }
    }

    fn rewind_tas_one_frame(&mut self) {
        if self.tas.cursor == 0 {
            self.status = "TAS is already at frame 0".into();
            return;
        }
        let target = self.tas.cursor - 1;
        let recording = self.tas.recording_context();
        if !self.seek_tas(target) {
            return;
        }
        let removed = if recording && self.tas.resume_recording() {
            // The machine is immediately before `target`; branching here must
            // remove that input and everything after it.
            self.tas.truncate_recording_at(target)
        } else {
            0
        };
        self.follow_tas_cursor();
        self.status = if recording {
            format!("Rewound exactly 1 TAS frame to {target}; removed {removed} future frame(s)")
        } else {
            format!("Rewound exactly 1 TAS frame to {target}")
        };
        self.presented_frames_in_window = self.presented_frames_in_window.wrapping_add(1);
    }

    fn rewind_once(&mut self) {
        let Some(point) = self.rewind.pop_back() else {
            self.status = "Rewind buffer is empty".into();
            return;
        };
        let machine = match point.decompress() {
            Ok(machine) => machine,
            Err(error) => {
                self.status = format!("Rewind snapshot is corrupt: {error}");
                return;
            }
        };
        let Some(nes) = self.nes.as_mut() else {
            return;
        };
        if let Err(error) = nes.load_state(&machine) {
            self.status = format!("Rewind restore failed: {error}");
            return;
        }
        let recording_rewind = self.tas.recording_context();
        let removed = if recording_rewind {
            self.tas.truncate_recording_at(point.tas_cursor)
        } else {
            0
        };
        if self.tas.movie.is_some() && self.tas.mode != TasMode::Inactive {
            self.tas.set_cursor_paused(point.tas_cursor);
        } else {
            self.tas.cursor = point.tas_cursor;
        }
        self.lag_frames = point.lag_frames;
        self.last_controller_reads = point.controller_reads;
        self.paused = true;
        self.frame_dirty = true;
        self.presented_frames_in_window = self.presented_frames_in_window.wrapping_add(1);
        if self.tas.movie.is_some() {
            self.follow_tas_cursor();
        }
        self.clear_audio_pipeline();
        self.status = if recording_rewind {
            format!(
                "Rewound to TAS frame {}; removed {removed} future input frame(s)",
                point.tas_cursor
            )
        } else if self.rewind_active {
            "Rewinding…".into()
        } else {
            "Rewound".into()
        };
    }
    fn quick_save(&mut self) {
        if self.play_mode().restricts_assists() {
            self.status = format!(
                "Save states are disabled in {} mode",
                self.play_mode().label()
            );
            return;
        }
        if let Some(nes) = &self.nes {
            match save_states::save_slot(nes, self.selected_slot) {
                Ok(_) => {
                    self.status = format!("Saved slot {}", self.selected_slot);
                    self.refresh_slots();
                }
                Err(e) => self.status = format!("Save failed: {e}"),
            }
        }
    }
    fn quick_load(&mut self) {
        if self.play_mode().restricts_assists() {
            self.status = format!(
                "Save states are disabled in {} mode",
                self.play_mode().label()
            );
            return;
        }
        if let Some(nes) = &mut self.nes {
            match save_states::load_slot(nes, self.selected_slot) {
                Ok(_) => {
                    self.paused = true;
                    self.powered = nes.powered();
                    self.frame_dirty = true;
                    self.last_controller_reads = nes.controller_reads(0);
                    self.clear_rewind_history();
                    self.tas.stop();
                    self.lag_frames = 0;
                    self.clear_audio_pipeline();
                    self.status = format!("Loaded slot {}", self.selected_slot);
                }
                Err(e) => self.status = format!("Load failed: {e}"),
            }
        }
    }
    fn refresh_slots(&mut self) {
        self.state_slots = self
            .nes
            .as_ref()
            .map(|n| save_states::inspect_slots(n.rom_hash(), self.settings.save_states.slots))
            .unwrap_or_default();
        self.state_preview = None;
        self.preview_slot = None;
    }
    fn select_slot(&mut self, slot: usize, ctx: &egui::Context) {
        self.selected_slot = slot;
        self.settings.save_states.selected_slot = slot;
        self.settings_dirty = true;
        if self.preview_slot != Some(slot) {
            self.state_preview = self
                .state_slots
                .get(slot)
                .and_then(Option::as_ref)
                .map(|info| {
                    ctx.load_texture(
                        format!("state-{slot}"),
                        ColorImage::from_rgb([FRAME_WIDTH, FRAME_HEIGHT], &info.preview_rgb),
                        TextureOptions::NEAREST,
                    )
                });
            self.preview_slot = Some(slot);
        }
    }
    fn clear_audio_pipeline(&mut self) {
        self.audio_scratch.clear();
        if let Some(nes) = &mut self.nes {
            nes.drain_audio_samples(&mut self.audio_scratch);
        }
        self.audio_scratch.clear();
        if let Some(audio) = &self.audio {
            audio.clear();
        }
    }
    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.fullscreen = !self.fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
    }
    fn open_rom_dialog(&mut self) {
        let mut dialog = FileDialog::new()
            .set_title("Open NES ROM")
            .add_filter("NES ROM", &["nes"]);
        if self.settings.paths.rom_folder.is_dir() {
            dialog = dialog.set_directory(&self.settings.paths.rom_folder);
        }
        if let Some(path) = dialog.pick_file() {
            self.try_load_rom(path);
        }
    }
    fn try_load_rom(&mut self, path: PathBuf) {
        if let Err(error) = self.load_rom(path) {
            self.status = format!("Could not load ROM: {error}");
        }
    }
    fn load_rom(&mut self, path: PathBuf) -> Result<(), Box<dyn Error>> {
        self.save_battery()?;
        let bytes = fs::read(&path)?;
        let inferred_region = inferred_region_from_rom_path(&path);
        let mut replacement = nes_from_rom_path(&bytes, Some(&path))?;
        load_battery(&mut replacement, &path)?;
        let hash = replacement.rom_hash();
        self.per_game = settings::load_per_game(hash);
        self.speed_index = if self.play_mode().restricts_assists() {
            NORMAL_SPEED_INDEX
        } else {
            self.per_game
                .speed_index
                .unwrap_or(self.settings.emulation.speed_index)
                .min(SPEEDS.len() - 1)
        };
        self.nes = Some(replacement);
        let palette_note = self.apply_video_palette().err();
        self.rom_path = Some(path.clone());
        self.rom_bytes = bytes;
        self.powered = true;
        self.paused = false;
        self.frame_budget = 0.0;
        self.frame_dirty = true;
        self.clear_rewind_history();
        self.tas = Default::default();
        self.tas_held_input = TasFrame::default();
        self.tas.checkpoint_interval = self.settings.tas.checkpoint_interval.max(1);
        self.lag_frames = 0;
        self.last_controller_reads = 0;
        self.selected_slot = self
            .settings
            .save_states
            .selected_slot
            .min(self.settings.save_states.slots.saturating_sub(1));
        self.refresh_slots();
        self.state_preview = None;
        self.preview_slot = None;
        self.library.remember(&path)?;
        self.refresh_library_and_artwork();
        self.clear_audio_pipeline();
        self.page = MainPage::Game;
        self.status = match (inferred_region, palette_note) {
            (Some(Region::Pal), Some(note)) => {
                format!("ROM loaded with PAL timing inferred from filename; {note}")
            }
            (Some(Region::Pal), None) => "ROM loaded with PAL timing inferred from filename".into(),
            (_, Some(note)) => format!("ROM loaded; {note}"),
            (_, None) => "ROM loaded".into(),
        };
        if self.play_mode() == PlayMode::Achievement && self.achievements.user().is_some() {
            self.achievement_toasts.clear();
            self.achievement_known_unlocked.clear();
            self.achievement_game_mastered = false;
            if let Err(error) = self.achievements.load_game(&path, &self.rom_bytes) {
                self.status = format!("ROM loaded; achievements could not start: {error}");
            } else {
                self.status = "ROM loaded; loading RetroAchievements set…".into();
            }
        }
        Ok(())
    }

    fn apply_video_palette(&mut self) -> Result<Option<String>, String> {
        if let Some(palette) = self.nes.as_ref().and_then(Nes::native_output_palette) {
            if let Some(nes) = &mut self.nes {
                nes.set_output_palette(palette);
            }
            self.frame_dirty = true;
            return Ok(Some(format!(
                "VS RP2C04-0004 palette selected; {} inserts a coin, Select starts 1 player, and Start starts 2 players",
                self.settings.input.vs_coin_binding.label()
            )));
        }
        let (palette, warning) = match self.settings.video.palette_mode {
            PaletteMode::Ntsc2c02 => (NTSC_2C02_PALETTE, None),
            PaletteMode::Rgb2c03 => (RGB_2C03_PALETTE, None),
            PaletteMode::Custom => {
                let Some(path) = self.settings.video.custom_palette_path.as_deref() else {
                    if let Some(nes) = &mut self.nes {
                        nes.set_output_palette(NTSC_2C02_PALETTE);
                    }
                    self.frame_dirty = true;
                    return Err("custom palette is not set; using the default NTSC palette".into());
                };
                match palettes::load(path) {
                    Ok(loaded) => (loaded.colors, loaded.warning),
                    Err(error) => {
                        if let Some(nes) = &mut self.nes {
                            nes.set_output_palette(NTSC_2C02_PALETTE);
                        }
                        self.frame_dirty = true;
                        return Err(format!(
                            "could not load custom palette ({}); using the default NTSC palette",
                            error
                        ));
                    }
                }
            }
        };
        if let Some(nes) = &mut self.nes {
            nes.set_output_palette(palette);
        }
        self.frame_dirty = true;
        Ok(warning)
    }

    fn apply_video_palette_with_status(&mut self) {
        match self.apply_video_palette() {
            Ok(Some(warning)) => self.status = warning,
            Ok(None) => {
                self.status = format!(
                    "Video palette changed to {}",
                    self.settings.video.palette_mode.label()
                );
            }
            Err(error) => self.status = error,
        }
    }

    fn import_custom_palette(&mut self) {
        let Some(path) = FileDialog::new()
            .set_title("Import NES color palette")
            .add_filter("NES palette", &["pal", "txt"])
            .pick_file()
        else {
            return;
        };
        match palettes::import(&path) {
            Ok((stored_path, loaded)) => {
                self.settings.video.custom_palette_path = Some(stored_path);
                self.settings.video.palette_mode = PaletteMode::Custom;
                self.settings_dirty = true;
                if let Some(nes) = &mut self.nes {
                    nes.set_output_palette(loaded.colors);
                }
                self.frame_dirty = true;
                self.status = loaded
                    .warning
                    .unwrap_or_else(|| "Custom palette imported and applied immediately".into());
            }
            Err(error) => self.status = format!("Could not import palette: {error}"),
        }
    }
    fn save_battery(&self) -> Result<(), Box<dyn Error>> {
        if let (Some(nes), Some(path)) = (&self.nes, &self.rom_path)
            && nes.has_battery()
            && let Some(data) = nes.battery_ram()
        {
            persistence::atomic_write(&persistence::battery_path(path), data)?;
        }
        Ok(())
    }
    fn save_per_game(&mut self) {
        if let Some(nes) = &self.nes
            && let Err(e) = settings::save_per_game(nes.rom_hash(), &self.per_game)
        {
            self.status = format!("Could not save per-game settings: {e}");
        }
    }
    fn new_tas_movie(&mut self, start_type: TasStartType) {
        if self.play_mode().restricts_assists() {
            self.status = format!(
                "TAS tools are disabled in {} mode",
                self.play_mode().label()
            );
            return;
        }
        if self.nes.is_none() || self.rom_bytes.is_empty() {
            self.status = "Load a ROM before creating a TAS movie".into();
            return;
        }
        if start_type == TasStartType::PowerOn {
            match nes_from_rom_path(&self.rom_bytes, self.rom_path.as_deref()) {
                Ok(nes) => self.nes = Some(nes),
                Err(error) => {
                    self.status = format!("Could not create power-on state: {error}");
                    return;
                }
            }
        } else if start_type == TasStartType::Reset
            && let Some(nes) = &mut self.nes
        {
            nes.reset();
        }
        let _ = self.apply_video_palette();
        // Host-facing queued PCM is not part of deterministic movie state.
        self.clear_audio_pipeline();
        let Some(nes) = &self.nes else {
            return;
        };
        let initial_state = match nes.save_state() {
            Ok(state) => state,
            Err(error) => {
                self.status = format!("Could not capture TAS start: {error}");
                return;
            }
        };
        let embedded = (start_type != TasStartType::PowerOn).then(|| initial_state.clone());
        let movie = TasMovie::new(tas::rom_sha256_hex(nes.rom_sha256()), start_type, embedded);
        self.tas.new_movie(movie, initial_state);
        self.tas_held_input = TasFrame::default();
        self.powered = true;
        self.paused = false;
        self.fast_forward = false;
        self.last_controller_reads = nes
            .controller_reads(0)
            .wrapping_add(nes.controller_reads(1));
        self.clear_rewind_history();
        self.lag_frames = 0;
        self.frame_dirty = true;
        self.follow_tas_cursor();
        self.clear_audio_pipeline();
        self.status = format!("Recording new {start_type:?} TAS at 1x");
    }

    fn restore_tas_start(&mut self) -> Result<Vec<u8>, String> {
        if self.play_mode().restricts_assists() {
            return Err(format!(
                "TAS tools are disabled in {} mode",
                self.play_mode().label()
            ));
        }
        let (start_type, starting_state) = self
            .tas
            .movie
            .as_ref()
            .map(|movie| (movie.start_type, movie.starting_state.clone()))
            .ok_or_else(|| "no TAS movie loaded".to_owned())?;
        match start_type {
            TasStartType::PowerOn => {
                self.nes = Some(
                    nes_from_rom_path(&self.rom_bytes, self.rom_path.as_deref())
                        .map_err(|e| e.to_string())?,
                );
            }
            TasStartType::Reset | TasStartType::SaveState => {
                let nes = self
                    .nes
                    .as_mut()
                    .ok_or_else(|| "no ROM loaded".to_owned())?;
                if let Some(state) = starting_state {
                    nes.load_state(&state).map_err(|e| e.to_string())?;
                } else if start_type == TasStartType::Reset {
                    nes.reset();
                } else {
                    return Err("save-state movie has no embedded starting state".into());
                }
            }
        }
        let _ = self.apply_video_palette();
        self.nes
            .as_ref()
            .unwrap()
            .save_state()
            .map_err(|e| e.to_string())
    }

    fn start_tas_playback(&mut self, read_only: bool) {
        match self.restore_tas_start() {
            Ok(initial_state) => {
                if self.tas.start_playback(read_only) {
                    self.tas.checkpoints = vec![tas::TasCheckpoint {
                        frame: 0,
                        state: initial_state.clone(),
                    }];
                    let refreshed_start = self.tas.maybe_checkpoint(0, initial_state.clone());
                    if refreshed_start {
                        // Playback has just reconstructed the declared starting
                        // condition, so a frame-zero mismatch is necessarily a
                        // stale serialized-state checksum rather than input
                        // divergence.
                        self.tas.repair_checkpoint_checksum(0, &initial_state);
                    }
                    self.lag_frames = 0;
                    if let Some(nes) = &self.nes {
                        self.last_controller_reads = nes
                            .controller_reads(0)
                            .wrapping_add(nes.controller_reads(1));
                    }
                    self.paused = false;
                    self.powered = true;
                    self.clear_rewind_history();
                    self.frame_dirty = true;
                    self.follow_tas_cursor();
                    self.clear_audio_pipeline();
                    self.status = if refreshed_start {
                        "TAS playback started; refreshed stale frame-0 checkpoint metadata".into()
                    } else if read_only {
                        "Read-only TAS playback".into()
                    } else {
                        "TAS playback".into()
                    };
                }
            }
            Err(error) => self.status = format!("TAS start failed: {error}"),
        }
    }

    fn seek_tas(&mut self, target: usize) -> bool {
        let total = self
            .tas
            .movie
            .as_ref()
            .map_or(0, |movie| movie.frames.len());
        let target = target.min(total);
        if self.tas.checkpoints.is_empty() {
            let Ok(initial_state) = self.restore_tas_start() else {
                self.status = "Could not restore the TAS starting condition".into();
                return false;
            };
            self.tas.checkpoints.push(tas::TasCheckpoint {
                frame: 0,
                state: initial_state,
            });
        }
        let Some(checkpoint) = self.tas.checkpoint_at_or_before(target) else {
            self.status = "No TAS checkpoint is available".into();
            return false;
        };
        let frames = self
            .tas
            .movie
            .as_ref()
            .map(|movie| movie.frames[checkpoint.frame..target].to_vec())
            .unwrap_or_default();
        {
            let Some(nes) = &mut self.nes else {
                return false;
            };
            if let Err(error) = nes.load_state(&checkpoint.state) {
                self.status = format!("Checkpoint load failed: {error}");
                return false;
            }
        }
        for (offset, input) in frames.into_iter().enumerate() {
            let next_frame = checkpoint.frame + offset + 1;
            let checkpoint_state = {
                let Some(nes) = &mut self.nes else {
                    return false;
                };
                set_controller_mask(nes, 0, input.player1);
                set_controller_mask(nes, 1, input.player2);
                if let Err(error) = nes.run_frame() {
                    self.status = format!("Seek stopped: {error}");
                    return false;
                }
                self.audio_scratch.clear();
                nes.drain_audio_samples(&mut self.audio_scratch);
                (next_frame % self.tas.checkpoint_interval.max(1) == 0)
                    .then(|| nes.save_state().ok())
                    .flatten()
            };
            if let Some(state) = checkpoint_state
                && self.tas.maybe_checkpoint(next_frame, state)
                && self.reconcile_tas_checkpoint(next_frame) == TasCheckpointRecovery::Unrecoverable
            {
                self.status =
                    self.tas.last_desync.clone().unwrap_or_else(|| {
                        format!("TAS seek could not reconcile frame {next_frame}")
                    });
                self.tas.pause();
                self.paused = true;
                self.follow_tas_cursor();
                return false;
            }
        }
        self.tas.set_cursor_paused_for_preview(target);
        self.tas_timeline_scroll = Some(target);
        self.paused = true;
        self.frame_dirty = true;
        self.clear_audio_pipeline();
        self.status = format!("Seeked to TAS frame {target}");
        true
    }

    fn apply_tas_timeline_action(&mut self, action: TasTimelineAction) {
        if self.play_mode().restricts_assists() {
            self.status = format!(
                "TAS tools are disabled in {} mode",
                self.play_mode().label()
            );
            return;
        }
        if matches!(action, TasTimelineAction::Copy) {
            if self.tas.copy_selection() {
                self.status = format!("Copied {} TAS frame(s)", self.tas.clipboard.len());
            }
            return;
        }
        if matches!(
            action,
            TasTimelineAction::Paste | TasTimelineAction::InsertPaste
        ) {
            let start = self.tas.selected_frame;
            if self
                .tas
                .paste_selection(matches!(action, TasTimelineAction::InsertPaste))
            {
                self.paused = true;
                self.tas.pause();
                self.frame_budget = 0.0;
                self.clear_audio_pipeline();
                self.status = format!(
                    "Pasted TAS input at frame {start}; paused without moving the timeline"
                );
            }
            return;
        }
        if !self.tas.editable() {
            self.status = "Read-only TAS playback cannot be edited".into();
            return;
        }
        let selected = self.tas.selected_frame;
        let range_end = self.tas.range_end_frame;
        let Some(movie) = &mut self.tas.movie else {
            return;
        };
        let changed = match action {
            TasTimelineAction::InsertBlank => TasEditor::insert(
                movie,
                selected.min(movie.frames.len()),
                &[TasFrame::default()],
            ),
            TasTimelineAction::Duplicate => {
                movie.frames.get(selected).copied().is_some_and(|input| {
                    TasEditor::insert(movie, (selected + 1).min(movie.frames.len()), &[input])
                })
            }
            TasTimelineAction::Delete => TasEditor::delete(movie, selected, range_end),
            TasTimelineAction::Fill => movie
                .frames
                .get(selected)
                .copied()
                .is_some_and(|input| TasEditor::fill(movie, selected, range_end, input)),
            TasTimelineAction::Clear => TasEditor::clear(movie, selected, range_end),
            TasTimelineAction::Copy | TasTimelineAction::Paste | TasTimelineAction::InsertPaste => {
                false
            }
        };
        if changed {
            let changed_frame = selected.min(range_end);
            self.tas.invalidate_after(changed_frame);
            let last = self
                .tas
                .movie
                .as_ref()
                .map_or(0, |movie| movie.frames.len().saturating_sub(1));
            if matches!(action, TasTimelineAction::Duplicate) {
                self.tas.selected_frame = selected.saturating_add(1).min(last);
                self.tas.range_end_frame = self.tas.selected_frame;
            } else {
                self.tas.selected_frame = self.tas.selected_frame.min(last);
                self.tas.range_end_frame = self.tas.range_end_frame.min(last);
            }
            self.paused = true;
            self.tas.pause();
            self.frame_budget = 0.0;
            self.clear_audio_pipeline();
            self.status =
                "TAS timeline edited without seeking; future checkpoints invalidated".into();
        }
    }

    fn export_tas(&mut self) {
        if self.play_mode().restricts_assists() {
            self.status = format!(
                "TAS tools are disabled in {} mode",
                self.play_mode().label()
            );
            return;
        }
        if let (Some(nes), Some(movie)) = (&self.nes, &self.tas.movie) {
            let dir = settings::tas_root().join(format!("{:016x}", nes.rom_hash()));
            let _ = fs::create_dir_all(&dir);
            if let Some(path) = FileDialog::new()
                .set_directory(dir)
                .add_filter("CrabNes TAS", &["tas"])
                .set_file_name("movie.tas")
                .save_file()
            {
                match tas::save(movie, &path) {
                    Ok(()) => {
                        self.tas.log(format!("saved {}", path.display()));
                        self.status = format!("Saved {}", path.display());
                    }
                    Err(e) => {
                        self.tas.log(format!("movie save failed: {e}"));
                        self.status = format!("Save failed: {e}");
                    }
                }
            }
        }
    }
    fn import_tas(&mut self) {
        if self.play_mode().restricts_assists() {
            self.status = format!(
                "TAS tools are disabled in {} mode",
                self.play_mode().label()
            );
            return;
        }
        if let Some(nes) = &self.nes
            && let Some(path) = FileDialog::new()
                .set_directory(settings::tas_root().join(format!("{:016x}", nes.rom_hash())))
                .add_filter("CrabNes TAS", &["tas"])
                .pick_file()
        {
            let expected = tas::rom_sha256_hex(nes.rom_sha256());
            match tas::load(&path, &expected) {
                Ok(loaded) => {
                    let warning = loaded.warnings.join("; ");
                    self.tas.install_movie(loaded.movie);
                    self.tas_held_input = TasFrame::default();
                    self.tas.log(format!("loaded {}", path.display()));
                    self.status = if warning.is_empty() {
                        format!("Loaded {}", path.display())
                    } else {
                        format!("Loaded with warning: {warning}")
                    };
                }
                Err(e @ tas::TasFormatError::WrongRom { .. }) => {
                    self.tas.log(format!("ROM hash mismatch: {e}"));
                    self.status = format!("ROM mismatch warning: {e}");
                }
                Err(e) => {
                    self.tas.log(format!("movie load rejected: {e}"));
                    self.status = format!("Load failed: {e}");
                }
            }
        }
    }

    fn play_mode(&self) -> PlayMode {
        self.active_play_mode
    }

    fn apply_play_mode(&mut self, mode: PlayMode) {
        let previous = self.active_play_mode;
        if previous == PlayMode::Achievement && mode != PlayMode::Achievement {
            self.achievements.unload_game();
            self.show_achievements = false;
            self.achievement_toasts.clear();
            self.achievement_known_unlocked.clear();
            self.achievement_game_mastered = false;
        }
        self.active_play_mode = mode;
        self.fast_forward = false;
        self.frame_budget = 0.0;
        if mode.restricts_assists() {
            self.speed_index = NORMAL_SPEED_INDEX;
            self.show_states = false;
            self.show_time = false;
            self.show_tas = false;
            self.show_tas_control = false;
            self.show_debugger = false;
            self.show_hex = false;
            self.frame_advance_held = false;
            self.rewind_active = false;
            self.resume_after_rewind = false;
            self.clear_rewind_history();
            self.tas.stop();
            if self.powered {
                self.paused = false;
            }
            self.clear_audio_pipeline();
            self.status = format!("{} mode enabled — emulator assists disabled", mode.label());
            if mode == PlayMode::Achievement && previous != PlayMode::Achievement {
                self.start_achievement_session();
            }
        } else {
            self.speed_index = self
                .per_game
                .speed_index
                .unwrap_or(self.settings.emulation.speed_index)
                .min(SPEEDS.len() - 1);
            self.status = "Standard mode enabled".into();
        }
    }

    fn start_achievement_session(&mut self) {
        self.show_achievements = true;
        if self.achievements.user().is_some() {
            self.load_current_achievement_game();
            return;
        }

        let username = self.settings.achievements.username.trim();
        let token = self.settings.achievements.token.trim();
        if username.is_empty() || token.is_empty() {
            self.status = "Achievement mode enabled — sign in to RetroAchievements".into();
            return;
        }
        match self.achievements.login_token(username, token) {
            Ok(()) => self.status = "Signing in to RetroAchievements…".into(),
            Err(error) => self.status = format!("RetroAchievements sign-in failed: {error}"),
        }
    }

    fn load_current_achievement_game(&mut self) {
        let Some(path) = self.rom_path.as_deref() else {
            return;
        };
        if self.rom_bytes.is_empty() {
            return;
        }
        self.achievement_known_unlocked.clear();
        self.achievement_game_mastered = false;
        match self.achievements.load_game(path, &self.rom_bytes) {
            Ok(()) => self.status = "Loading RetroAchievements set…".into(),
            Err(error) => self.status = format!("Could not load achievement set: {error}"),
        }
    }

    fn handle_achievement_events(&mut self, events: Vec<AchievementEvent>) {
        for event in events {
            match event.kind {
                AchievementEventKind::Login if event.result == 0 => {
                    self.achievement_password.clear();
                    if let Some(user) = self.achievements.user() {
                        self.settings.achievements.username = user.username;
                        self.settings.achievements.token = user.token;
                        self.settings_dirty = true;
                        self.status =
                            format!("Signed in to RetroAchievements as {}", user.display_name);
                    } else {
                        self.status = "Signed in to RetroAchievements".into();
                    }
                    self.load_current_achievement_game();
                }
                AchievementEventKind::Login => {
                    self.settings.achievements.token.clear();
                    self.settings_dirty = true;
                    self.status = if event.message.is_empty() {
                        "RetroAchievements sign-in failed".into()
                    } else {
                        format!("RetroAchievements sign-in failed: {}", event.message)
                    };
                }
                AchievementEventKind::GameLoad if event.result == 0 => {
                    let rows = self.achievements.achievements();
                    let (known_unlocked, mastered) = achievement_unlock_baseline(&rows);
                    self.achievement_known_unlocked = known_unlocked;
                    self.achievement_game_mastered = mastered;
                    if let Some(game) = self.achievements.game() {
                        self.status = format!("Achievements loaded: {} (hardcore)", game.title);
                    } else {
                        self.status = "Achievement set loaded (hardcore)".into();
                    }
                }
                AchievementEventKind::GameLoad => {
                    self.status = if event.message.is_empty() {
                        "No RetroAchievements set was found for this ROM".into()
                    } else {
                        format!("Achievement set not loaded: {}", event.message)
                    };
                }
                AchievementEventKind::Achievement => {
                    let details = self
                        .achievements
                        .achievements()
                        .into_iter()
                        .find(|achievement| achievement.id == event.id);
                    if event.id == 0
                        || is_achievement_warning_title(&event.title)
                        || details.as_ref().is_some_and(is_achievement_client_warning)
                    {
                        self.status = if event.message.is_empty() {
                            event.title
                        } else {
                            event.message
                        };
                        continue;
                    }
                    let (first_unlock, show_notification) = register_achievement_unlock(
                        &mut self.achievement_known_unlocked,
                        event.id,
                        self.settings.achievements.show_replayed_unlocks,
                    );
                    if !show_notification {
                        continue;
                    }
                    let game = self.achievements.game();
                    let description = details
                        .as_ref()
                        .map(|achievement| achievement.description.clone())
                        .filter(|description| !description.is_empty())
                        .unwrap_or_else(|| event.message.clone());
                    let badge_url = details
                        .as_ref()
                        .map(|achievement| achievement.badge_url.clone())
                        .unwrap_or_default();
                    self.ensure_achievement_badge(&badge_url);
                    self.achievement_toasts.push_back(AchievementToast {
                        title: event.title.clone(),
                        description: description.clone(),
                        points: event.points,
                        badge_url: badge_url.clone(),
                        started_at: None,
                    });
                    if first_unlock && let Some(game) = game {
                        let archive_result = self.achievement_archive.record(UnlockEntry {
                            game_id: game.id,
                            achievement_id: event.id,
                            game_title: game.title,
                            title: event.title.clone(),
                            description,
                            points: event.points,
                            badge_url,
                            unlocked_at: 0,
                        });
                        if let Err(error) = archive_result {
                            self.push_achievement_activity(format!(
                                "Could not save unlock archive: {error}"
                            ));
                        }
                    }
                    let text = format!("Unlocked: {} (+{} points)", event.title, event.points);
                    self.push_achievement_activity(text.clone());
                    self.status = text;
                }
                AchievementEventKind::GameCompleted => {
                    let already_mastered = self.achievement_game_mastered;
                    self.achievement_game_mastered = true;
                    if already_mastered && !self.settings.achievements.show_replayed_unlocks {
                        continue;
                    }
                    let text = "Game mastered — all core achievements unlocked".to_owned();
                    self.push_achievement_activity(text.clone());
                    self.status = text;
                }
                AchievementEventKind::Reset => {
                    self.reset();
                    self.achievement_toasts.clear();
                    self.status = "Reset for RetroAchievements hardcore mode".into();
                }
                AchievementEventKind::Disconnected
                | AchievementEventKind::Reconnected
                | AchievementEventKind::Leaderboard
                | AchievementEventKind::ServerError => {
                    let text = if event.message.is_empty() {
                        event.title
                    } else if event.title.is_empty() {
                        event.message
                    } else {
                        format!("{}: {}", event.title, event.message)
                    };
                    if !text.is_empty() {
                        self.push_achievement_activity(text.clone());
                        self.status = text;
                    }
                }
                AchievementEventKind::Unknown => {}
            }
        }
    }

    fn push_achievement_activity(&mut self, item: String) {
        self.achievement_feed.push_front(item);
        self.achievement_feed.truncate(8);
    }

    fn ensure_achievement_badge(&mut self, url: &str) {
        if url.is_empty()
            || self.achievement_badges.contains_key(url)
            || !self.achievement_badges_requested.insert(url.to_owned())
        {
            return;
        }
        self.achievements.request_badge(url.to_owned());
    }

    fn collect_achievement_badges(&mut self, ctx: &egui::Context) {
        for badge in self.achievements.take_badge_images() {
            let image = ColorImage::from_rgba_unmultiplied(badge.size, &badge.rgba);
            let texture = ctx.load_texture(
                format!("achievement-badge:{}", badge.url),
                image,
                TextureOptions::LINEAR,
            );
            self.achievement_badges.insert(badge.url, texture);
        }
    }

    fn achievement_toast_overlay(&mut self, ctx: &egui::Context) {
        if self.play_mode() != PlayMode::Achievement {
            return;
        }
        let now = Instant::now();
        if let Some(toast) = self.achievement_toasts.front_mut()
            && toast.started_at.is_none()
        {
            toast.started_at = Some(now);
        }
        let Some(toast) = self.achievement_toasts.front().cloned() else {
            return;
        };
        let elapsed = now
            .duration_since(toast.started_at.unwrap_or(now))
            .as_secs_f32();
        const DISPLAY_SECONDS: f32 = 5.5;
        if elapsed >= DISPLAY_SECONDS {
            self.achievement_toasts.pop_front();
            ctx.request_repaint();
            return;
        }

        let enter = (elapsed / 0.28).clamp(0.0, 1.0);
        let enter = 1.0 - (1.0 - enter).powi(3);
        let exit = ((DISPLAY_SECONDS - elapsed) / 0.45).clamp(0.0, 1.0);
        let alpha = enter.min(exit);
        let y = 58.0 - (1.0 - enter) * 100.0;
        let text_color =
            egui::Color32::from_rgba_unmultiplied(242, 239, 190, (255.0 * alpha) as u8);
        let muted_color =
            egui::Color32::from_rgba_unmultiplied(190, 190, 190, (255.0 * alpha) as u8);
        let texture = self.achievement_badges.get(&toast.badge_url).cloned();

        egui::Area::new(egui::Id::new("achievement-unlock-toast"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(24.0, y))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_unmultiplied(
                        10,
                        11,
                        13,
                        (242.0 * alpha) as u8,
                    ))
                    .stroke(egui::Stroke::new(
                        2.0,
                        egui::Color32::from_rgba_unmultiplied(102, 106, 112, (255.0 * alpha) as u8),
                    ))
                    .corner_radius(12)
                    .inner_margin(12)
                    .show(ui, |ui| {
                        let toast_width = (ctx.content_rect().width() - 48.0).clamp(240.0, 430.0);
                        ui.set_width(toast_width);
                        ui.horizontal_wrapped(|ui| {
                            if let Some(texture) = &texture {
                                ui.add(
                                    egui::Image::new(texture)
                                        .fit_to_exact_size(Vec2::splat(76.0))
                                        .corner_radius(5),
                                );
                            } else {
                                egui::Frame::new()
                                    .fill(egui::Color32::from_rgba_unmultiplied(
                                        35,
                                        36,
                                        42,
                                        (255.0 * alpha) as u8,
                                    ))
                                    .corner_radius(5)
                                    .show(ui, |ui| {
                                        ui.allocate_space(Vec2::splat(76.0));
                                    });
                            }
                            ui.add_space(6.0);
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new("ACHIEVEMENT UNLOCKED")
                                        .small()
                                        .strong()
                                        .color(egui::Color32::from_rgba_unmultiplied(
                                            222,
                                            189,
                                            78,
                                            (255.0 * alpha) as u8,
                                        )),
                                );
                                ui.label(
                                    egui::RichText::new(&toast.title)
                                        .size(20.0)
                                        .strong()
                                        .color(text_color),
                                );
                                if !toast.description.is_empty() {
                                    ui.label(
                                        egui::RichText::new(&toast.description)
                                            .small()
                                            .color(muted_color),
                                    );
                                }
                                ui.label(
                                    egui::RichText::new(format!("+{} points", toast.points))
                                        .strong()
                                        .color(text_color),
                                );
                            });
                        });
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn current_speed(&self) -> f64 {
        if self.play_mode().restricts_assists() {
            return 1.0;
        }
        if self.tas.mode == TasMode::Recording {
            1.0
        } else if self.fast_forward {
            4.0
        } else {
            SPEEDS[self
                .per_game
                .speed_index
                .unwrap_or(self.speed_index)
                .min(SPEEDS.len() - 1)]
        }
    }
    fn emulation_frame_rate(&self) -> f64 {
        self.nes.as_ref().map_or(NTSC_FRAME_RATE, Nes::frame_rate)
    }
    fn effective_volume(&self) -> f32 {
        self.per_game
            .volume
            .unwrap_or(self.settings.audio.volume)
            .clamp(0.0, 1.0)
    }
    fn effective_muted(&self) -> bool {
        self.per_game.muted.unwrap_or(self.settings.audio.muted)
    }
    fn take_screenshot(&mut self) {
        if let (Some(nes), Some(path)) = (&self.nes, &self.rom_path) {
            match screenshot::save(nes.frame(), path) {
                Ok(p) => self.status = format!("Screenshot: {}", p.display()),
                Err(e) => self.status = format!("Screenshot failed: {e}"),
            }
        }
    }
}

fn floating_window_max_size(ctx: &egui::Context) -> Vec2 {
    let available = ctx.content_rect().size();
    Vec2::new(
        (available.x - 24.0).max(280.0),
        (available.y - 24.0).max(180.0),
    )
}

fn inferred_region_from_rom_path(path: &Path) -> Option<Region> {
    let file_name = path.file_name()?.to_str()?.to_ascii_lowercase();
    const PAL_TAGS: [&str; 11] = [
        "(europe)",
        "(australia)",
        "(pal)",
        "[pal]",
        "(france)",
        "(germany)",
        "(italy)",
        "(spain)",
        "(sweden)",
        "(netherlands)",
        "(united kingdom)",
    ];
    PAL_TAGS
        .iter()
        .any(|tag| file_name.contains(tag))
        .then_some(Region::Pal)
}

fn nes_from_rom_path(bytes: &[u8], path: Option<&Path>) -> Result<Nes, nes_core::EmulationError> {
    match path.and_then(inferred_region_from_rom_path) {
        Some(region) => Nes::from_ines_with_region(bytes, region),
        None => Nes::from_ines(bytes),
    }
}

fn is_achievement_client_warning(achievement: &nes_achievements_native::Achievement) -> bool {
    achievement.bucket == AchievementBucket::Unsupported
        || achievement.id == 0
        || is_achievement_warning_title(&achievement.title)
}

fn achievement_unlock_baseline(
    achievements: &[nes_achievements_native::Achievement],
) -> (HashSet<u32>, bool) {
    let mut known_unlocked = HashSet::new();
    let mut core_count = 0usize;
    let mut core_unlocked = 0usize;
    for achievement in achievements
        .iter()
        .filter(|achievement| !is_achievement_client_warning(achievement))
    {
        if achievement.unlocked {
            known_unlocked.insert(achievement.id);
        }
        if achievement.bucket != AchievementBucket::Unofficial {
            core_count += 1;
            core_unlocked += usize::from(achievement.unlocked);
        }
    }
    (
        known_unlocked,
        core_count != 0 && core_unlocked == core_count,
    )
}

fn register_achievement_unlock(
    known_unlocked: &mut HashSet<u32>,
    achievement_id: u32,
    show_replayed: bool,
) -> (bool, bool) {
    let first_unlock = known_unlocked.insert(achievement_id);
    (first_unlock, first_unlock || show_replayed)
}

fn is_achievement_warning_title(title: &str) -> bool {
    title
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Warning:"))
        || title.eq_ignore_ascii_case("Unsupported Game Version")
}

fn is_archived_achievement_warning(entry: &UnlockEntry) -> bool {
    entry.achievement_id == 0 || is_achievement_warning_title(&entry.title)
}

fn achievement_warning_banner(ui: &mut egui::Ui, warning: &nes_achievements_native::Achievement) {
    let warning_title = warning
        .title
        .get(8..)
        .filter(|_| {
            warning
                .title
                .get(..8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Warning:"))
        })
        .unwrap_or(&warning.title)
        .trim();
    let heading = if warning_title.eq_ignore_ascii_case("Unknown Emulator") {
        "Emulator verification required"
    } else if warning_title.is_empty() {
        "RetroAchievements warning"
    } else {
        warning_title
    };
    ui.add_space(8.0);
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(55, 43, 22))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(184, 130, 39),
        ))
        .corner_radius(7)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("!")
                        .size(22.0)
                        .strong()
                        .color(egui::Color32::from_rgb(244, 191, 73)),
                );
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(heading)
                            .strong()
                            .color(egui::Color32::from_rgb(246, 208, 125)),
                    );
                    ui.label(
                        egui::RichText::new(&warning.description)
                            .small()
                            .color(egui::Color32::from_rgb(220, 205, 174)),
                    );
                });
            });
        });
}

fn achievement_card(
    ui: &mut egui::Ui,
    achievement: &nes_achievements_native::Achievement,
    texture: Option<&TextureHandle>,
) {
    let fill = if achievement.unlocked {
        egui::Color32::from_rgb(31, 43, 38)
    } else {
        egui::Color32::from_rgb(29, 30, 35)
    };
    let stroke = if achievement.unlocked {
        egui::Color32::from_rgb(67, 137, 91)
    } else {
        egui::Color32::from_rgb(57, 59, 68)
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(8)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                achievement_badge(ui, texture, 64.0);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(&achievement.title)
                                .strong()
                                .size(16.0)
                                .color(if achievement.unlocked {
                                    egui::Color32::from_rgb(225, 239, 228)
                                } else {
                                    egui::Color32::from_rgb(220, 220, 224)
                                }),
                        );
                        ui.label(
                            egui::RichText::new(format!("{} pts", achievement.points))
                                .small()
                                .strong()
                                .color(egui::Color32::from_rgb(224, 188, 78)),
                        );
                        if achievement.unlocked {
                            ui.label(
                                egui::RichText::new("UNLOCKED")
                                    .small()
                                    .strong()
                                    .color(egui::Color32::from_rgb(91, 205, 125)),
                            );
                        }
                    });
                    ui.label(
                        egui::RichText::new(&achievement.description)
                            .small()
                            .color(egui::Color32::from_gray(180)),
                    );
                    if !achievement.measured_progress.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.add(
                                egui::ProgressBar::new(
                                    (achievement.measured_percent / 100.0).clamp(0.0, 1.0),
                                )
                                .desired_width(ui.available_width().min(180.0)),
                            );
                            ui.small(&achievement.measured_progress);
                        });
                    }
                });
            });
        });
}

fn archive_card(ui: &mut egui::Ui, entry: &UnlockEntry, texture: Option<&TextureHandle>) {
    let unlocked_at = chrono::DateTime::from_timestamp(entry.unlocked_at, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%b %-d, %Y · %-I:%M %p")
                .to_string()
        })
        .unwrap_or_else(|| "Unknown time".into());
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(31, 38, 35))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(61, 105, 78)))
        .corner_radius(8)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                achievement_badge(ui, texture, 64.0);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new(&entry.title).strong().size(16.0));
                        ui.label(
                            egui::RichText::new(format!("{} pts", entry.points))
                                .small()
                                .strong()
                                .color(egui::Color32::from_rgb(224, 188, 78)),
                        );
                    });
                    if !entry.description.is_empty() {
                        ui.label(
                            egui::RichText::new(&entry.description)
                                .small()
                                .color(egui::Color32::from_gray(185)),
                        );
                    }
                    ui.label(
                        egui::RichText::new(format!("{} · {}", entry.game_title, unlocked_at))
                            .small()
                            .color(egui::Color32::from_gray(140)),
                    );
                });
            });
        });
}

fn achievement_badge(ui: &mut egui::Ui, texture: Option<&TextureHandle>, size: f32) {
    if let Some(texture) = texture {
        ui.add(
            egui::Image::new(texture)
                .fit_to_exact_size(Vec2::splat(size))
                .corner_radius(5),
        );
    } else {
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(43, 44, 51))
            .corner_radius(5)
            .show(ui, |ui| {
                ui.allocate_ui(Vec2::splat(size), |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new("RA")
                                .size(18.0)
                                .color(egui::Color32::from_gray(110)),
                        );
                    });
                });
            });
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

fn decode_cover_image(path: &Path) -> Result<ColorImage, String> {
    const MAX_COVER_FILE_BYTES: u64 = 32 * 1024 * 1024;
    let size = fs::metadata(path).map_err(|error| error.to_string())?.len();
    if size > MAX_COVER_FILE_BYTES {
        return Err("cover image is larger than 32 MiB".into());
    }
    let mut reader = image::ImageReader::open(path)
        .map_err(|error| error.to_string())?
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8_192);
    limits.max_image_height = Some(8_192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let rgba = reader
        .decode()
        .map_err(|error| error.to_string())?
        .into_rgba8();
    let dimensions = [rgba.width() as usize, rgba.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(
        dimensions,
        rgba.as_raw(),
    ))
}

fn palette_settings_ui(
    ui: &mut egui::Ui,
    id: &'static str,
    video: &mut VideoSettings,
    import_requested: &mut bool,
) -> bool {
    let old_mode = video.palette_mode;
    egui::ComboBox::from_id_salt(id)
        .selected_text(video.palette_mode.label())
        .show_ui(ui, |ui| {
            for mode in [
                PaletteMode::Ntsc2c02,
                PaletteMode::Rgb2c03,
                PaletteMode::Custom,
            ] {
                ui.selectable_value(&mut video.palette_mode, mode, mode.label());
            }
        });
    ui.horizontal_wrapped(|ui| {
        if ui.button("Import palette…").clicked() {
            *import_requested = true;
        }
        if video.palette_mode == PaletteMode::Rgb2c03 {
            ui.small("RGB DAC colors used by RP2C03 / PlayChoice-10");
        }
    });
    if let Some(path) = video.custom_palette_path.as_deref() {
        ui.small(format!("Custom: {}", path.display()));
    } else if video.palette_mode == PaletteMode::Custom {
        ui.colored_label(
            egui::Color32::YELLOW,
            "Import a 64-color palette before selecting Custom.",
        );
    }

    let preview = match video.palette_mode {
        PaletteMode::Ntsc2c02 => Some(NTSC_2C02_PALETTE),
        PaletteMode::Rgb2c03 => Some(RGB_2C03_PALETTE),
        PaletteMode::Custom => video
            .custom_palette_path
            .as_deref()
            .and_then(|path| palettes::load(path).ok())
            .map(|loaded| loaded.colors),
    };
    if let Some(palette) = preview {
        palette_preview(ui, &palette);
    }
    old_mode != video.palette_mode
}

fn crt_signature(
    video: &VideoSettings,
) -> (bool, CrtProfile, CrtMask, u32, u32, u32, u32, u32, u32, u32) {
    (
        video.crt_enabled,
        video.crt_profile,
        video.crt_mask,
        video.crt_scanline_strength.to_bits(),
        video.crt_mask_strength.to_bits(),
        video.crt_bloom_strength.to_bits(),
        video.crt_curvature.to_bits(),
        video.crt_halation_strength.to_bits(),
        video.crt_diffusion_strength.to_bits(),
        video.crt_convergence.to_bits(),
    )
}

fn crt_settings_ui(ui: &mut egui::Ui, video: &mut VideoSettings) -> bool {
    let old = crt_signature(video);
    ui.checkbox(&mut video.crt_enabled, "CRT display (3× phosphor raster)");
    if video.crt_enabled {
        ui.indent("crt-controls", |ui| {
            egui::ComboBox::from_label("Profile")
                .selected_text(video.crt_profile.label())
                .show_ui(ui, |ui| {
                    for profile in [
                        CrtProfile::Flat,
                        CrtProfile::Royale,
                        CrtProfile::Lightweight,
                    ] {
                        ui.selectable_value(&mut video.crt_profile, profile, profile.label());
                    }
                });
            if matches!(video.crt_profile, CrtProfile::Royale | CrtProfile::Flat) {
                egui::ComboBox::from_label("Phosphor mask")
                    .selected_text(video.crt_mask.label())
                    .show_ui(ui, |ui| {
                        for mask in [
                            CrtMask::ApertureGrille,
                            CrtMask::SlotMask,
                            CrtMask::ShadowMask,
                        ] {
                            ui.selectable_value(&mut video.crt_mask, mask, mask.label());
                        }
                    });
            }
            ui.add(
                egui::Slider::new(&mut video.crt_scanline_strength, 0.0..=0.75)
                    .text("Scanlines"),
            );
            ui.add(
                egui::Slider::new(&mut video.crt_mask_strength, 0.0..=0.65)
                    .text("RGB phosphor mask"),
            );
            ui.add(
                egui::Slider::new(&mut video.crt_bloom_strength, 0.0..=0.75)
                    .text("Beam bloom"),
            );
            if video.crt_profile == CrtProfile::Royale {
                ui.add(
                    egui::Slider::new(&mut video.crt_curvature, 0.0..=0.16)
                        .text("Screen curvature"),
                );
            } else if video.crt_profile == CrtProfile::Flat {
                ui.small("No curvature, vignette, or curved black edges.");
            }
            if matches!(video.crt_profile, CrtProfile::Royale | CrtProfile::Flat) {
                ui.add(
                    egui::Slider::new(&mut video.crt_halation_strength, 0.0..=0.75)
                        .text("Faceplate halation"),
                );
                ui.add(
                    egui::Slider::new(&mut video.crt_diffusion_strength, 0.0..=0.75)
                        .text("Glass diffusion"),
                );
                ui.add(
                    egui::Slider::new(&mut video.crt_convergence, 0.0..=1.0)
                        .text("RGB convergence offset"),
                );
            }
            if ui.button("Royale-style PVM preset").clicked() {
                let defaults = VideoSettings::default();
                video.crt_profile = CrtProfile::Royale;
                video.crt_mask = CrtMask::ApertureGrille;
                video.crt_scanline_strength = defaults.crt_scanline_strength;
                video.crt_mask_strength = defaults.crt_mask_strength;
                video.crt_bloom_strength = defaults.crt_bloom_strength;
                video.crt_curvature = defaults.crt_curvature;
                video.crt_halation_strength = defaults.crt_halation_strength;
                video.crt_diffusion_strength = defaults.crt_diffusion_strength;
                video.crt_convergence = defaults.crt_convergence;
            }
            if ui.button("Flat CRT / PVM preset").clicked() {
                video.crt_profile = CrtProfile::Flat;
                video.crt_mask = CrtMask::ApertureGrille;
                video.crt_scanline_strength = 0.34;
                video.crt_mask_strength = 0.28;
                video.crt_bloom_strength = 0.18;
                video.crt_halation_strength = 0.14;
                video.crt_diffusion_strength = 0.08;
                video.crt_convergence = 0.06;
            }
            if ui.button("Royale-style consumer TV preset").clicked() {
                video.crt_profile = CrtProfile::Royale;
                video.crt_mask = CrtMask::SlotMask;
                video.crt_scanline_strength = 0.28;
                video.crt_mask_strength = 0.38;
                video.crt_bloom_strength = 0.32;
                video.crt_curvature = 0.09;
                video.crt_halation_strength = 0.30;
                video.crt_diffusion_strength = 0.20;
                video.crt_convergence = 0.20;
            }
            ui.small(
                "Flat CRT keeps the advanced tube graphics without screen geometry. Royale-style adds curved glass and vignette. Lightweight uses the faster original pass.",
            );
        });
    }
    old != crt_signature(video)
}

fn palette_preview(ui: &mut egui::Ui, palette: &OutputPalette) {
    const COLUMNS: usize = 16;
    const CELL: f32 = 10.0;
    let size = Vec2::new(COLUMNS as f32 * CELL, 4.0 * CELL);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    for (index, rgb) in palette.iter().enumerate() {
        let column = index % COLUMNS;
        let row = index / COLUMNS;
        let minimum = rect.min + Vec2::new(column as f32 * CELL, row as f32 * CELL);
        let color_rect = egui::Rect::from_min_size(minimum, Vec2::splat(CELL));
        ui.painter().rect_filled(
            color_rect,
            0.0,
            egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
        );
    }
}

fn set_controller_mask(nes: &mut Nes, port: usize, mask: u8) {
    if let Some(controller) = nes.controller_mut(port) {
        for (index, button) in [
            Button::A,
            Button::B,
            Button::Select,
            Button::Start,
            Button::Up,
            Button::Down,
            Button::Left,
            Button::Right,
        ]
        .into_iter()
        .enumerate()
        {
            controller.set_button(button, mask & (1 << index) != 0);
        }
    }
}

fn binding_mask(ctx: &egui::Context, bindings: &[KeyBinding; 8]) -> u8 {
    bindings
        .iter()
        .enumerate()
        .fold(0, |mask, (index, binding)| {
            mask | (u8::from(binding_down(ctx, binding)) << index)
        })
}

fn input_mask_editor(ui: &mut egui::Ui, mask: &mut u8, player: &str, frame: usize) {
    ui.push_id((player, frame), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong(player);
            for (bit, label) in ["A", "B", "Select", "Start", "Up", "Down", "Left", "Right"]
                .into_iter()
                .enumerate()
            {
                let pressed = *mask & (1 << bit) != 0;
                if ui.selectable_label(pressed, label).clicked() {
                    if pressed {
                        *mask &= !(1 << bit);
                    } else {
                        *mask |= 1 << bit;
                    }
                }
            }
            ui.monospace(format!("mask {:02X}", *mask));
        });
    });
}

fn input_mask_label(mask: u8) -> String {
    let pressed = [
        (0, "A"),
        (1, "B"),
        (2, "Select"),
        (3, "Start"),
        (4, "Up"),
        (5, "Down"),
        (6, "Left"),
        (7, "Right"),
    ]
    .into_iter()
    .filter_map(|(bit, label)| (mask & (1 << bit) != 0).then_some(label))
    .collect::<Vec<_>>();
    if pressed.is_empty() {
        "—".into()
    } else {
        pressed.join("+")
    }
}

fn binding_down(ctx: &egui::Context, binding: &KeyBinding) -> bool {
    if binding.label() == "Shift" {
        return ctx.input(|i| i.key_down(Key::ShiftLeft) || i.key_down(Key::ShiftRight));
    }
    Key::ALL
        .iter()
        .copied()
        .find(|key| key.name() == binding.label())
        .is_some_and(|key| ctx.input(|input| input.key_down(key)))
}

fn nes_button_label(index: usize) -> &'static str {
    ["A", "B", "Select", "Start", "Up", "Down", "Left", "Right"][index]
}

fn gamepad_binding_down(gamepad: &Gamepad<'_>, binding: GamepadBinding, threshold: f32) -> bool {
    match binding {
        GamepadBinding::Button(button) => {
            button != GamepadButton::Unknown && gamepad.is_pressed(button)
        }
        GamepadBinding::Axis { axis, direction } => {
            axis != Axis::Unknown && axis_active(gamepad.value(axis), direction, threshold)
        }
        GamepadBinding::ExactButton { code, .. } => {
            gamepad.state().is_pressed(code) || gamepad.state().value(code) >= threshold
        }
        GamepadBinding::ExactButtonLow { code, .. } => {
            raw_value_known(gamepad, code)
                && low_input_active(gamepad.state().value(code), threshold)
        }
        GamepadBinding::ExactAxis {
            code, direction, ..
        } => axis_active(gamepad.state().value(code), direction, threshold),
        GamepadBinding::ExactAxisLow { code, .. } => {
            raw_value_known(gamepad, code)
                && low_input_active(gamepad.state().value(code), threshold)
        }
        GamepadBinding::RawButton(code) => gamepad.state().is_pressed(code),
        GamepadBinding::RawAxis { code, direction } => {
            axis_active(gamepad.state().value(code), direction, threshold)
        }
    }
}

fn axis_active(value: f32, direction: i8, threshold: f32) -> bool {
    if direction < 0 {
        value <= -threshold
    } else {
        value >= threshold
    }
}

fn low_input_cutoff(threshold: f32) -> f32 {
    (1.0 - threshold.clamp(0.1, 0.9)) * 0.5
}

fn low_input_active(value: f32, threshold: f32) -> bool {
    value < low_input_cutoff(threshold)
}

fn raw_value_known(gamepad: &Gamepad<'_>, code: gilrs::ev::Code) -> bool {
    gamepad.state().button_data(code).is_some() || gamepad.state().axis_data(code).is_some()
}

fn gamepad_binding_label(binding: GamepadBinding) -> String {
    match binding {
        GamepadBinding::Button(button) => gamepad_button_label(button).into(),
        GamepadBinding::Axis { axis, direction } => {
            format!(
                "{} {}",
                gamepad_axis_label(axis),
                direction_label(direction)
            )
        }
        GamepadBinding::ExactButton { button, code } => {
            format!("{} [{code}]", gamepad_button_label(button))
        }
        GamepadBinding::ExactButtonLow { button, code } => {
            format!("{} low/inverted [{code}]", gamepad_button_label(button))
        }
        GamepadBinding::ExactAxis {
            axis,
            code,
            direction,
        } => format!(
            "{} {} [{code}]",
            gamepad_axis_label(axis),
            direction_label(direction)
        ),
        GamepadBinding::ExactAxisLow { axis, code } => {
            format!("{} low/inverted [{code}]", gamepad_axis_label(axis))
        }
        GamepadBinding::RawButton(code) => format!("Button {code}"),
        GamepadBinding::RawAxis { code, direction } => {
            format!("Axis {code} {}", direction_label(direction))
        }
    }
}

fn describe_gamepad_event(event: EventType) -> Option<String> {
    match event {
        EventType::ButtonPressed(button, code) => {
            Some(format!("{} pressed [{code}]", gamepad_button_label(button)))
        }
        EventType::ButtonReleased(button, code) => Some(format!(
            "{} released [{code}]",
            gamepad_button_label(button)
        )),
        EventType::ButtonChanged(button, value, code) => Some(format!(
            "{} value {value:.3} [{code}]",
            gamepad_button_label(button)
        )),
        EventType::AxisChanged(axis, value, code) => Some(format!(
            "{} value {value:.3} [{code}]",
            gamepad_axis_label(axis)
        )),
        EventType::Connected => Some("connected".into()),
        EventType::Disconnected => Some("disconnected".into()),
        _ => None,
    }
}

fn direction_label(direction: i8) -> &'static str {
    if direction < 0 { "−" } else { "+" }
}

fn gamepad_button_label(button: GamepadButton) -> &'static str {
    match button {
        GamepadButton::South => "South / A / Cross",
        GamepadButton::East => "East / B / Circle",
        GamepadButton::North => "North / Y / Triangle",
        GamepadButton::West => "West / X / Square",
        GamepadButton::C => "C",
        GamepadButton::Z => "Z",
        GamepadButton::LeftTrigger => "Left bumper",
        GamepadButton::LeftTrigger2 => "Left trigger",
        GamepadButton::RightTrigger => "Right bumper",
        GamepadButton::RightTrigger2 => "Right trigger",
        GamepadButton::Select => "Select / Back / Create",
        GamepadButton::Start => "Start / Options",
        GamepadButton::Mode => "Home / Guide",
        GamepadButton::LeftThumb => "Left stick click",
        GamepadButton::RightThumb => "Right stick click",
        GamepadButton::DPadUp => "D-pad Up",
        GamepadButton::DPadDown => "D-pad Down",
        GamepadButton::DPadLeft => "D-pad Left",
        GamepadButton::DPadRight => "D-pad Right",
        GamepadButton::Unknown => "Unknown button",
    }
}

fn gamepad_axis_label(axis: Axis) -> &'static str {
    match axis {
        Axis::LeftStickX => "Left stick X",
        Axis::LeftStickY => "Left stick Y",
        Axis::LeftZ => "Left trigger axis",
        Axis::RightStickX => "Right stick X",
        Axis::RightStickY => "Right stick Y",
        Axis::RightZ => "Right trigger axis",
        Axis::DPadX => "D-pad X",
        Axis::DPadY => "D-pad Y",
        Axis::Unknown => "Unknown axis",
    }
}

fn neutralize_opposite_directions(mut mask: u8) -> u8 {
    const UP_DOWN: u8 = (1 << 4) | (1 << 5);
    const LEFT_RIGHT: u8 = (1 << 6) | (1 << 7);
    if mask & UP_DOWN == UP_DOWN {
        mask &= !UP_DOWN;
    }
    if mask & LEFT_RIGHT == LEFT_RIGHT {
        mask &= !LEFT_RIGHT;
    }
    mask
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

fn advance_rewind_deadline(deadline: Instant, now: Instant, interval: Duration) -> Instant {
    let next = deadline + interval;
    if now.saturating_duration_since(next) >= interval {
        now + interval
    } else {
        next
    }
}

fn memory_label(space: MemorySpace) -> &'static str {
    match space {
        MemorySpace::CpuRam => "CPU RAM",
        MemorySpace::PpuNametable => "PPU nametables",
        MemorySpace::Palette => "Palette RAM",
        MemorySpace::Oam => "OAM",
        MemorySpace::PrgRom => "PRG ROM (read-only)",
        MemorySpace::Chr => "CHR",
    }
}
fn load_battery(nes: &mut Nes, rom_path: &Path) -> Result<(), Box<dyn Error>> {
    if nes.has_battery() {
        let path = persistence::battery_path(rom_path);
        if path.is_file() {
            nes.load_battery_ram(&fs::read(path)?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod input_filter_tests {
    use std::{
        collections::HashSet,
        path::Path,
        time::{Duration, Instant},
    };

    use super::{
        RewindCapture, RewindPoint, achievement_unlock_baseline, advance_rewind_deadline,
        inferred_region_from_rom_path, is_achievement_client_warning,
        is_archived_achievement_warning, low_input_active, neutralize_opposite_directions,
        register_achievement_unlock,
    };
    use crate::achievement_archive::UnlockEntry;
    use nes_achievements_native::{Achievement, AchievementBucket};
    use nes_core::Region;

    #[test]
    fn opposite_dpad_directions_cancel_without_touching_buttons() {
        let a_and_start = (1 << 0) | (1 << 3);
        assert_eq!(
            neutralize_opposite_directions(a_and_start | (1 << 6) | (1 << 7)),
            a_and_start
        );
        assert_eq!(
            neutralize_opposite_directions(a_and_start | (1 << 4) | (1 << 5)),
            a_and_start
        );
        assert_eq!(
            neutralize_opposite_directions(a_and_start | (1 << 4) | (1 << 7)),
            a_and_start | (1 << 4) | (1 << 7)
        );
    }

    #[test]
    fn zero_based_inverted_axes_treat_zero_as_active_and_center_as_neutral() {
        assert!(low_input_active(0.0, 0.5));
        assert!(!low_input_active(0.5, 0.5));
        assert!(!low_input_active(1.0, 0.5));
    }

    #[test]
    fn rewind_points_round_trip_through_fast_compression() {
        let machine = (0..256_u16)
            .flat_map(|byte| [byte as u8; 256])
            .collect::<Vec<_>>();
        let point = RewindPoint::compress(RewindCapture {
            machine: machine.clone(),
            generation: 9,
            tas_cursor: 12,
            lag_frames: 3,
            controller_reads: 4,
        });
        assert!(point.compressed_machine.len() < machine.len());
        assert_eq!(point.generation, 9);
        assert_eq!(point.decompress().unwrap(), machine);
    }

    #[test]
    fn rewind_deadline_carries_small_scheduler_delays_without_drifting() {
        let start = Instant::now();
        let interval = Duration::from_millis(10);
        let slightly_late = start + Duration::from_millis(12);
        assert_eq!(
            advance_rewind_deadline(start, slightly_late, interval),
            start + interval
        );

        let badly_late = start + Duration::from_millis(25);
        assert_eq!(
            advance_rewind_deadline(start, badly_late, interval),
            badly_late + interval
        );
    }

    #[test]
    fn unsupported_ra_entries_are_warnings_not_achievements() {
        let warning = Achievement {
            id: 123,
            points: 0,
            unlocked: true,
            bucket: AchievementBucket::Unsupported,
            measured_percent: 0.0,
            title: "Unsupported Game Version".into(),
            description: "This version has not been tested".into(),
            measured_progress: String::new(),
            badge_url: String::new(),
            badge_locked_url: String::new(),
        };
        assert!(is_achievement_client_warning(&warning));

        let archived = UnlockEntry {
            game_id: 1,
            achievement_id: 123,
            game_title: "Game".into(),
            title: warning.title,
            description: warning.description,
            points: 0,
            badge_url: String::new(),
            unlocked_at: 0,
        };
        assert!(is_archived_achievement_warning(&archived));
    }

    #[test]
    fn achievement_baseline_suppresses_completed_unlock_replays() {
        let achievement = |id, unlocked, bucket| Achievement {
            id,
            points: 5,
            unlocked,
            bucket,
            measured_percent: 0.0,
            title: format!("Achievement {id}"),
            description: String::new(),
            measured_progress: String::new(),
            badge_url: String::new(),
            badge_locked_url: String::new(),
        };
        let rows = [
            achievement(1, true, AchievementBucket::Unlocked),
            achievement(2, true, AchievementBucket::Unlocked),
            achievement(3, false, AchievementBucket::Unofficial),
            achievement(0, true, AchievementBucket::Unsupported),
        ];
        let (mut known, mastered) = achievement_unlock_baseline(&rows);
        assert_eq!(known, HashSet::from([1, 2]));
        assert!(mastered);
        assert_eq!(
            register_achievement_unlock(&mut known, 1, false),
            (false, false),
            "a reboot replay is suppressed by default"
        );
        assert_eq!(
            register_achievement_unlock(&mut known, 1, true),
            (false, true),
            "the saved preference can opt back into replay popups"
        );
        assert_eq!(
            register_achievement_unlock(&mut known, 4, false),
            (true, true),
            "a newly earned achievement always produces a notification"
        );
    }

    #[test]
    fn european_filename_corrects_a_missing_pal_header_flag() {
        assert_eq!(
            inferred_region_from_rom_path(Path::new(
                "25th Anniversary Super Mario Bros. (Europe) (Promo, Virtual Console).nes"
            )),
            Some(Region::Pal)
        );
        assert_eq!(
            inferred_region_from_rom_path(Path::new("Palace of Power (USA).nes")),
            None
        );
    }
}
