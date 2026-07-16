use super::*;

#[derive(Clone)]
pub(super) struct AchievementToast {
    pub(super) title: String,
    pub(super) description: String,
    pub(super) points: u32,
    pub(super) badge_url: String,
    pub(super) started_at: Option<Instant>,
}

impl App {
    pub(super) fn achievements_window(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn start_achievement_session(&mut self) {
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

    pub(super) fn load_current_achievement_game(&mut self) {
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

    pub(super) fn handle_achievement_events(&mut self, events: Vec<AchievementEvent>) {
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

    pub(super) fn push_achievement_activity(&mut self, item: String) {
        self.achievement_feed.push_front(item);
        self.achievement_feed.truncate(8);
    }

    pub(super) fn ensure_achievement_badge(&mut self, url: &str) {
        if url.is_empty()
            || self.achievement_badges.contains_key(url)
            || !self.achievement_badges_requested.insert(url.to_owned())
        {
            return;
        }
        self.achievements.request_badge(url.to_owned());
    }

    pub(super) fn collect_achievement_badges(&mut self, ctx: &egui::Context) {
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

    pub(super) fn achievement_toast_overlay(&mut self, ctx: &egui::Context) {
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
}

pub(super) fn is_achievement_client_warning(
    achievement: &nes_achievements_native::Achievement,
) -> bool {
    achievement.bucket == AchievementBucket::Unsupported
        || achievement.id == 0
        || is_achievement_warning_title(&achievement.title)
}

pub(super) fn achievement_unlock_baseline(
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

pub(super) fn register_achievement_unlock(
    known_unlocked: &mut HashSet<u32>,
    achievement_id: u32,
    show_replayed: bool,
) -> (bool, bool) {
    let first_unlock = known_unlocked.insert(achievement_id);
    (first_unlock, first_unlock || show_replayed)
}

pub(super) fn is_achievement_warning_title(title: &str) -> bool {
    title
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Warning:"))
        || title.eq_ignore_ascii_case("Unsupported Game Version")
}

pub(super) fn is_archived_achievement_warning(entry: &UnlockEntry) -> bool {
    entry.achievement_id == 0 || is_achievement_warning_title(&entry.title)
}

pub(super) fn achievement_warning_banner(
    ui: &mut egui::Ui,
    warning: &nes_achievements_native::Achievement,
) {
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

pub(super) fn achievement_card(
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

pub(super) fn archive_card(
    ui: &mut egui::Ui,
    entry: &UnlockEntry,
    texture: Option<&TextureHandle>,
) {
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

pub(super) fn achievement_badge(ui: &mut egui::Ui, texture: Option<&TextureHandle>, size: f32) {
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        achievement_unlock_baseline, is_achievement_client_warning,
        is_archived_achievement_warning, register_achievement_unlock,
    };
    use crate::achievement_archive::UnlockEntry;
    use nes_achievements_native::{Achievement, AchievementBucket};

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
}
