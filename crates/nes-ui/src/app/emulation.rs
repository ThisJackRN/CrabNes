use super::*;

impl App {
    pub(super) fn emulate(&mut self, ctx: &egui::Context) {
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

    pub(super) fn run_one_frame(&mut self, live_input: TasFrame, present_audio: bool) -> bool {
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

    pub(super) fn toggle_pause(&mut self) {
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
    pub(super) fn advance_frame(&mut self, ctx: &egui::Context) {
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
    pub(super) fn reset(&mut self) {
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
    pub(super) fn power_cycle(&mut self) {
        if self.nes.is_some() {
            self.reset();
            self.status = "Power cycled".into();
        }
    }
    pub(super) fn toggle_power(&mut self) {
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

    pub(super) fn quick_save(&mut self) {
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
    pub(super) fn quick_load(&mut self) {
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
    pub(super) fn refresh_slots(&mut self) {
        self.state_slots = self
            .nes
            .as_ref()
            .map(|n| save_states::inspect_slots(n.rom_hash(), self.settings.save_states.slots))
            .unwrap_or_default();
        self.state_preview = None;
        self.preview_slot = None;
    }
    pub(super) fn select_slot(&mut self, slot: usize, ctx: &egui::Context) {
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
    pub(super) fn clear_audio_pipeline(&mut self) {
        self.audio_scratch.clear();
        if let Some(nes) = &mut self.nes {
            nes.drain_audio_samples(&mut self.audio_scratch);
        }
        self.audio_scratch.clear();
        if let Some(audio) = &self.audio {
            audio.clear();
        }
    }
    pub(super) fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.fullscreen = !self.fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
    }
    pub(super) fn open_rom_dialog(&mut self) {
        let mut dialog = FileDialog::new()
            .set_title("Open NES ROM")
            .add_filter("NES/FDS ROM", &["nes", "fds"]);
        if self.settings.paths.rom_folder.is_dir() {
            dialog = dialog.set_directory(&self.settings.paths.rom_folder);
        }
        if let Some(path) = dialog.pick_file() {
            self.try_load_rom(path);
        }
    }
    pub(super) fn try_load_rom(&mut self, path: PathBuf) {
        if let Err(error) = self.load_rom(path) {
            self.status = format!("Could not load ROM: {error}");
        }
    }
    pub(super) fn load_rom(&mut self, path: PathBuf) -> Result<(), Box<dyn Error>> {
        self.save_battery()?;
        let bytes = fs::read(&path)?;

        let mut bios_bytes = vec![];
        let mut bios_slice = None;
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("fds"))
            || bytes.starts_with(b"FDS\x1a")
        {
            let mut found_bios = false;
            if let Some(bios_path) = &self.settings.paths.fds_bios_path
                && let Ok(b) = fs::read(bios_path)
            {
                bios_bytes = b;
                found_bios = true;
            }
            if !found_bios {
                let local_path = std::env::current_dir()
                    .unwrap_or_default()
                    .join("disksys.rom");
                let rom_dir_path = path
                    .parent()
                    .unwrap_or(std::path::Path::new(""))
                    .join("disksys.rom");
                if let Ok(b) = fs::read(&local_path) {
                    bios_bytes = b;
                    found_bios = true;
                } else if let Ok(b) = fs::read(&rom_dir_path) {
                    bios_bytes = b;
                    found_bios = true;
                }
            }
            if found_bios {
                bios_slice = Some(bios_bytes.as_slice());
            } else {
                return Err("FDS BIOS not configured in Settings -> Paths and disksys.rom not found in ROM folder or current directory.".into());
            }
        }

        let inferred_region = inferred_region_from_rom_path(&path);
        let mut replacement = nes_from_rom_path_with_bios(&bytes, Some(&path), bios_slice)?;
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

    pub(super) fn apply_video_palette(&mut self) -> Result<Option<String>, String> {
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

    pub(super) fn apply_video_palette_with_status(&mut self) {
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

    pub(super) fn import_custom_palette(&mut self) {
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
    pub(super) fn save_battery(&self) -> Result<(), Box<dyn Error>> {
        if let (Some(nes), Some(path)) = (&self.nes, &self.rom_path)
            && nes.has_battery()
            && let Some(data) = nes.battery_ram()
        {
            persistence::atomic_write(&persistence::battery_path(path), data)?;
        }
        Ok(())
    }
    pub(super) fn save_per_game(&mut self) {
        if let Some(nes) = &self.nes
            && let Err(e) = settings::save_per_game(nes.rom_hash(), &self.per_game)
        {
            self.status = format!("Could not save per-game settings: {e}");
        }
    }

    pub(super) fn play_mode(&self) -> PlayMode {
        self.active_play_mode
    }

    pub(super) fn apply_play_mode(&mut self, mode: PlayMode) {
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

    pub(super) fn current_speed(&self) -> f64 {
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
    pub(super) fn emulation_frame_rate(&self) -> f64 {
        self.nes.as_ref().map_or(NTSC_FRAME_RATE, Nes::frame_rate)
    }
    pub(super) fn effective_volume(&self) -> f32 {
        self.per_game
            .volume
            .unwrap_or(self.settings.audio.volume)
            .clamp(0.0, 1.0)
    }
    pub(super) fn effective_muted(&self) -> bool {
        self.per_game.muted.unwrap_or(self.settings.audio.muted)
    }
    pub(super) fn take_screenshot(&mut self) {
        if let (Some(nes), Some(path)) = (&self.nes, &self.rom_path) {
            match screenshot::save(nes.frame(), path) {
                Ok(p) => self.status = format!("Screenshot: {}", p.display()),
                Err(e) => self.status = format!("Screenshot failed: {e}"),
            }
        }
    }
}

pub(super) fn inferred_region_from_rom_path(path: &Path) -> Option<Region> {
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

pub(super) fn nes_from_rom_path(
    bytes: &[u8],
    path: Option<&Path>,
) -> Result<Nes, nes_core::EmulationError> {
    nes_from_rom_path_with_bios(bytes, path, None)
}

fn nes_from_rom_path_with_bios(
    bytes: &[u8],
    path: Option<&Path>,
    bios: Option<&[u8]>,
) -> Result<Nes, nes_core::EmulationError> {
    if let Some(bios) = bios
        && (path.is_some_and(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("fds")))
            || bytes.starts_with(b"FDS\x1a"))
    {
        return Nes::from_fds(bytes, bios);
    }
    match path.and_then(inferred_region_from_rom_path) {
        Some(region) => Nes::from_ines_with_region(bytes, region),
        None => Nes::from_ines(bytes),
    }
}

pub(super) fn load_battery(nes: &mut Nes, rom_path: &Path) -> Result<(), Box<dyn Error>> {
    if nes.has_battery() {
        let path = persistence::battery_path(rom_path);
        if path.is_file() {
            nes.load_battery_ram(&fs::read(path)?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::inferred_region_from_rom_path;
    use nes_core::Region;

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
