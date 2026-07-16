use super::*;

impl App {
    pub(super) fn settings_window(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn states_window(&mut self, ui: &mut egui::Ui) {
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
                    let scale = (ui.available_width() / 256.0).clamp(0.1, 1.0);
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

    pub(super) fn time_window(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn held_frame_advance_button(&mut self, ui: &mut egui::Ui, label: &str) {
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

    pub(super) fn input_mapping_ui(&mut self, ui: &mut egui::Ui) -> bool {
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

    pub(super) fn input_window(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn av_window(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn debugger_window(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn hex_window(&mut self, ui: &mut egui::Ui) {
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
}

pub(super) fn palette_settings_ui(
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

pub(super) fn crt_signature(
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

pub(super) fn crt_settings_ui(ui: &mut egui::Ui, video: &mut VideoSettings) -> bool {
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

pub(super) fn palette_preview(ui: &mut egui::Ui, palette: &OutputPalette) {
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

pub(super) fn memory_label(space: MemorySpace) -> &'static str {
    match space {
        MemorySpace::CpuRam => "CPU RAM",
        MemorySpace::PpuNametable => "PPU nametables",
        MemorySpace::Palette => "Palette RAM",
        MemorySpace::Oam => "OAM",
        MemorySpace::PrgRom => "PRG ROM (read-only)",
        MemorySpace::Chr => "CHR",
    }
}
