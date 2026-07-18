use super::*;

impl App {
    pub(super) fn tas_window(&mut self, ui: &mut egui::Ui) {
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
                        if movie.fceux_joypad_compat {
                            ui.colored_label(egui::Color32::LIGHT_BLUE, "FCEUX pad timing")
                                .on_hover_text(
                                    "This movie plays with FCEUX's simplified controller \
                                     clocking (no DMC/OAM DMA read corruption). Hardware-accurate \
                                     clocking resumes when the TAS stops.",
                                );
                        }
                    });
                    if !movie.cheats.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Game Genie / cheats locked into this movie:");
                            ui.monospace(movie.cheats.join("  "));
                        });
                    }
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
            self.stop_tas_and_restore_cheats();
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

    pub(super) fn tas_control_window(&mut self, ui: &mut egui::Ui) {
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
                    ui.checkbox(
                        &mut self.tas_control_fceux_timing,
                        "FCEUX-compatible controller timing for this movie",
                    )
                    .on_hover_text(
                        "FCEUX does not emulate the DMC/OAM DMA controller-read corruption real \
                         hardware has, so FM2 movies were recorded without it. This plays the \
                         movie with FCEUX's simplified pad clocking; normal play keeps the \
                         hardware-accurate model.",
                    );
                    let enabled_codes = self.enabled_cheat_codes();
                    if !enabled_codes.is_empty() {
                        ui.checkbox(
                            &mut self.tas_control_include_cheats,
                            format!(
                                "Play through your enabled Game Genie codes ({})",
                                enabled_codes.join("  ")
                            ),
                        );
                        if self.tas_control_include_cheats {
                            ui.small(
                                "Like a real Game Genie between console and cartridge. The source \
                                 movie was recorded without these codes, so the run may play out \
                                 differently or break.",
                            );
                        }
                    }
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
                    self.tas_control_fceux_timing = movie.fceux_joypad_compat;
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

    pub(super) fn convert_tas_control_movie(&mut self) {
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
        // Foreign movies were recorded without this emulator's cheat engine.
        // Only the explicit opt-in locks the player's enabled codes into the
        // conversion, replaying the inputs through a plugged-in Game Genie;
        // either way the machine matches the converted movie's cheat list.
        if self.tas_control_include_cheats {
            converted.cheats = self.enabled_cheat_codes();
        }
        converted.fceux_joypad_compat = self.tas_control_fceux_timing;
        let cheat_count = converted.cheats.len();
        let movie_cheats = converted.parsed_cheats();
        let joypad_compat = converted.fceux_joypad_compat;
        self.tas.movie = Some(converted);
        if let Some(nes) = &mut self.nes {
            nes.set_cheats(movie_cheats);
            nes.set_fceux_joypad_compat(joypad_compat);
        }
        self.tas.set_cursor_paused_for_preview(0);
        self.tas_held_input = TasFrame::default();
        self.tas_timeline_scroll = Some(0);
        self.paused = true;
        self.fast_forward = false;
        self.frame_budget = 0.0;
        self.clear_audio_pipeline();
        self.show_tas = true;
        self.status = format!(
            "Converted {frame_count} {} frames into the native TAS editor{}{}",
            source.format,
            if imported_fceux {
                " from its embedded FCEUX state"
            } else {
                ""
            },
            if cheat_count > 0 {
                format!(" with {cheat_count} Game Genie code(s) locked in")
            } else {
                String::new()
            }
        );
        self.tas_control_status = self.status.clone();
    }

    pub(super) fn prepare_fceux_tas_start(&mut self, source: &ControlMovie) -> Result<(), String> {
        source.verify_fceux_rom(&self.rom_bytes)?;
        let fcs = source
            .embedded_fceux_state
            .as_deref()
            .ok_or_else(|| "movie has no embedded FCEUX state".to_owned())?;
        let mut nes = nes_from_rom_path(&self.rom_bytes, self.rom_path.as_deref())
            .map_err(|error| error.to_string())?;
        nes.import_fceux_state(fcs)
            .map_err(|error| error.to_string())?;
        // An embedded FCEUX state implies an FCEUX-recorded movie; play it
        // with the same simplified joypad clocking it was made against.
        nes.set_fceux_joypad_compat(true);
        let initial_state = nes.save_state().map_err(|error| error.to_string())?;
        let mut movie = TasMovie::new(
            tas::rom_sha256_hex(nes.rom_sha256()),
            TasStartType::SaveState,
            Some(initial_state.clone()),
        );
        movie.fceux_joypad_compat = true;
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

    /// Keep the editor anchored to the next TAS input that will execute.
    ///
    /// Playback calls this after every emulated frame. Once paused, no further
    /// automatic updates occur, so the user can still select and edit any row.
    pub(super) fn follow_tas_cursor(&mut self) {
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
    pub(super) fn reconcile_tas_checkpoint(&mut self, frame: usize) -> TasCheckpointRecovery {
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
        verifier.set_cheats(
            self.tas
                .movie
                .as_ref()
                .map(TasMovie::parsed_cheats)
                .unwrap_or_default(),
        );
        verifier.set_fceux_joypad_compat(
            self.tas
                .movie
                .as_ref()
                .is_some_and(|movie| movie.fceux_joypad_compat),
        );
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

    /// Enabled, decodable per-game cheat codes in canonical text form.
    pub(super) fn enabled_cheat_codes(&self) -> Vec<String> {
        self.per_game
            .cheats
            .iter()
            .filter(|entry| entry.enabled)
            .map(|entry| entry.code.trim().to_ascii_uppercase())
            .filter(|code| Cheat::parse(code).is_ok())
            .collect()
    }

    pub(super) fn new_tas_movie(&mut self, start_type: TasStartType) {
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
        let mut movie = TasMovie::new(tas::rom_sha256_hex(nes.rom_sha256()), start_type, embedded);
        // The enabled cheat codes are locked into the movie at start, exactly
        // like a physical Game Genie sitting between cartridge and console.
        movie.cheats = self.enabled_cheat_codes();
        // Likewise the joypad model: recordings capture the advanced Emulation
        // setting so deliberate FCEUX-style recordings replay consistently.
        movie.fceux_joypad_compat = self.effective_fceux_joypad_compat();
        let joypad_compat = movie.fceux_joypad_compat;
        let movie_cheats = movie.parsed_cheats();
        self.last_controller_reads = nes
            .controller_reads(0)
            .wrapping_add(nes.controller_reads(1));
        self.tas.new_movie(movie, initial_state);
        if let Some(nes) = &mut self.nes {
            nes.set_cheats(movie_cheats);
            nes.set_fceux_joypad_compat(joypad_compat);
        }
        self.tas_held_input = TasFrame::default();
        self.powered = true;
        self.paused = false;
        self.fast_forward = false;
        self.clear_rewind_history();
        self.lag_frames = 0;
        self.frame_dirty = true;
        self.follow_tas_cursor();
        self.clear_audio_pipeline();
        self.status = format!("Recording new {start_type:?} TAS at 1x");
    }

    pub(super) fn restore_tas_start(&mut self) -> Result<Vec<u8>, String> {
        if self.play_mode().restricts_assists() {
            return Err(format!(
                "TAS tools are disabled in {} mode",
                self.play_mode().label()
            ));
        }
        let (start_type, starting_state, movie_cheats, joypad_compat) = self
            .tas
            .movie
            .as_ref()
            .map(|movie| {
                (
                    movie.start_type,
                    movie.starting_state.clone(),
                    movie.parsed_cheats(),
                    movie.fceux_joypad_compat,
                )
            })
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
        // Machine snapshots carry neither cheat state nor the joypad model, so
        // both are reapplied whenever the starting condition is reconstructed.
        if let Some(nes) = &mut self.nes {
            nes.set_cheats(movie_cheats);
            nes.set_fceux_joypad_compat(joypad_compat);
        }
        let _ = self.apply_video_palette();
        self.nes
            .as_ref()
            .unwrap()
            .save_state()
            .map_err(|e| e.to_string())
    }

    pub(super) fn start_tas_playback(&mut self, read_only: bool) {
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

    pub(super) fn seek_tas(&mut self, target: usize) -> bool {
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
        let movie_cheats = self
            .tas
            .movie
            .as_ref()
            .map(TasMovie::parsed_cheats)
            .unwrap_or_default();
        let joypad_compat = self
            .tas
            .movie
            .as_ref()
            .is_some_and(|movie| movie.fceux_joypad_compat);
        {
            let Some(nes) = &mut self.nes else {
                return false;
            };
            if let Err(error) = nes.load_state(&checkpoint.state) {
                self.status = format!("Checkpoint load failed: {error}");
                return false;
            }
            nes.set_cheats(movie_cheats);
            nes.set_fceux_joypad_compat(joypad_compat);
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

    pub(super) fn apply_tas_timeline_action(&mut self, action: TasTimelineAction) {
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

    pub(super) fn export_tas(&mut self) {
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
    pub(super) fn import_tas(&mut self) {
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
}
