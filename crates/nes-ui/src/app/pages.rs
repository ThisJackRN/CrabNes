use super::*;

impl App {
    pub(super) fn top_bar(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn central(&mut self, ui: &mut egui::Ui) {
        match self.page {
            MainPage::Game => self.game_page(ui),
            MainPage::Library => self.library_page(ui),
        }
    }

    pub(super) fn game_page(&mut self, ui: &mut egui::Ui) {
        // Present the console output on a margin-free black letterbox so the
        // frame reads as a screen instead of a widget floating in a panel.
        let frame = if self.nes.is_some() {
            egui::Frame::new().fill(egui::Color32::BLACK)
        } else {
            egui::Frame::central_panel(ui.style())
        };
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
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
                ui.add_space((ui.available_height() / 2.0 - 32.0).max(0.0));
                ui.vertical_centered(|ui| {
                    ui.heading("No ROM loaded");
                    ui.add_space(8.0);
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
            // Center the frame in both axes and snap it to whole physical
            // pixels so resizing never leaves it top-anchored or shimmering.
            let (rect, _) = ui.allocate_exact_size(available, egui::Sense::hover());
            let pixels_per_point = ui.ctx().pixels_per_point();
            let snap = |value: f32| (value * pixels_per_point).round() / pixels_per_point;
            let image_rect = egui::Rect::from_min_size(
                egui::pos2(
                    snap(rect.center().x - size.x / 2.0),
                    snap(rect.center().y - size.y / 2.0),
                ),
                size,
            );
            egui::Image::new(&self.texture).paint_at(ui, image_rect);
        });
        self.achievement_toast_overlay(ui.ctx());
    }

    pub(super) fn library_page(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn library_cover_texture(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
    ) -> Option<TextureHandle> {
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

    pub(super) fn refresh_library_and_artwork(&mut self) {
        let folder = self.settings.paths.rom_folder.clone();
        self.library.refresh(Some(&folder));
        self.queue_library_artwork();
    }

    pub(super) fn queue_library_artwork(&mut self) {
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

    pub(super) fn collect_library_artwork(&mut self) {
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

    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn feature_windows(&mut self, ui: &mut egui::Ui) {
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
}

pub(super) fn decode_cover_image(path: &Path) -> Result<ColorImage, String> {
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
