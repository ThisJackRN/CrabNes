use super::*;

impl App {
    pub(super) fn handle_hotkeys(&mut self, ctx: &egui::Context) {
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

    pub(super) fn hotkey_pressed(&self, ctx: &egui::Context, key: Key) -> bool {
        !self.key_is_controller_binding(key) && ctx.input(|input| input.key_pressed(key))
    }

    pub(super) fn key_is_controller_binding(&self, key: Key) -> bool {
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

    pub(super) fn host_input_frame(&self, ctx: &egui::Context) -> TasFrame {
        if self.binding_capture.is_some() {
            return TasFrame::default();
        }
        if ctx.egui_wants_keyboard_input() {
            return self.gamepad_input_frame();
        }
        self.bound_input_frame(ctx)
    }

    pub(super) fn bound_input_frame(&self, ctx: &egui::Context) -> TasFrame {
        let gamepad = self.gamepad_input_frame();
        self.filter_live_dpad(TasFrame {
            player1: binding_mask(ctx, &self.settings.input.bindings) | gamepad.player1,
            player2: binding_mask(ctx, &self.settings.input.player2_bindings) | gamepad.player2,
        })
    }

    pub(super) fn gamepad_input_frame(&self) -> TasFrame {
        self.filter_live_dpad(TasFrame {
            player1: self.gamepad_mask(0, &self.settings.input.gamepad_bindings),
            player2: self.gamepad_mask(1, &self.settings.input.player2_gamepad_bindings),
        })
    }

    pub(super) fn vs_coin_down(&self, ctx: &egui::Context) -> bool {
        if self.binding_capture.is_some() || ctx.egui_wants_keyboard_input() {
            return false;
        }
        let keyboard = binding_down(ctx, &self.settings.input.vs_coin_binding);
        keyboard || self.vs_coin_gamepad_down()
    }

    pub(super) fn vs_coin_gamepad_down(&self) -> bool {
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

    pub(super) fn filter_live_dpad(&self, input: TasFrame) -> TasFrame {
        if self.play_mode() == PlayMode::Standard && self.settings.input.allow_opposite_directions {
            input
        } else {
            TasFrame {
                player1: neutralize_opposite_directions(input.player1),
                player2: neutralize_opposite_directions(input.player2),
            }
        }
    }

    pub(super) fn poll_input_devices(&mut self, ctx: &egui::Context) {
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

    pub(super) fn gamepad_mask(&self, player: usize, bindings: &[Option<GamepadBinding>; 8]) -> u8 {
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
}

pub(super) fn set_controller_mask(nes: &mut Nes, port: usize, mask: u8) {
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

pub(super) fn binding_mask(ctx: &egui::Context, bindings: &[KeyBinding; 8]) -> u8 {
    bindings
        .iter()
        .enumerate()
        .fold(0, |mask, (index, binding)| {
            mask | (u8::from(binding_down(ctx, binding)) << index)
        })
}

pub(super) fn input_mask_editor(ui: &mut egui::Ui, mask: &mut u8, player: &str, frame: usize) {
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

pub(super) fn input_mask_label(mask: u8) -> String {
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

pub(super) fn binding_down(ctx: &egui::Context, binding: &KeyBinding) -> bool {
    if binding.label() == "Shift" {
        return ctx.input(|i| i.key_down(Key::ShiftLeft) || i.key_down(Key::ShiftRight));
    }
    Key::ALL
        .iter()
        .copied()
        .find(|key| key.name() == binding.label())
        .is_some_and(|key| ctx.input(|input| input.key_down(key)))
}

pub(super) fn nes_button_label(index: usize) -> &'static str {
    ["A", "B", "Select", "Start", "Up", "Down", "Left", "Right"][index]
}

pub(super) fn gamepad_binding_down(
    gamepad: &Gamepad<'_>,
    binding: GamepadBinding,
    threshold: f32,
) -> bool {
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

pub(super) fn axis_active(value: f32, direction: i8, threshold: f32) -> bool {
    if direction < 0 {
        value <= -threshold
    } else {
        value >= threshold
    }
}

pub(super) fn low_input_cutoff(threshold: f32) -> f32 {
    (1.0 - threshold.clamp(0.1, 0.9)) * 0.5
}

pub(super) fn low_input_active(value: f32, threshold: f32) -> bool {
    value < low_input_cutoff(threshold)
}

pub(super) fn raw_value_known(gamepad: &Gamepad<'_>, code: gilrs::ev::Code) -> bool {
    gamepad.state().button_data(code).is_some() || gamepad.state().axis_data(code).is_some()
}

pub(super) fn gamepad_binding_label(binding: GamepadBinding) -> String {
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

pub(super) fn describe_gamepad_event(event: EventType) -> Option<String> {
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

pub(super) fn direction_label(direction: i8) -> &'static str {
    if direction < 0 { "−" } else { "+" }
}

pub(super) fn gamepad_button_label(button: GamepadButton) -> &'static str {
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

pub(super) fn gamepad_axis_label(axis: Axis) -> &'static str {
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

pub(super) fn neutralize_opposite_directions(mut mask: u8) -> u8 {
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

#[cfg(test)]
mod tests {
    use super::{low_input_active, neutralize_opposite_directions};

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
}
