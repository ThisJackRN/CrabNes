use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use eframe::egui::{self, ColorImage, Key, TextureHandle, TextureOptions, Vec2};
use nes_core::{
    ApuChannel, Button, FRAME_HEIGHT, FRAME_WIDTH, MemorySpace, NTSC_2C02_PALETTE, NTSC_FRAME_RATE,
    Nes, OutputPalette, RGB_2C03_PALETTE,
};
use rfd::FileDialog;

use crate::{
    audio::AudioOutput,
    crt::{CrtParameters, CrtRenderer},
    library::{EntryStatus, LibraryEntry, RomLibrary},
    palettes, persistence, save_states, screenshot,
    settings::{
        self, CrtMask, CrtProfile, KeyBinding, PaletteMode, PerGameSettings, Settings,
        VideoSettings,
    },
    tas::{self, TasEditor, TasFrame, TasManager, TasMode, TasMovie, TasStartType},
    tas_control::{self, ControlMovie},
};

const SPEEDS: &[f64] = &[0.25, 0.5, 1.0, 1.5, 2.0, 3.0, 4.0];
const NORMAL_SPEED_INDEX: usize = 2;
// Permit a normal-speed frame to begin up to this fraction early. This keeps a
// 60 Hz presentation callback phase-locked to the 60.0988 Hz NES cadence instead
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

struct RewindPoint {
    machine: Vec<u8>,
    tas_cursor: usize,
    lag_frames: u64,
    controller_reads: u64,
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
    emulated_frames_in_window: u64,
    measured_fps: f64,
    last_rewind: Instant,
    last_held_frame_advance: Instant,
    frame_advance_hold_started: Option<Instant>,
    frame_advance_held: bool,
    frame_advance_repeated: bool,
    status: String,
    audio: Option<AudioOutput>,
    audio_error: Option<String>,
    audio_scratch: Vec<f32>,
    settings: Settings,
    per_game: PerGameSettings,
    settings_dirty: bool,
    settings_category: SettingsCategory,
    library: RomLibrary,
    library_search: String,
    library_sort: LibrarySort,
    library_cover_textures: HashMap<PathBuf, TextureHandle>,
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
            speed_index: settings.emulation.speed_index.min(SPEEDS.len() - 1),
            fast_forward: false,
            frame_budget: 0.0,
            last_tick: Instant::now(),
            fps_window_start: Instant::now(),
            emulated_frames_in_window: 0,
            measured_fps: 0.0,
            last_rewind: Instant::now(),
            last_held_frame_advance: Instant::now(),
            frame_advance_hold_started: None,
            frame_advance_held: false,
            frame_advance_repeated: false,
            status: "Choose a game from the library or open a ROM".into(),
            audio,
            audio_error,
            audio_scratch: Vec::with_capacity(1_024),
            settings,
            per_game: PerGameSettings::default(),
            settings_dirty: false,
            settings_category: SettingsCategory::General,
            library,
            library_search: String::new(),
            library_sort: LibrarySort::Title,
            library_cover_textures: HashMap::new(),
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
            lag_frames: 0,
            last_controller_reads: 0,
            hex_space: MemorySpace::CpuRam,
            hex_start: 0,
            hex_jump: String::new(),
            hex_selected: None,
            hex_value: String::new(),
        };
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
        self.handle_hotkeys(ctx);
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick).as_secs_f64().min(0.1);
        self.last_tick = now;
        let speed = self.current_speed();
        let held_frame_advance =
            self.frame_advance_held && ctx.input(|input| input.pointer.any_down()) && self.powered;
        if held_frame_advance {
            self.frame_budget = 0.0;
            let interval = Duration::from_secs_f64(1.0 / NTSC_FRAME_RATE);
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
            self.frame_budget += elapsed * NTSC_FRAME_RATE * speed;
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
        if ctx.input(|input| input.key_down(Key::Backspace))
            && self.last_rewind.elapsed() >= Duration::from_millis(35)
        {
            self.rewind_step();
            self.last_rewind = Instant::now();
        }
        let fps_elapsed = self.fps_window_start.elapsed();
        if fps_elapsed >= Duration::from_secs(2) {
            let sample = self.emulated_frames_in_window as f64 / fps_elapsed.as_secs_f64();
            self.measured_fps = if self.measured_fps == 0.0 {
                sample
            } else {
                self.measured_fps * 0.35 + sample * 0.65
            };
            self.emulated_frames_in_window = 0;
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
        let interval = self.settings.emulation.rewind_interval_frames.max(1);
        if let Some(nes) = &self.nes
            && nes.frame().number.is_multiple_of(interval)
            && let Ok(machine) = nes.save_state()
        {
            self.rewind.push_back(RewindPoint {
                machine,
                tas_cursor: self.tas.cursor,
                lag_frames: self.lag_frames,
                controller_reads: self.last_controller_reads,
            });
            let max = self.settings.emulation.rewind_seconds * 60 / interval as usize;
            while self.rewind.len() > max.max(1) {
                self.rewind.pop_front();
            }
        }
        if self.tas.mode != TasMode::Inactive
            && self
                .tas
                .cursor
                .is_multiple_of(self.tas.checkpoint_interval.max(1))
            && let Some(nes) = &self.nes
            && let Ok(state) = nes.save_state()
        {
            let frame = self.tas.cursor;
            let had_desync = self.tas.last_desync.is_some();
            self.tas.maybe_checkpoint(frame, state);
            if !had_desync && self.tas.last_desync.is_some() {
                if self.confirm_and_repair_stale_tas_checkpoint(frame) {
                    self.status = format!(
                        "Repaired stale TAS checkpoint at frame {frame}; save the movie to persist it"
                    );
                } else {
                    self.status = self.tas.last_desync.clone().unwrap_or_default();
                    self.tas.pause();
                    self.paused = true;
                    self.follow_tas_cursor();
                    return false;
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
        }
        if present_audio && let Some(audio) = &mut self.audio {
            audio.push(&self.audio_scratch);
        }
        self.frame_dirty = true;
        self.emulated_frames_in_window = self.emulated_frames_in_window.wrapping_add(1);
        if self.tas.mode != TasMode::Inactive {
            self.follow_tas_cursor();
        }
        true
    }

    fn handle_hotkeys(&mut self, ctx: &egui::Context) {
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
        if ctx.input(|i| i.key_pressed(Key::Space)) {
            self.toggle_pause();
        }
        if ctx.input(|i| i.key_pressed(Key::R)) {
            self.reset();
        }
        if ctx.input(|i| i.key_pressed(Key::P)) {
            if ctrl {
                self.power_cycle();
            } else {
                self.toggle_power();
            }
        }
        if ctx.input(|i| i.key_pressed(Key::F5)) {
            self.quick_save();
        }
        if ctx.input(|i| i.key_pressed(Key::F8)) {
            self.quick_load();
        }
        if ctx.input(|i| i.key_pressed(Key::N)) {
            self.advance_frame(ctx);
        }
        if ctx.input(|i| i.key_pressed(Key::F11)) {
            self.toggle_fullscreen(ctx);
        }
        if ctx.input(|i| i.key_pressed(Key::F12)) {
            self.take_screenshot();
        }
        if ctx.input(|i| i.key_pressed(Key::Num0)) {
            self.speed_index = NORMAL_SPEED_INDEX;
        }
        if ctx.input(|i| i.key_pressed(Key::F1)) {
            self.show_debugger = true;
        }
        if ctx.input(|i| i.key_pressed(Key::F2)) {
            self.show_hex = true;
            self.paused = true;
        }
        self.fast_forward = ctx.input(|i| i.key_down(Key::Tab));
        if shift && ctx.input(|i| i.key_pressed(Key::F5)) {
            self.show_states = true;
        }
    }

    fn host_input_frame(&self, ctx: &egui::Context) -> TasFrame {
        if ctx.egui_wants_keyboard_input() {
            return TasFrame::default();
        }
        self.bound_input_frame(ctx)
    }

    fn bound_input_frame(&self, ctx: &egui::Context) -> TasFrame {
        self.filter_live_dpad(TasFrame {
            player1: binding_mask(ctx, &self.settings.input.bindings),
            player2: binding_mask(ctx, &self.settings.input.player2_bindings),
        })
    }

    fn filter_live_dpad(&self, input: TasFrame) -> TasFrame {
        if self.settings.input.allow_opposite_directions {
            input
        } else {
            TasFrame {
                player1: neutralize_opposite_directions(input.player1),
                player2: neutralize_opposite_directions(input.player2),
            }
        }
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
                            egui::Button::new("Rewind One Frame    Backspace"),
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

                ui.separator();
                ui.selectable_value(&mut self.page, MainPage::Game, "Game");
                ui.selectable_value(&mut self.page, MainPage::Library, "Library");

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.settings.video.show_fps {
                        ui.label(format!("{:.1} FPS", self.measured_fps));
                        ui.separator();
                    }
                    let frame = self.nes.as_ref().map_or(0, |nes| nes.frame().number);
                    ui.label(format!("Frame {frame} · Lag {}", self.lag_frames));
                    ui.separator();
                    ui.label(if self.paused { "Paused" } else { "Running" });
                });
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
                let scale = (size.x / FRAME_WIDTH as f32)
                    .min(size.y / FRAME_HEIGHT as f32)
                    .floor()
                    .max(1.0);
                size = Vec2::new(FRAME_WIDTH as f32 * scale, FRAME_HEIGHT as f32 * scale);
            }
            ui.vertical_centered(|ui| {
                ui.add(egui::Image::new(&self.texture).fit_to_exact_size(size));
            });
        });
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
                    self.library.refresh(Some(&self.settings.paths.rom_folder));
                }
                if ui.button("Refresh").clicked() {
                    self.library.refresh(Some(&self.settings.paths.rom_folder));
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
                                ui.add_space(12.0);
                                ui.horizontal(|ui| {
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
                                        if ui.button("Change Cover Image…").clicked() {
                                            action = Some(LibraryAction::ChooseCover(
                                                entry.path.clone(),
                                            ));
                                            ui.close();
                                        }
                                        if entry.cover_image.is_some()
                                            && ui.button("Remove Cover Image").clicked()
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
                    .set_title("Choose game cover image")
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
                            self.status = "Library cover image updated".into();
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
                        self.status = "Library cover image removed".into();
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

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
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
        self.states_window(ui);
        self.time_window(ui);
        self.tas_window(ui);
        self.tas_control_window(ui);
        self.input_window(ui);
        self.av_window(ui);
        self.debugger_window(ui);
        self.hex_window(ui);
    }

    fn settings_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_settings {
            return;
        }
        let mut open = self.show_settings;
        let mut close_requested = false;
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
            .constrain(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Drag this window beyond the game area if needed.");
                    if ui.button("Close Settings").clicked() {
                        close_requested = true;
                    }
                });
                ui.separator();
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
                        ui.selectable_value(&mut self.settings_category, cat, label);
                    }
                });
                ui.separator();
                let mut changed = false;
                match self.settings_category {
                    SettingsCategory::General => {
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
                        changed |= input_mapping_ui(ui, &mut self.settings);
                        if ui.button("Restore Input defaults").clicked() {
                            self.settings.input = Default::default();
                            changed = true;
                        }
                    }
                    SettingsCategory::Emulation => {
                        changed |= speed_ui(ui, &mut self.settings.emulation.speed_index);
                        changed |= ui
                            .add(
                                egui::Slider::new(
                                    &mut self.settings.emulation.rewind_seconds,
                                    1..=30,
                                )
                                .text("Rewind seconds"),
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
                            self.library.refresh(Some(&self.settings.paths.rom_folder));
                            changed = true;
                        }
                        ui.label(format!("States: {}", settings::state_root().display()));
                        ui.label(format!("TAS: {}", settings::tas_root().display()));
                        if ui.button("Restore Paths defaults").clicked() {
                            self.settings.paths = Default::default();
                            self.library.refresh(Some(&self.settings.paths.rom_folder));
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
        if self.settings.emulation.speed_index != old_default_speed {
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
                    ui.add(egui::Image::new(texture).fit_to_exact_size(Vec2::new(256.0, 240.0)));
                }
                ui.horizontal(|ui| {
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
        let mut open = self.show_time;
        egui::Window::new("Rewind & Speed")
            .open(&mut open)
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
                    self.rewind_once();
                }
                ui.label(format!(
                    "{} snapshots buffered (~{} seconds)",
                    self.rewind.len(),
                    self.settings.emulation.rewind_seconds
                ));
                ui.label("Hold Backspace to rewind.");
            });
        self.show_time = open;
    }

    fn held_frame_advance_button(&mut self, ui: &mut egui::Ui, label: &str) {
        let response = ui
            .add_enabled(self.powered, egui::Button::new(label))
            .on_hover_text("Click for one frame, or hold for continuous NTSC-rate frame advance");
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
            .default_width(940.0)
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
                        ui.horizontal(|ui| {
                            ui.label("Author");
                            ui.text_edit_singleline(movie.author.get_or_insert_with(String::new));
                            ui.label("Description");
                            ui.text_edit_singleline(
                                movie.description.get_or_insert_with(String::new),
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
                                ui.horizontal(|ui| {
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
                        let mut timeline = egui::ScrollArea::vertical()
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
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.tas.marker_label);
                        add_marker = ui
                            .add_enabled(!read_only, egui::Button::new("Add at selected"))
                            .clicked();
                    });
                    for marker in movie.markers.clone() {
                        ui.horizontal(|ui| {
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
            .default_width(900.0)
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
                            "Current state",
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
                        ui.horizontal(|ui| {
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
                        let mut inputs = egui::ScrollArea::vertical()
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
        let Some(source) = self.tas_control_movie.clone() else {
            return;
        };
        if self.nes.is_none() {
            self.tas_control_status = "Load the matching NES ROM before converting".into();
            return;
        }
        let start_type = self.tas_control_start;
        self.new_tas_movie(start_type);
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
            "Converted {frame_count} {} frames into the native TAS editor",
            source.format
        );
        self.tas_control_status = self.status.clone();
    }

    fn input_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_input {
            return;
        }
        let mut open = self.show_input;
        egui::Window::new("Input Configuration")
            .open(&mut open)
            .show(ui, |ui| {
                if input_mapping_ui(ui, &mut self.settings) {
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
                ui.horizontal(|ui| {
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
            .resizable(true)
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
                ui.horizontal(|ui| {
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
                egui::ScrollArea::vertical()
                    .max_height(380.0)
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
                    ui.horizontal(|ui| {
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
    /// If it produces the live state byte-for-byte, emulation is deterministic
    /// and only the checksum stored in the edited movie is stale.
    fn confirm_and_repair_stale_tas_checkpoint(&mut self, frame: usize) -> bool {
        let Some(current) = self
            .tas
            .checkpoints
            .iter()
            .find(|point| point.frame == frame)
            .cloned()
        else {
            return false;
        };
        let Some(previous) = self
            .tas
            .checkpoints
            .iter()
            .rev()
            .find(|point| point.frame < frame)
            .cloned()
        else {
            return false;
        };
        let Some(inputs) = self
            .tas
            .movie
            .as_ref()
            .and_then(|movie| movie.frames.get(previous.frame..frame))
            .map(|inputs| inputs.to_vec())
        else {
            return false;
        };
        let Ok(mut verifier) = Nes::from_ines(&self.rom_bytes) else {
            return false;
        };
        if verifier.load_state(&previous.state).is_err() {
            return false;
        }
        let mut audio = Vec::new();
        for input in inputs {
            set_controller_mask(&mut verifier, 0, input.player1);
            set_controller_mask(&mut verifier, 1, input.player2);
            if verifier.run_frame().is_err() {
                return false;
            }
            audio.clear();
            verifier.drain_audio_samples(&mut audio);
        }
        let Ok(verified) = verifier.save_state() else {
            return false;
        };
        if verified != current.state {
            return false;
        }
        self.tas.repair_checkpoint_checksum(frame, &current.state);
        true
    }

    fn toggle_pause(&mut self) {
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
        if let Some(nes) = &mut self.nes {
            nes.reset();
            self.powered = true;
            self.paused = false;
            self.rewind.clear();
            self.tas.stop();
            self.lag_frames = 0;
            self.frame_dirty = true;
            self.clear_audio_pipeline();
            self.status = "Reset".into();
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
    fn rewind_step(&mut self) {
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
    }

    fn rewind_once(&mut self) {
        if let (Some(point), Some(nes)) = (self.rewind.pop_back(), self.nes.as_mut())
            && nes.load_state(&point.machine).is_ok()
        {
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
            if self.tas.movie.is_some() {
                self.follow_tas_cursor();
            }
            self.clear_audio_pipeline();
            self.status = if recording_rewind {
                format!(
                    "Rewound to TAS frame {}; removed {removed} future input frame(s)",
                    point.tas_cursor
                )
            } else {
                "Rewound".into()
            };
        }
    }
    fn quick_save(&mut self) {
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
        if let Some(nes) = &mut self.nes {
            match save_states::load_slot(nes, self.selected_slot) {
                Ok(_) => {
                    self.paused = true;
                    self.powered = nes.powered();
                    self.frame_dirty = true;
                    self.rewind.clear();
                    self.tas.stop();
                    self.lag_frames = 0;
                    self.last_controller_reads = nes.controller_reads(0);
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
        let mut replacement = Nes::from_ines(&bytes)?;
        load_battery(&mut replacement, &path)?;
        let hash = replacement.rom_hash();
        self.per_game = settings::load_per_game(hash);
        self.speed_index = self
            .per_game
            .speed_index
            .unwrap_or(self.settings.emulation.speed_index)
            .min(SPEEDS.len() - 1);
        self.nes = Some(replacement);
        let palette_note = self.apply_video_palette().err();
        self.rom_path = Some(path.clone());
        self.rom_bytes = bytes;
        self.powered = true;
        self.paused = false;
        self.frame_budget = 0.0;
        self.frame_dirty = true;
        self.rewind.clear();
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
        self.library.refresh(Some(&self.settings.paths.rom_folder));
        self.clear_audio_pipeline();
        self.page = MainPage::Game;
        self.status = palette_note
            .map(|note| format!("ROM loaded; {note}"))
            .unwrap_or_else(|| "ROM loaded".into());
        Ok(())
    }

    fn apply_video_palette(&mut self) -> Result<Option<String>, String> {
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
        if self.nes.is_none() || self.rom_bytes.is_empty() {
            self.status = "Load a ROM before creating a TAS movie".into();
            return;
        }
        if start_type == TasStartType::PowerOn {
            match Nes::from_ines(&self.rom_bytes) {
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
        self.rewind.clear();
        self.lag_frames = 0;
        self.last_controller_reads = nes
            .controller_reads(0)
            .wrapping_add(nes.controller_reads(1));
        self.frame_dirty = true;
        self.follow_tas_cursor();
        self.clear_audio_pipeline();
        self.status = format!("Recording new {start_type:?} TAS at 1x");
    }

    fn restore_tas_start(&mut self) -> Result<Vec<u8>, String> {
        let (start_type, starting_state) = self
            .tas
            .movie
            .as_ref()
            .map(|movie| (movie.start_type, movie.starting_state.clone()))
            .ok_or_else(|| "no TAS movie loaded".to_owned())?;
        match start_type {
            TasStartType::PowerOn => {
                self.nes = Some(Nes::from_ines(&self.rom_bytes).map_err(|e| e.to_string())?);
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
                    self.tas.maybe_checkpoint(0, initial_state);
                    self.lag_frames = 0;
                    if let Some(nes) = &self.nes {
                        self.last_controller_reads = nes
                            .controller_reads(0)
                            .wrapping_add(nes.controller_reads(1));
                    }
                    self.paused = false;
                    self.powered = true;
                    self.rewind.clear();
                    self.frame_dirty = true;
                    self.follow_tas_cursor();
                    self.clear_audio_pipeline();
                    self.status = if read_only {
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
        let Some(nes) = &mut self.nes else {
            return false;
        };
        if let Err(error) = nes.load_state(&checkpoint.state) {
            self.status = format!("Checkpoint load failed: {error}");
            return false;
        }
        for (offset, input) in frames.into_iter().enumerate() {
            set_controller_mask(nes, 0, input.player1);
            set_controller_mask(nes, 1, input.player2);
            if let Err(error) = nes.run_frame() {
                self.status = format!("Seek stopped: {error}");
                return false;
            }
            self.audio_scratch.clear();
            nes.drain_audio_samples(&mut self.audio_scratch);
            let next_frame = checkpoint.frame + offset + 1;
            if next_frame % self.tas.checkpoint_interval.max(1) == 0
                && let Ok(state) = nes.save_state()
            {
                self.tas.maybe_checkpoint(next_frame, state);
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
        if let (Some(nes), Some(movie)) = (&self.nes, &self.tas.movie) {
            let dir = settings::tas_root().join(format!("{:016x}", nes.rom_hash()));
            let _ = fs::create_dir_all(&dir);
            if let Some(path) = FileDialog::new()
                .set_directory(dir)
                .add_filter("My Own NES TAS", &["tas"])
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
        if let Some(nes) = &self.nes
            && let Some(path) = FileDialog::new()
                .set_directory(settings::tas_root().join(format!("{:016x}", nes.rom_hash())))
                .add_filter("My Own NES TAS", &["tas"])
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
    fn current_speed(&self) -> f64 {
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
        if !ui.ctx().input(|input| input.pointer.any_down()) {
            self.frame_advance_repeated = false;
            self.frame_advance_hold_started = None;
        }
    }
    fn on_exit(&mut self) {
        if self.settings.save_states.autosave_on_exit {
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
    ui.horizontal(|ui| {
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
            mask | (u8::from(binding_down(ctx, *binding)) << index)
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

fn binding_down(ctx: &egui::Context, binding: KeyBinding) -> bool {
    match binding {
        KeyBinding::Shift => {
            ctx.input(|i| i.key_down(Key::ShiftLeft) || i.key_down(Key::ShiftRight))
        }
        _ => ctx.input(|i| {
            i.key_down(match binding {
                KeyBinding::Z => Key::Z,
                KeyBinding::X => Key::X,
                KeyBinding::Enter => Key::Enter,
                KeyBinding::Up => Key::ArrowUp,
                KeyBinding::Down => Key::ArrowDown,
                KeyBinding::Left => Key::ArrowLeft,
                KeyBinding::Right => Key::ArrowRight,
                KeyBinding::A => Key::A,
                KeyBinding::S => Key::S,
                KeyBinding::Q => Key::Q,
                KeyBinding::W => Key::W,
                KeyBinding::C => Key::C,
                KeyBinding::V => Key::V,
                KeyBinding::E => Key::E,
                KeyBinding::I => Key::I,
                KeyBinding::J => Key::J,
                KeyBinding::K => Key::K,
                KeyBinding::L => Key::L,
                KeyBinding::Shift => unreachable!(),
            })
        }),
    }
}
fn input_mapping_ui(ui: &mut egui::Ui, settings: &mut Settings) -> bool {
    let mut changed = false;
    changed |= ui
        .checkbox(
            &mut settings.input.allow_opposite_directions,
            "Allow opposite D-pad directions",
        )
        .on_hover_text(
            "Off matches a stock NES rocker D-pad: Left+Right and Up+Down cancel to neutral. Enable only for TAS/debug input that intentionally needs impossible combinations.",
        )
        .changed();
    if !settings.input.allow_opposite_directions {
        ui.small("Hardware-accurate D-pad: opposite directions cancel to neutral.");
    }
    egui::Grid::new("input-map").striped(true).show(ui, |ui| {
        ui.strong("Button");
        ui.strong("Player 1");
        ui.strong("Player 2");
        ui.end_row();
        for (index, label) in ["A", "B", "Select", "Start", "Up", "Down", "Left", "Right"]
            .into_iter()
            .enumerate()
        {
            ui.label(label);
            egui::ComboBox::from_id_salt(("binding", index))
                .selected_text(settings.input.bindings[index].label())
                .show_ui(ui, |ui| {
                    for key in KeyBinding::ALL {
                        changed |= ui
                            .selectable_value(&mut settings.input.bindings[index], key, key.label())
                            .changed();
                    }
                });
            egui::ComboBox::from_id_salt(("binding-p2", index))
                .selected_text(settings.input.player2_bindings[index].label())
                .show_ui(ui, |ui| {
                    for key in KeyBinding::ALL {
                        changed |= ui
                            .selectable_value(
                                &mut settings.input.player2_bindings[index],
                                key,
                                key.label(),
                            )
                            .changed();
                    }
                });
            ui.end_row();
        }
    });
    changed
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
    use super::neutralize_opposite_directions;

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
}
