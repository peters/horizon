use std::collections::VecDeque;

use alacritty_terminal::term::TermMode;
use egui::emath::TSTransform;
use egui::{Key, Pos2, Rect, Vec2};
use horizon_core::{Panel, SelectionType, TerminalSide};

use crate::input;
use crate::primary_clipboard;

use super::layout::{GridMetrics, TerminalInteraction, cell_side, grid_point_from_position};
use super::scrollbar::{scrollbar_pointer_to_scrollback, scrollbar_thumb_height};

struct PointerContext<'a> {
    interaction: &'a TerminalInteraction,
    metrics: &'a GridMetrics,
    visible_rows: u16,
    visible_cols: u16,
    terminal_mode: TermMode,
    pointer_buttons: input::PointerButtons,
    current_modifiers: egui::Modifiers,
    hovered_point: Option<input::GridPoint>,
    from_global: Option<TSTransform>,
}

pub(super) fn handle_terminal_pointer_input(
    ui: &mut egui::Ui,
    panel: &mut Panel,
    interaction: &TerminalInteraction,
    is_active_panel: bool,
    metrics: &GridMetrics,
    visible_rows: u16,
    visible_cols: u16,
) {
    if interaction.body.clicked() {
        interaction.body.request_focus();
    }
    if is_active_panel && ui.input(|input| input.key_pressed(Key::Tab)) {
        interaction.body.request_focus();
    }

    let should_handle_pointer = interaction.body.hovered()
        || interaction.body.dragged()
        || interaction.body.clicked()
        || interaction.body.drag_started()
        || interaction.scrollbar.hovered()
        || interaction.scrollbar.dragged()
        || interaction.scrollbar.clicked();
    if !should_handle_pointer {
        return;
    }

    let Some(terminal_mode) = panel.terminal_mut().map(|terminal| terminal.mode()) else {
        return;
    };
    let events: Vec<egui::Event> = ui.input(|input| input.events.clone());
    let pointer_buttons = ui.input(|input| input::PointerButtons {
        primary: input.pointer.primary_down(),
        middle: input.pointer.middle_down(),
        secondary: input.pointer.secondary_down(),
    });
    let from_global = ui.ctx().layer_transform_from_global(ui.layer_id());
    let current_modifiers = ui.input(|input| input.modifiers);
    let hovered_point = ui
        .input(|input| input.pointer.hover_pos())
        .map(|position| transform_pos(from_global, position))
        .filter(|position| interaction.layout.body.contains(*position))
        .and_then(|position| {
            grid_point_from_position(interaction.layout.body, position, metrics, visible_rows, visible_cols)
        });
    let pointer_context = PointerContext {
        interaction,
        metrics,
        visible_rows,
        visible_cols,
        terminal_mode,
        pointer_buttons,
        current_modifiers,
        hovered_point,
        from_global,
    };

    handle_pointer_events(&events, panel, &pointer_context);

    // Copy selected text to primary selection buffer when a selection
    // completes (drag release or double/triple click).
    if (interaction.body.drag_stopped() || interaction.body.double_clicked() || interaction.body.triple_clicked())
        && let Some(terminal) = panel.terminal()
        && let Some(text) = terminal.selection_to_string()
    {
        primary_clipboard::copy_to_primary(&text);
    }

    handle_scrollbar_drag(ui, panel, interaction, visible_rows);

    // Show pointing hand when Ctrl/Cmd hovering over clickable content.
    if ui.input(|input| input.modifiers.ctrl || input.modifiers.command)
        && let Some(point) = pointer_context.hovered_point
        && let Some(terminal) = panel.terminal()
        && terminal.clickable_at_point(point.line, point.column).is_some()
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

fn mouse_mode_active(terminal_mode: TermMode, modifiers: egui::Modifiers) -> bool {
    !modifiers.shift && terminal_mode.intersects(alacritty_terminal::term::TermMode::MOUSE_MODE)
}

fn handle_pointer_events(events: &[egui::Event], panel: &mut Panel, pointer: &PointerContext<'_>) {
    for event in events {
        match event {
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers,
            } => {
                let pos = transform_pos(pointer.from_global, *pos);
                if !pointer.interaction.layout.body.contains(pos) {
                    continue;
                }
                if *pressed {
                    pointer.interaction.body.request_focus();
                }
                handle_pointer_button(panel, pointer, pos, *button, *pressed, *modifiers);
            }
            egui::Event::PointerMoved(pos) => {
                let pos = transform_pos(pointer.from_global, *pos);
                let inside = pointer.interaction.layout.body.contains(pos);
                if inside && mouse_mode_active(pointer.terminal_mode, pointer.current_modifiers) {
                    if let Some(point) = grid_point_from_position(
                        pointer.interaction.layout.body,
                        pos,
                        pointer.metrics,
                        pointer.visible_rows,
                        pointer.visible_cols,
                    ) && let Some(bytes) = input::mouse_motion_report(
                        pointer.pointer_buttons,
                        pointer.current_modifiers,
                        pointer.terminal_mode,
                        point,
                    ) && !bytes.is_empty()
                    {
                        panel.write_input(&bytes);
                    }
                } else if pointer.interaction.body.dragged() && panel.terminal_mut().is_some_and(|t| t.has_selection())
                {
                    handle_pointer_selection_drag(
                        panel,
                        pos,
                        pointer.interaction.layout.body,
                        pointer.metrics,
                        pointer.visible_rows,
                        pointer.visible_cols,
                    );
                }
            }
            egui::Event::MouseWheel { delta, unit, modifiers } => {
                if modifiers.ctrl || modifiers.command {
                    continue;
                }
                if let Some(point) = pointer.hovered_point
                    && let Some(action) = input::wheel_action(
                        *delta,
                        *unit,
                        Vec2::new(pointer.metrics.char_width, pointer.metrics.line_height),
                        *modifiers,
                        pointer.terminal_mode,
                        point,
                    )
                {
                    match action {
                        input::WheelAction::Pty(bytes) if !bytes.is_empty() => panel.write_input(&bytes),
                        input::WheelAction::Pty(_) => {}
                        input::WheelAction::Scrollback(lines) => panel.scroll_scrollback_by(lines),
                    }
                }
            }
            _ => {}
        }
    }
}

fn handle_pointer_button(
    panel: &mut Panel,
    pointer: &PointerContext<'_>,
    pos: Pos2,
    button: egui::PointerButton,
    pressed: bool,
    modifiers: egui::Modifiers,
) {
    // Ctrl+click / Cmd+click opens URLs and file paths regardless of mouse mode.
    if (modifiers.ctrl || modifiers.command)
        && button == egui::PointerButton::Primary
        && pressed
        && let Some(point) = grid_point_from_position(
            pointer.interaction.layout.body,
            pos,
            pointer.metrics,
            pointer.visible_rows,
            pointer.visible_cols,
        )
        && let Some(terminal) = panel.terminal()
        && let Some(target) = terminal.clickable_at_point(point.line, point.column)
    {
        horizon_core::open_url(&target);
        return;
    }

    if mouse_mode_active(pointer.terminal_mode, modifiers) {
        if let Some(point) = grid_point_from_position(
            pointer.interaction.layout.body,
            pos,
            pointer.metrics,
            pointer.visible_rows,
            pointer.visible_cols,
        ) && let Some(bytes) = input::mouse_button_report(button, pressed, modifiers, pointer.terminal_mode, point)
            && !bytes.is_empty()
        {
            panel.write_input(&bytes);
        }
    } else if button == egui::PointerButton::Middle && pressed && !modifiers.ctrl && !modifiers.command {
        if let Some(text) = primary_clipboard::paste_from_primary() {
            let bytes = input::paste_bytes(&text, pointer.terminal_mode, true);
            panel.write_input(&bytes);
        }
    } else if button == egui::PointerButton::Primary
        && pressed
        && let Some(point) = grid_point_from_position(
            pointer.interaction.layout.body,
            pos,
            pointer.metrics,
            pointer.visible_rows,
            pointer.visible_cols,
        )
    {
        let sel_type = if pointer.interaction.body.triple_clicked() {
            SelectionType::Lines
        } else if pointer.interaction.body.double_clicked() {
            SelectionType::Semantic
        } else {
            SelectionType::Simple
        };
        if let Some(terminal) = panel.terminal_mut() {
            terminal.start_selection(sel_type, point.line, point.column);
        }
    }
}

fn handle_scrollbar_drag(ui: &mut egui::Ui, panel: &mut Panel, interaction: &TerminalInteraction, visible_rows: u16) {
    let from_global = ui.ctx().layer_transform_from_global(ui.layer_id());
    if (interaction.scrollbar.dragged() || interaction.scrollbar.clicked())
        && let Some(pointer_position) = ui
            .input(|input| input.pointer.interact_pos())
            .map(|position| transform_pos(from_global, position))
    {
        let history_size = panel.terminal().map_or(0, horizon_core::Terminal::history_size);
        let target_scrollback = scrollbar_pointer_to_scrollback(
            pointer_position,
            interaction.scrollbar.rect.shrink2(Vec2::new(2.0, 2.0)),
            scrollbar_thumb_height(interaction.scrollbar.rect.height() - 4.0, visible_rows, history_size),
            history_size,
        );
        panel.set_scrollback(target_scrollback);
    }
}

fn transform_pos(from_global: Option<TSTransform>, pos: Pos2) -> Pos2 {
    from_global.map_or(pos, |transform| transform * pos)
}

fn handle_pointer_selection_drag(
    panel: &mut Panel,
    pos: Pos2,
    body_rect: Rect,
    metrics: &GridMetrics,
    visible_rows: u16,
    visible_cols: u16,
) {
    if pos.y < body_rect.min.y {
        let overshoot = body_rect.min.y - pos.y;
        let lines = (overshoot / metrics.line_height).ceil().max(1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let lines = (lines as i32).min(5);
        panel.scroll_scrollback_by(lines);
        if let Some(terminal) = panel.terminal_mut() {
            terminal.update_selection(0, 0, TerminalSide::Left);
        }
    } else if pos.y > body_rect.max.y {
        let overshoot = pos.y - body_rect.max.y;
        let lines = (overshoot / metrics.line_height).ceil().max(1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let lines = (lines as i32).min(5);
        panel.scroll_scrollback_by(-lines);
        let last_row = visible_rows.saturating_sub(1);
        let last_col = visible_cols.saturating_sub(1);
        if let Some(terminal) = panel.terminal_mut() {
            terminal.update_selection(usize::from(last_row), usize::from(last_col), TerminalSide::Right);
        }
    } else if let Some(point) = grid_point_from_position(body_rect, pos, metrics, visible_rows, visible_cols) {
        let side = cell_side(pos, body_rect, metrics, point);
        if let Some(terminal) = panel.terminal_mut() {
            terminal.update_selection(point.line, point.column, side);
        }
    }
}

pub(super) fn handle_terminal_keyboard_input(ui: &egui::Ui, panel: &mut Panel) {
    let events: Vec<egui::Event> = ui.input(|input| input.events.clone());
    let Some(terminal) = panel.terminal_mut() else {
        return;
    };
    let mode = terminal.mode();
    let mut forwarder = KeyboardInputForwarder::default();

    for event in &events {
        match event {
            egui::Event::Text(text) | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                let emission = forwarder.on_text(text, mode);
                if emission.clears_selection {
                    terminal.clear_selection();
                }
                if !emission.bytes.is_empty() {
                    terminal.write_input(&emission.bytes);
                }
            }
            egui::Event::Paste(text) => {
                terminal.clear_selection();
                let bytes = input::paste_bytes(text, mode, true);
                terminal.write_input(&bytes);
            }
            egui::Event::Copy => {
                if let Some(text) = terminal.selection_to_string() {
                    primary_clipboard::copy_to_primary(&text);
                    ui.ctx().copy_text(text);
                    terminal.clear_selection();
                } else {
                    terminal.write_input(&[3]);
                }
            }
            egui::Event::Cut => {
                if let Some(text) = terminal.selection_to_string() {
                    primary_clipboard::copy_to_primary(&text);
                    ui.ctx().copy_text(text);
                    terminal.clear_selection();
                }
                terminal.write_input(&[24]);
            }
            egui::Event::Key {
                key,
                physical_key,
                pressed,
                repeat,
                modifiers,
                ..
            } => {
                let emission = forwarder.on_key(*key, *physical_key, *pressed, *repeat, *modifiers, mode);
                if !emission.bytes.is_empty() {
                    terminal.write_input(&emission.bytes);
                }
            }
            _ => {}
        }
    }

    let emission = forwarder.finish();
    if !emission.bytes.is_empty() {
        terminal.write_input(&emission.bytes);
    }
}

#[derive(Default)]
struct KeyboardInputForwarder {
    suppressed_text: VecDeque<String>,
    deferred_text_key: Option<DeferredTextKey>,
}

impl KeyboardInputForwarder {
    fn on_text(&mut self, text: &str, mode: TermMode) -> InputEmission {
        if let Some(mut deferred) = self.deferred_text_key.take() {
            if let Some(actual_text) = deferred.synthetic_text.as_deref() {
                if actual_text != text {
                    // Drop stale synthetic state if a later text event does not
                    // belong to the deferred key.
                    return InputEmission::raw_text(text);
                }
            } else {
                let emission = deferred.resolve_text(text, mode);
                if deferred.synthetic_text.is_some() {
                    self.deferred_text_key = Some(deferred);
                }
                return emission;
            }
        }

        if self.suppressed_text.front().is_some_and(|expected| expected == text) {
            self.suppressed_text.pop_front();
            return InputEmission::default();
        }

        InputEmission::raw_text(text)
    }

    fn on_key(
        &mut self,
        key: Key,
        physical_key: Option<Key>,
        pressed: bool,
        repeat: bool,
        modifiers: egui::Modifiers,
        mode: TermMode,
    ) -> InputEmission {
        let mut emission = InputEmission::default();

        if let Some(deferred) = self.deferred_text_key.as_mut() {
            if let Some(actual_text) = deferred.synthetic_text.as_deref() {
                if !pressed && deferred.matches(key, physical_key) {
                    if let Some(translation) =
                        input::translate_text_event(key, physical_key, actual_text, false, repeat, modifiers, mode)
                    {
                        emission.bytes.extend_from_slice(&translation.bytes);
                    }
                    self.deferred_text_key = None;
                    return emission;
                }

                if !deferred.matches(key, physical_key) {
                    self.deferred_text_key = None;
                }
            } else if !pressed && deferred.matches(key, physical_key) {
                deferred.release_seen = true;
                deferred.release_translation =
                    input::translate_key_event_with_physical(key, physical_key, false, repeat, modifiers, mode);
                return emission;
            } else if !deferred.matches(key, physical_key) {
                emission.bytes.extend_from_slice(&deferred.flush_fallback());
                self.deferred_text_key = None;
            }
        }

        if let Some(translation) =
            input::translate_key_event_with_physical(key, physical_key, pressed, repeat, modifiers, mode)
        {
            if pressed
                && translation.suppress_text.is_some()
                && (modifiers.alt
                    || mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL)
                    || input::should_defer_textual_key(key, physical_key, pressed, modifiers, mode))
            {
                self.deferred_text_key = Some(DeferredTextKey::new(key, physical_key, modifiers, Some(translation)));
                return emission;
            }

            if let Some(text) = translation.suppress_text {
                self.suppressed_text.push_back(text);
            }
            emission.bytes.extend_from_slice(&translation.bytes);
            return emission;
        }

        if input::should_defer_textual_key(key, physical_key, pressed, modifiers, mode) {
            self.deferred_text_key = Some(DeferredTextKey::new(key, physical_key, modifiers, None));
        }

        emission
    }

    fn finish(&mut self) -> InputEmission {
        let Some(deferred) = self.deferred_text_key.take() else {
            return InputEmission::default();
        };

        if deferred.synthetic_text.is_some() {
            return InputEmission::default();
        }

        InputEmission::pty(deferred.flush_fallback())
    }
}

struct DeferredTextKey {
    key: Key,
    physical_key: Option<Key>,
    modifiers: egui::Modifiers,
    press_translation: Option<input::KeyTranslation>,
    release_translation: Option<input::KeyTranslation>,
    release_seen: bool,
    synthetic_text: Option<String>,
}

impl DeferredTextKey {
    fn new(
        key: Key,
        physical_key: Option<Key>,
        modifiers: egui::Modifiers,
        press_translation: Option<input::KeyTranslation>,
    ) -> Self {
        Self {
            key,
            physical_key,
            modifiers,
            press_translation,
            release_translation: None,
            release_seen: false,
            synthetic_text: None,
        }
    }

    fn matches(&self, key: Key, physical_key: Option<Key>) -> bool {
        self.key == key && self.physical_key == physical_key
    }

    fn resolve_text(&mut self, text: &str, mode: TermMode) -> InputEmission {
        if self
            .press_translation
            .as_ref()
            .and_then(|translation| translation.suppress_text.as_deref())
            .is_some_and(|expected| expected == text)
        {
            return InputEmission::pty(self.flush_fallback());
        }

        if let Some(translation) =
            input::translate_text_event(self.key, self.physical_key, text, true, false, self.modifiers, mode)
        {
            let mut bytes = translation.bytes;
            if self.release_seen {
                if let Some(release) =
                    input::translate_text_event(self.key, self.physical_key, text, false, false, self.modifiers, mode)
                {
                    bytes.extend_from_slice(&release.bytes);
                }
            } else {
                self.synthetic_text = Some(text.to_owned());
            }
            return InputEmission::pty(bytes);
        }

        InputEmission::raw_text(text)
    }

    fn flush_fallback(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        if let Some(translation) = self.press_translation.as_ref() {
            bytes.extend_from_slice(&translation.bytes);
        }
        if self.release_seen
            && let Some(translation) = self.release_translation.as_ref()
        {
            bytes.extend_from_slice(&translation.bytes);
        }
        bytes
    }
}

#[derive(Default)]
struct InputEmission {
    bytes: Vec<u8>,
    clears_selection: bool,
}

impl InputEmission {
    fn pty(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            clears_selection: false,
        }
    }

    fn raw_text(text: &str) -> Self {
        Self {
            bytes: text.as_bytes().to_vec(),
            clears_selection: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::KeyboardInputForwarder;
    use alacritty_terminal::term::TermMode;
    use egui::{Event, Key, Modifiers};

    #[test]
    fn altgr_text_after_release_emits_only_kitty_sequences() {
        let events = vec![
            Event::Key {
                key: Key::Num2,
                physical_key: Some(Key::Num2),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::ALT,
            },
            Event::Key {
                key: Key::Num2,
                physical_key: Some(Key::Num2),
                pressed: false,
                repeat: false,
                modifiers: Modifiers::ALT,
            },
            Event::Text("@".to_owned()),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        assert_eq!(bytes, b"\x1b[50:64;3u\x1b[50:64;3:3u");
    }

    #[test]
    fn shifted_symbol_uses_text_reconciliation_for_release_order() {
        let events = vec![
            Event::Key {
                key: Key::Num2,
                physical_key: Some(Key::Num2),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::SHIFT,
            },
            Event::Text("@".to_owned()),
            Event::Key {
                key: Key::Num2,
                physical_key: Some(Key::Num2),
                pressed: false,
                repeat: false,
                modifiers: Modifiers::SHIFT,
            },
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        assert_eq!(bytes, b"\x1b[50:64;2u\x1b[50:64;2:3u");
    }

    /// Regression: on some Linux setups, `AltGr` is NOT reported as
    /// `modifiers.alt` by winit.  When kitty keyboard protocol is active,
    /// the key press was immediately emitted as a kitty sequence for the
    /// base key ("2") and the text event ("@") passed through as raw
    /// text because suppression expected "2".  Result: "2@" instead of
    /// just the kitty sequence for "@".
    #[test]
    fn altgr_without_alt_modifier_in_kitty_mode_does_not_leak_base_key() {
        let events = vec![
            Event::Key {
                key: Key::Num2,
                physical_key: Some(Key::Num2),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
            Event::Text("@".to_owned()),
            Event::Key {
                key: Key::Num2,
                physical_key: Some(Key::Num2),
                pressed: false,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        // Must produce kitty sequences for "@" (codepoint 64), NOT
        // the base key "2" (codepoint 50) followed by raw "@".
        // Release includes ";1" (no-modifier marker) because
        // REPORT_EVENT_TYPES forces the modifier field.
        assert_eq!(bytes, b"\x1b[50:64u\x1b[50:64;1:3u");
    }

    /// Same scenario as above but in non-kitty mode: the text event
    /// should pass through as raw "@" with no preceding "2".
    #[test]
    fn altgr_without_alt_modifier_in_legacy_mode_emits_only_text() {
        let events = vec![
            Event::Key {
                key: Key::Num2,
                physical_key: Some(Key::Num2),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
            Event::Text("@".to_owned()),
            Event::Key {
                key: Key::Num2,
                physical_key: Some(Key::Num2),
                pressed: false,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
        ];

        let bytes = forward_bytes(&events, TermMode::NONE);

        assert_eq!(bytes, b"@");
    }

    #[test]
    fn legacy_c0_key_events_are_forwarded_in_legacy_mode() {
        let cases: [(&str, Event, &[u8]); 6] = [
            (
                "shift enter",
                Event::Key {
                    key: Key::Enter,
                    physical_key: Some(Key::Enter),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::SHIFT,
                },
                b"\r",
            ),
            (
                "alt escape",
                Event::Key {
                    key: Key::Escape,
                    physical_key: Some(Key::Escape),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::ALT,
                },
                b"\x1b\x1b",
            ),
            (
                "ctrl backspace",
                Event::Key {
                    key: Key::Backspace,
                    physical_key: Some(Key::Backspace),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::CTRL,
                },
                b"\x08",
            ),
            (
                "alt backspace",
                Event::Key {
                    key: Key::Backspace,
                    physical_key: Some(Key::Backspace),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::ALT,
                },
                b"\x1b\x7f",
            ),
            (
                "ctrl shift tab",
                Event::Key {
                    key: Key::Tab,
                    physical_key: Some(Key::Tab),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::CTRL | Modifiers::SHIFT,
                },
                b"\x1b[Z",
            ),
            (
                "alt shift tab",
                Event::Key {
                    key: Key::Tab,
                    physical_key: Some(Key::Tab),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::ALT | Modifiers::SHIFT,
                },
                b"\x1b\x1b[Z",
            ),
        ];

        for (name, event, expected) in cases {
            let bytes = forward_bytes(&[event], TermMode::NONE);
            assert_eq!(bytes, expected, "{name}");
        }
    }

    fn forward_bytes(events: &[Event], mode: TermMode) -> Vec<u8> {
        let mut forwarder = KeyboardInputForwarder::default();
        let mut bytes = Vec::new();

        for event in events {
            let emission = match event {
                Event::Text(text) | Event::Ime(egui::ImeEvent::Commit(text)) => forwarder.on_text(text, mode),
                Event::Key {
                    key,
                    physical_key,
                    pressed,
                    repeat,
                    modifiers,
                } => forwarder.on_key(*key, *physical_key, *pressed, *repeat, *modifiers, mode),
                _ => continue,
            };
            bytes.extend_from_slice(&emission.bytes);
        }

        bytes.extend_from_slice(&forwarder.finish().bytes);
        bytes
    }
}
