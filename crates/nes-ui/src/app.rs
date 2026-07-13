use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use eframe::egui::{self, ColorImage, Key, TextureHandle, TextureOptions, Vec2};
use nes_core::{Button, FRAME_HEIGHT, FRAME_WIDTH, NTSC_FRAME_RATE, Nes};
use rfd::FileDialog;

use crate::{audio::AudioOutput, persistence, screenshot};

const SPEEDS: &[f64] = &[0.25, 0.5, 1.0, 1.5, 2.0, 3.0, 4.0];
const NORMAL_SPEED_INDEX: usize = 2;

pub struct App {
    nes: Nes,
    rom_path: PathBuf,
    rom_bytes: Vec<u8>,
    texture: TextureHandle,
    frame_dirty: bool,
    paused: bool,
    powered: bool,
    fullscreen: bool,
    speed_index: usize,
    fast_forward: bool,
    frame_budget: f64,
    last_tick: Instant,
    recent_roms: Vec<PathBuf>,
    status: String,
    audio: Option<AudioOutput>,
    audio_error: Option<String>,
    audio_scratch: Vec<f32>,
    volume: f32,
    muted: bool,
    reference_mastering: bool,
    show_game: bool,
    show_states: bool,
    show_time: bool,
    show_tas: bool,
    show_input: bool,
    show_av: bool,
    show_library: bool,
    show_debugger: bool,
    selected_slot: usize,
}

impl App {
    pub fn new(path: PathBuf, cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        let rom_bytes = fs::read(&path).map_err(|error| error.to_string())?;
        let mut nes = Nes::from_ines(&rom_bytes).map_err(|error| error.to_string())?;
        load_battery(&mut nes, &path).map_err(|error| error.to_string())?;
        let mut recent_roms = persistence::load_recent_roms();
        persistence::remember_rom(&mut recent_roms, &path).map_err(|error| error.to_string())?;
        let image = ColorImage::from_rgb([FRAME_WIDTH, FRAME_HEIGHT], &nes.frame().pixels);
        let texture = cc
            .egui_ctx
            .load_texture("nes-frame", image, TextureOptions::NEAREST);
        let (audio, audio_error) = match AudioOutput::new(nes.audio_sample_rate()) {
            Ok(audio) => (Some(audio), None),
            Err(error) => (None, Some(error.to_string())),
        };

        Ok(Self {
            nes,
            rom_path: path,
            rom_bytes,
            texture,
            frame_dirty: false,
            paused: false,
            powered: true,
            fullscreen: false,
            speed_index: NORMAL_SPEED_INDEX,
            fast_forward: false,
            frame_budget: 0.0,
            last_tick: Instant::now(),
            recent_roms,
            status: "Running".into(),
            audio,
            audio_error,
            audio_scratch: Vec::with_capacity(1_024),
            volume: 0.75,
            muted: false,
            reference_mastering: true,
            show_game: false,
            show_states: false,
            show_time: false,
            show_tas: false,
            show_input: false,
            show_av: false,
            show_library: false,
            show_debugger: false,
            selected_slot: 0,
        })
    }

    fn emulate(&mut self, ctx: &egui::Context) {
        self.handle_hotkeys(ctx);
        self.update_controller(ctx);
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick).as_secs_f64().min(0.1);
        self.last_tick = now;
        let speed = if self.fast_forward {
            4.0
        } else {
            SPEEDS[self.speed_index]
        };

        if !self.paused && self.powered {
            self.frame_budget += elapsed * NTSC_FRAME_RATE * speed;
            let mut frames_run = 0;
            while self.frame_budget >= 1.0 && frames_run < 8 {
                if let Err(error) = self.nes.run_frame() {
                    self.paused = true;
                    self.status = format!("Emulation stopped: {error}");
                    break;
                }
                self.frame_budget -= 1.0;
                frames_run += 1;
                self.frame_dirty = true;
                self.audio_scratch.clear();
                self.nes.drain_audio_samples(&mut self.audio_scratch);
                if speed == 1.0
                    && let Some(audio) = &mut self.audio
                {
                    audio.push(&self.audio_scratch);
                }
            }
            if frames_run == 8 {
                self.frame_budget = self.frame_budget.min(1.0);
            }
        }

        if let Some(audio) = &mut self.audio {
            audio.set_volume(self.volume);
            audio.set_muted(self.muted || self.paused || !self.powered || speed != 1.0);
            audio.set_reference_mastering(self.reference_mastering);
            if speed != 1.0 {
                audio.clear();
            }
        }
        ctx.request_repaint_after(Duration::from_millis(2));
    }

    fn handle_hotkeys(&mut self, ctx: &egui::Context) {
        let commands = ctx.input(|input| {
            (
                input.key_pressed(Key::Space),
                input.key_pressed(Key::N),
                input.key_pressed(Key::R),
                input.modifiers.ctrl,
                input.key_pressed(Key::P),
                input.key_pressed(Key::Plus) || input.key_pressed(Key::Equals),
                input.key_pressed(Key::Minus),
                input.key_pressed(Key::Num0),
                input.key_pressed(Key::F11),
                input.key_pressed(Key::F12),
                input.key_pressed(Key::O),
                input.key_down(Key::Tab),
                input.key_pressed(Key::Escape),
            )
        });
        let (
            pause,
            advance,
            reset,
            ctrl,
            power,
            faster,
            slower,
            normal,
            fullscreen,
            shot,
            open,
            fast,
            exit,
        ) = commands;
        self.fast_forward = fast;
        if exit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if pause {
            self.toggle_pause();
        }
        if advance && self.paused {
            self.advance_frame();
        }
        if reset {
            if ctrl {
                self.restart();
            } else {
                self.reset();
            }
        }
        if power {
            self.toggle_power();
        }
        if faster {
            self.speed_index = (self.speed_index + 1).min(SPEEDS.len() - 1);
        }
        if slower {
            self.speed_index = self.speed_index.saturating_sub(1);
        }
        if normal {
            self.speed_index = NORMAL_SPEED_INDEX;
        }
        if fullscreen {
            self.toggle_fullscreen(ctx);
        }
        if shot {
            self.take_screenshot();
        }
        if open && ctrl {
            self.open_rom_dialog();
        }

        if ctrl {
            let keys = [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
            ];
            let selected = ctx.input(|input| keys.iter().position(|key| input.key_pressed(*key)));
            if let Some(index) = selected
                && let Some(path) = self.recent_roms.get(index).cloned()
            {
                self.try_load_rom(path);
            }
        }
    }

    fn update_controller(&mut self, ctx: &egui::Context) {
        let buttons = ctx.input(|input| {
            [
                input.key_down(Key::Z),
                input.key_down(Key::X),
                input.key_down(Key::ShiftLeft) || input.key_down(Key::ShiftRight),
                input.key_down(Key::Enter),
                input.key_down(Key::ArrowUp),
                input.key_down(Key::ArrowDown),
                input.key_down(Key::ArrowLeft),
                input.key_down(Key::ArrowRight),
            ]
        });
        if let Some(controller) = self.nes.controller_mut(0) {
            for (button, pressed) in [
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
            .zip(buttons)
            {
                controller.set_button(button, pressed);
            }
        }
    }

    fn top_tabs(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("feature-tabs").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("NES");
                tab(ui, "Game", &mut self.show_game);
                tab(ui, "Save States", &mut self.show_states);
                tab(ui, "Rewind & Speed", &mut self.show_time);
                tab(ui, "TAS", &mut self.show_tas);
                tab(ui, "Input", &mut self.show_input);
                tab(ui, "Audio / Video", &mut self.show_av);
                tab(ui, "Library", &mut self.show_library);
                tab(ui, "Debugger", &mut self.show_debugger);
                ui.separator();
                ui.label(format!("Frame {}", self.nes.frame().number));
                ui.label(if self.paused { "Paused" } else { "Running" });
            });
        });
    }

    fn game_view(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            if self.frame_dirty {
                self.texture.set(
                    ColorImage::from_rgb([FRAME_WIDTH, FRAME_HEIGHT], &self.nes.frame().pixels),
                    TextureOptions::NEAREST,
                );
                self.frame_dirty = false;
            }
            let available = ui.available_size();
            let aspect = FRAME_WIDTH as f32 / FRAME_HEIGHT as f32;
            let mut size = Vec2::new(available.x, available.x / aspect);
            if size.y > available.y {
                size = Vec2::new(available.y * aspect, available.y);
            }
            size.x = size.x.max(FRAME_WIDTH as f32);
            size.y = size.y.max(FRAME_HEIGHT as f32);
            ui.vertical_centered(|ui| {
                ui.add(egui::Image::new(&self.texture).fit_to_exact_size(size));
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                ui.separator();
                ui.label(format!(
                    "{}x",
                    if self.fast_forward {
                        4.0
                    } else {
                        SPEEDS[self.speed_index]
                    }
                ));
                ui.separator();
                ui.label(if self.audio.is_some() && !self.muted {
                    "Audio on"
                } else {
                    "Audio off"
                });
                ui.separator();
                ui.label(
                    self.rom_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("ROM"),
                );
            });
        });
    }

    fn feature_windows(&mut self, ui: &mut egui::Ui) {
        self.game_window(ui);
        self.states_window(ui);
        self.time_window(ui);
        self.tas_window(ui);
        self.input_window(ui);
        self.av_window(ui);
        self.library_window(ui);
        self.debugger_window(ui);
    }

    fn game_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_game {
            return;
        }
        let mut open = true;
        egui::Window::new("Game Controls")
            .open(&mut open)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Open ROM…").clicked() {
                        self.open_rom_dialog();
                    }
                    if ui
                        .button(if self.paused { "Resume" } else { "Pause" })
                        .clicked()
                    {
                        self.toggle_pause();
                    }
                    if ui.button("Frame advance").clicked() {
                        self.advance_frame();
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Reset").clicked() {
                        self.reset();
                    }
                    if ui.button("Restart").clicked() {
                        self.restart();
                    }
                    if ui
                        .button(if self.powered {
                            "Power off"
                        } else {
                            "Power on"
                        })
                        .clicked()
                    {
                        self.toggle_power();
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Fullscreen").clicked() {
                        self.toggle_fullscreen(ui.ctx());
                    }
                    if ui.button("Screenshot").clicked() {
                        self.take_screenshot();
                    }
                });
            });
        self.show_game = open;
    }

    fn states_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_states {
            return;
        }
        let mut open = true;
        egui::Window::new("Save States").open(&mut open).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for slot in 0..10 {
                    ui.selectable_value(&mut self.selected_slot, slot, format!("Slot {slot}"));
                }
            });
            ui.horizontal(|ui| {
                ui.add_enabled(false, egui::Button::new("Save state"));
                ui.add_enabled(false, egui::Button::new("Load state"));
            });
            ui.label("State serialization is the next core milestone; slots are reserved here so the UI will not be redesigned later.");
        });
        self.show_states = open;
    }

    fn time_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_time {
            return;
        }
        let mut open = true;
        egui::Window::new("Rewind & Speed")
            .open(&mut open)
            .show(ui, |ui| {
                ui.label("Emulation speed");
                ui.horizontal_wrapped(|ui| {
                    for (index, speed) in SPEEDS.iter().enumerate() {
                        ui.selectable_value(&mut self.speed_index, index, format!("{speed}x"));
                    }
                });
                ui.checkbox(&mut self.fast_forward, "Fast-forward (4x, audio muted)");
                if ui.button("Advance one frame").clicked() {
                    self.advance_frame();
                }
                ui.separator();
                ui.add_enabled(false, egui::Button::new("Hold to rewind"));
                ui.label("Rewind awaits full-machine snapshots.");
            });
        self.show_time = open;
    }

    fn tas_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_tas {
            return;
        }
        let mut open = true;
        egui::Window::new("TAS Tools").open(&mut open).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_enabled(false, egui::Button::new("Record"));
                ui.add_enabled(false, egui::Button::new("Play"));
                ui.add_enabled(false, egui::Button::new("Rerecord"));
            });
            ui.label("Movie timeline, input editing, and rerecord count will use the same deterministic snapshots as rewind.");
        });
        self.show_tas = open;
    }

    fn input_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_input {
            return;
        }
        let mut open = true;
        egui::Window::new("Controller Input").open(&mut open).show(ui, |ui| {
            egui::Grid::new("input-map").striped(true).show(ui, |ui| {
                for (button, key) in [("D-pad", "Arrow keys"), ("A", "Z"), ("B", "X"), ("Start", "Enter"), ("Select", "Shift")] {
                    ui.label(button); ui.label(key); ui.end_row();
                }
            });
            ui.label("Clickable remapping and gamepad discovery are planned; keyboard input is live now.");
        });
        self.show_input = open;
    }

    fn av_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_av {
            return;
        }
        let mut open = true;
        egui::Window::new("Audio / Video")
            .open(&mut open)
            .show(ui, |ui| {
                ui.checkbox(&mut self.muted, "Mute");
                ui.add(egui::Slider::new(&mut self.volume, 0.0..=1.0).text("Volume"));
                ui.checkbox(
                    &mut self.reference_mastering,
                    "Reference-style output mastering",
                );
                if let Some(audio) = &self.audio {
                    let apu = self.nes.apu_state();
                    ui.label(format!("Device: {}", audio.device_name()));
                    ui.label(format!("Device rate: {} Hz", audio.device_sample_rate()));
                    ui.label(format!("Buffered samples: {}", audio.queued_samples()));
                    ui.label(format!(
                        "Underruns: {}  Overflows: {}",
                        audio.underflows(),
                        audio.overflows()
                    ));
                    ui.separator();
                    ui.monospace(format!(
                        "Pulse 1 {:7.2} Hz  level {:2}   Pulse 2 {:7.2} Hz  level {:2}",
                        apu.pulse_frequencies_hz[0],
                        apu.pulse_levels[0],
                        apu.pulse_frequencies_hz[1],
                        apu.pulse_levels[1]
                    ));
                    ui.monospace(format!(
                        "Triangle {:7.2} Hz  level {:2}   Noise period {:4}  level {:2}",
                        apu.triangle_frequency_hz,
                        apu.triangle_level,
                        apu.noise_period,
                        apu.noise_level
                    ));
                    ui.monospace(format!(
                        "DMC period {:3}  DAC {:3}   Frame sequencer: {}-step",
                        apu.dmc_period,
                        apu.dmc_level,
                        if apu.frame_five_step { 5 } else { 4 }
                    ));
                } else {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        self.audio_error.as_deref().unwrap_or("No audio device"),
                    );
                }
                ui.separator();
                if ui.button("Toggle fullscreen").clicked() {
                    self.toggle_fullscreen(ui.ctx());
                }
                ui.label("256×240, nearest-neighbor presentation, aspect ratio preserved.");
            });
        self.show_av = open;
    }

    fn library_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_library {
            return;
        }
        let mut open = true;
        let mut selected = None;
        egui::Window::new("ROM Library")
            .open(&mut open)
            .show(ui, |ui| {
                if ui.button("Browse for ROM…").clicked() {
                    self.open_rom_dialog();
                }
                ui.separator();
                ui.label("Recent ROMs");
                for (index, path) in self.recent_roms.iter().enumerate() {
                    if ui
                        .button(format!("{}. {}", index + 1, path.display()))
                        .clicked()
                    {
                        selected = Some(path.clone());
                    }
                }
            });
        if let Some(path) = selected {
            self.try_load_rom(path);
        }
        self.show_library = open;
    }

    fn debugger_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_debugger {
            return;
        }
        let mut open = true;
        let cpu = self.nes.cpu_state();
        let ppu = self.nes.ppu_state();
        egui::Window::new("Debugger").open(&mut open).show(ui, |ui| {
            ui.monospace(format!("PC {:04X}  A {:02X}  X {:02X}  Y {:02X}", cpu.program_counter, cpu.a, cpu.x, cpu.y));
            ui.monospace(format!("SP {:02X}  P {:02X}  Instructions {}", cpu.stack_pointer, cpu.status, cpu.instructions));
            ui.separator();
            ui.monospace(format!("PPU scanline {} dot {}  v {:04X} t {:04X} x {}", ppu.scanline, ppu.dot, ppu.vram_address, ppu.temp_address, ppu.fine_x));
            ui.horizontal(|ui| {
                if ui.button(if self.paused { "Resume" } else { "Pause" }).clicked() { self.toggle_pause(); }
                if ui.button("Frame step").clicked() { self.advance_frame(); }
            });
            ui.label("Memory view, breakpoints, disassembly, and instruction stepping require a side-effect-free debug bus, which is reserved for the debugger milestone.");
        });
        self.show_debugger = open;
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        self.frame_budget = 0.0;
        self.status = if self.paused { "Paused" } else { "Running" }.into();
    }
    fn advance_frame(&mut self) {
        if self.powered {
            self.paused = true;
            match self.nes.run_frame() {
                Ok(_) => {
                    self.frame_dirty = true;
                    self.status = "Frame advanced".into();
                }
                Err(error) => self.status = format!("Frame advance failed: {error}"),
            }
        }
    }
    fn reset(&mut self) {
        self.nes.reset();
        self.powered = true;
        self.paused = false;
        self.frame_budget = 0.0;
        if let Some(audio) = &self.audio {
            audio.clear();
        }
        self.status = "Reset".into();
    }
    fn restart(&mut self) {
        if let Err(error) = self.restart_inner() {
            self.status = format!("Restart failed: {error}");
        }
    }
    fn restart_inner(&mut self) -> Result<(), Box<dyn Error>> {
        self.save_battery()?;
        self.nes = Nes::from_ines(&self.rom_bytes)?;
        load_battery(&mut self.nes, &self.rom_path)?;
        self.powered = true;
        self.paused = false;
        self.frame_budget = 0.0;
        self.frame_dirty = true;
        if let Some(audio) = &self.audio {
            audio.clear();
        }
        self.status = "Restarted".into();
        Ok(())
    }
    fn toggle_power(&mut self) {
        if self.powered {
            self.nes.power_off();
            self.powered = false;
            self.paused = true;
            self.status = "Powered off".into();
        } else {
            self.nes.power_on();
            self.powered = true;
            self.paused = false;
            self.status = "Powered on".into();
        }
    }
    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.fullscreen = !self.fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
    }
    fn take_screenshot(&mut self) {
        match screenshot::save(self.nes.frame(), &self.rom_path) {
            Ok(path) => self.status = format!("Screenshot: {}", path.display()),
            Err(error) => self.status = format!("Screenshot failed: {error}"),
        }
    }
    fn open_rom_dialog(&mut self) {
        let mut dialog = FileDialog::new()
            .set_title("Open an NES ROM")
            .add_filter("NES ROM", &["nes"]);
        if let Some(directory) = self.rom_path.parent() {
            dialog = dialog.set_directory(directory);
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
        let bytes = fs::read(&path)?;
        let mut replacement = Nes::from_ines(&bytes)?;
        load_battery(&mut replacement, &path)?;
        self.save_battery()?;
        self.nes = replacement;
        self.rom_path = path;
        self.rom_bytes = bytes;
        self.powered = true;
        self.paused = false;
        self.frame_budget = 0.0;
        self.frame_dirty = true;
        persistence::remember_rom(&mut self.recent_roms, &self.rom_path)?;
        if let Some(audio) = &self.audio {
            audio.clear();
        }
        self.status = "ROM loaded".into();
        Ok(())
    }
    fn save_battery(&self) -> Result<(), Box<dyn Error>> {
        if self.nes.has_battery()
            && let Some(data) = self.nes.battery_ram()
        {
            persistence::atomic_write(&persistence::battery_path(&self.rom_path), data)?;
        }
        Ok(())
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.emulate(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.top_tabs(ui);
        self.status_bar(ui);
        self.game_view(ui);
        self.feature_windows(ui);
    }

    fn on_exit(&mut self) {
        let _ = self.save_battery();
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.save_battery();
    }
}

fn tab(ui: &mut egui::Ui, title: &str, open: &mut bool) {
    if ui.selectable_label(*open, title).clicked() {
        *open = !*open;
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
