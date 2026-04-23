use std::collections::VecDeque;

use alacritty_terminal::term::TermMode;
use egui::emath::TSTransform;
use egui::{Key, PointerButton, Pos2, Rect, Vec2};
use horizon_core::{
    Panel, PanelKind, SelectionType, ShortcutBinding, ShortcutKey, ShortcutModifiers, SshConnectionStatus, TerminalSide,
};

use super::super::input::{self, TerminalInputEvent};
use super::super::primary_selection::PrimarySelection;
use crate::app::shortcuts::shortcut_event_matches;

use super::ime::{prepare_terminal_keyboard_events, store_terminal_ime_enabled, terminal_ime_enabled};
use super::layout::{GridMetrics, TerminalInteraction, cell_side, grid_point_from_position};
use super::scrollbar::{scrollbar_pointer_to_scrollback, scrollbar_thumb_height};

pub(crate) const SSH_RECONNECT_SHORTCUT: ShortcutBinding =
    ShortcutBinding::new(ShortcutModifiers::PRIMARY_SHIFT, ShortcutKey::Letter('R'));

#[derive(Clone, Copy)]
pub(super) struct PointerSupport<'a> {
    pub metrics: &'a GridMetrics,
    pub visible_rows: u16,
    pub visible_cols: u16,
    pub primary_selection: &'a PrimarySelection,
}

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
    active_pointer_pos: Option<Pos2>,
    primary_selection: &'a PrimarySelection,
    ui_ctx: egui::Context,
}

pub(super) fn handle_terminal_pointer_input(
    ui: &mut egui::Ui,
    panel: &mut Panel,
    interaction: &TerminalInteraction,
    is_active_panel: bool,
    support: PointerSupport<'_>,
) {
    if interaction.body.clicked() {
        interaction.body.request_focus();
    }
    if is_active_panel && ui.input(|input| input.key_pressed(Key::Tab)) {
        interaction.body.request_focus();
    }

    let from_global = ui.ctx().layer_transform_from_global(ui.layer_id());
    let body_pointer_pos = response_pointer_pos(&interaction.body);
    let scrollbar_pointer_pos = response_pointer_pos(&interaction.scrollbar);

    // Check cheap interaction-state conditions before cloning the event list.
    // For most panels the pointer is elsewhere, so we exit early and avoid the
    // per-panel Vec<Event> clone entirely.
    let should_handle_pointer = body_pointer_pos.is_some()
        || scrollbar_pointer_pos.is_some()
        || interaction.body.is_pointer_button_down_on()
        || interaction.scrollbar.is_pointer_button_down_on()
        || interaction.body.drag_stopped_by(PointerButton::Primary)
        || interaction.body.double_clicked()
        || interaction.body.triple_clicked()
        || interaction.body.clicked_by(PointerButton::Middle)
        || interaction.scrollbar.clicked()
        || ui.input(|input| {
            pointer_event_targets_rect(&input.events, from_global, interaction.layout.body)
                || pointer_event_targets_rect(&input.events, from_global, interaction.layout.scrollbar)
        });
    if !should_handle_pointer {
        return;
    }

    // Only clone events for the panel that actually needs pointer processing.
    let events: Vec<egui::Event> = ui.input(|input| input.events.clone());
    let body_primary_press_pos = pointer_button_event_pos(
        &events,
        from_global,
        PointerButton::Primary,
        true,
        interaction.layout.body,
    );
    let body_middle_press_pos = pointer_button_event_pos(
        &events,
        from_global,
        PointerButton::Middle,
        true,
        interaction.layout.body,
    );

    let Some(terminal_mode) = panel.terminal_mut().map(|terminal| terminal.mode()) else {
        return;
    };
    let pointer_buttons = ui.input(|input| input::PointerButtons {
        primary: input.pointer.primary_down(),
        middle: input.pointer.middle_down(),
        secondary: input.pointer.secondary_down(),
    });
    let current_modifiers = ui.input(|input| input.modifiers);
    let active_pointer_pos = ui
        .input(|input| input.pointer.interact_pos())
        .map(|position| transform_pos(from_global, position));
    let hovered_point = interaction
        .body
        .hover_pos()
        .filter(|position| interaction.layout.body.contains(*position))
        .and_then(|position| {
            grid_point_from_position(
                interaction.layout.body,
                position,
                support.metrics,
                support.visible_rows,
                support.visible_cols,
            )
        });
    let pointer_context = PointerContext {
        interaction,
        metrics: support.metrics,
        visible_rows: support.visible_rows,
        visible_cols: support.visible_cols,
        terminal_mode,
        pointer_buttons,
        current_modifiers,
        hovered_point,
        from_global,
        active_pointer_pos,
        primary_selection: support.primary_selection,
        ui_ctx: ui.ctx().clone(),
    };

    if !mouse_mode_active(terminal_mode, current_modifiers) {
        handle_terminal_body_pointer_actions(panel, &pointer_context, body_primary_press_pos, body_middle_press_pos);
    }
    handle_pointer_events(&events, panel, &pointer_context);
    maybe_copy_selection_to_primary(panel, interaction, support.primary_selection);

    handle_scrollbar_drag(ui, panel, interaction, support.visible_rows);

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
                if !mouse_mode_active(pointer.terminal_mode, *modifiers) {
                    continue;
                }
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
                if inside
                    && mouse_mode_active(pointer.terminal_mode, pointer.current_modifiers)
                    && let Some(point) = grid_point_from_position(
                        pointer.interaction.layout.body,
                        pos,
                        pointer.metrics,
                        pointer.visible_rows,
                        pointer.visible_cols,
                    )
                    && let Some(bytes) = input::mouse_motion_report(
                        pointer.pointer_buttons,
                        pointer.current_modifiers,
                        pointer.terminal_mode,
                        point,
                    )
                    && !bytes.is_empty()
                {
                    panel.write_input(&bytes);
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

fn handle_terminal_body_pointer_actions(
    panel: &mut Panel,
    pointer: &PointerContext<'_>,
    body_primary_press_pos: Option<Pos2>,
    body_middle_press_pos: Option<Pos2>,
) {
    let body_pointer_pos = pointer
        .active_pointer_pos
        .or_else(|| response_pointer_pos(&pointer.interaction.body));

    if (pointer.current_modifiers.ctrl || pointer.current_modifiers.command)
        && let Some(pos) = body_primary_press_pos
    {
        handle_pointer_button(
            panel,
            pointer,
            pos,
            PointerButton::Primary,
            true,
            pointer.current_modifiers,
        );
        return;
    }

    if body_middle_press_pos.is_some()
        && should_request_primary_paste(PointerButton::Middle, true, pointer.current_modifiers)
    {
        pointer
            .primary_selection
            .request_paste(panel.id, pointer.ui_ctx.clone());
        return;
    }

    if let Some(pos) = body_primary_press_pos {
        handle_pointer_button(
            panel,
            pointer,
            pos,
            PointerButton::Primary,
            true,
            pointer.current_modifiers,
        );
    }

    if pointer.pointer_buttons.primary
        && pointer.interaction.body.is_pointer_button_down_on()
        && panel.terminal().is_some_and(horizon_core::Terminal::has_selection)
        && let Some(pos) = body_pointer_pos
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
        let side = cell_side(pos, pointer.interaction.layout.body, pointer.metrics, point);
        if let Some(terminal) = panel.terminal_mut() {
            terminal.start_selection(sel_type, point.line, point.column, side);
        }
    }
}

fn maybe_copy_selection_to_primary(
    panel: &Panel,
    interaction: &TerminalInteraction,
    primary_selection: &PrimarySelection,
) {
    if !selection_copy_completed(
        interaction.body.drag_stopped_by(PointerButton::Primary),
        interaction.body.double_clicked(),
        interaction.body.triple_clicked(),
    ) {
        return;
    }

    if let Some(text) = panel.terminal().and_then(horizon_core::Terminal::selection_to_string) {
        primary_selection.copy(&text);
    }
}

fn selection_copy_completed(drag_stopped: bool, double_clicked: bool, triple_clicked: bool) -> bool {
    drag_stopped || double_clicked || triple_clicked
}

fn should_request_primary_paste(button: egui::PointerButton, pressed: bool, modifiers: egui::Modifiers) -> bool {
    cfg!(target_os = "linux")
        && button == egui::PointerButton::Middle
        && pressed
        && !modifiers.ctrl
        && !modifiers.command
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

fn pointer_event_targets_rect(events: &[egui::Event], from_global: Option<TSTransform>, rect: Rect) -> bool {
    events.iter().any(|event| match event {
        egui::Event::PointerButton { pos, .. } | egui::Event::PointerMoved(pos) => {
            rect.contains(transform_pos(from_global, *pos))
        }
        _ => false,
    })
}

fn pointer_button_event_pos(
    events: &[egui::Event],
    from_global: Option<TSTransform>,
    button: PointerButton,
    pressed: bool,
    rect: Rect,
) -> Option<Pos2> {
    events.iter().rev().find_map(|event| match event {
        egui::Event::PointerButton {
            pos,
            button: event_button,
            pressed: event_pressed,
            ..
        } if *event_button == button && *event_pressed == pressed => {
            let pos = transform_pos(from_global, *pos);
            rect.contains(pos).then_some(pos)
        }
        _ => None,
    })
}

fn response_pointer_pos(response: &egui::Response) -> Option<Pos2> {
    response.interact_pointer_pos().or_else(|| response.hover_pos())
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

pub(super) fn handle_terminal_keyboard_input(
    ui: &egui::Ui,
    terminal_id: egui::Id,
    panel: &mut Panel,
    events: &[TerminalInputEvent],
    primary_selection: &PrimarySelection,
    local_ssh_reconnect_enabled: bool,
) -> bool {
    if local_ssh_reconnect_enabled && disconnected_ssh_reconnect_requested(panel.kind, panel.ssh_status(), events) {
        return true;
    }

    let Some(terminal) = panel.terminal_mut() else {
        return false;
    };
    let mode = terminal.mode();
    let mut forwarder = KeyboardInputForwarder::default();
    let mut ime_enabled = terminal_ime_enabled(ui, terminal_id);
    let events = prepare_terminal_keyboard_events(events, ime_enabled);

    for event in &events {
        match &event.event {
            egui::Event::Ime(egui::ImeEvent::Enabled | egui::ImeEvent::Preedit(_)) => {
                ime_enabled = true;
            }
            egui::Event::Ime(egui::ImeEvent::Disabled) => {
                ime_enabled = false;
            }
            egui::Event::Text(text) | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                if matches!(&event.event, egui::Event::Ime(egui::ImeEvent::Commit(_))) {
                    ime_enabled = false;
                }
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
                if event.is_plain_ctrl_c_copy_command() {
                    terminal.write_input(&[3]);
                } else if let Some(text) = terminal.selection_to_string() {
                    primary_selection.copy(&text);
                    ui.ctx().copy_text(text);
                    terminal.clear_selection();
                }
            }
            egui::Event::Cut => {
                if let Some(text) = terminal.selection_to_string() {
                    primary_selection.copy(&text);
                    ui.ctx().copy_text(text);
                    terminal.clear_selection();
                }
                terminal.write_input(&[24]);
            }
            egui::Event::Key { .. } => {
                let emission = forwarder.on_key(event, mode);
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

    store_terminal_ime_enabled(ui, terminal_id, ime_enabled);

    false
}

fn disconnected_ssh_reconnect_requested(
    kind: PanelKind,
    ssh_status: Option<SshConnectionStatus>,
    events: &[TerminalInputEvent],
) -> bool {
    kind == PanelKind::Ssh
        && matches!(ssh_status, Some(SshConnectionStatus::Disconnected))
        && events.iter().any(|input_event| {
            matches!(
                &input_event.event,
                egui::Event::Key {
                    pressed: true,
                    repeat: false,
                    ..
                }
            ) && shortcut_event_matches(&input_event.event, SSH_RECONNECT_SHORTCUT)
        })
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

    fn on_key(&mut self, input_event: &TerminalInputEvent, mode: TermMode) -> InputEmission {
        let egui::Event::Key {
            key,
            physical_key,
            pressed,
            repeat,
            modifiers,
            ..
        } = &input_event.event
        else {
            return InputEmission::default();
        };

        let key_identity =
            input::KeyIdentity::new(*key, *physical_key, input_event.key_without_modifiers_text.as_deref());
        let context = input::KeyEventContext::new(*pressed, *repeat, *modifiers, mode);
        let mut emission = InputEmission::default();

        if let Some(deferred) = self.deferred_text_key.as_mut() {
            if let Some(actual_text) = deferred.synthetic_text.as_deref() {
                if !pressed && deferred.matches(*key, *physical_key) {
                    if let Some(translation) = input::translate_text_event(
                        input::KeyIdentity::new(*key, *physical_key, deferred.key_without_modifiers_text.as_deref()),
                        actual_text,
                        input::KeyEventContext::new(false, *repeat, *modifiers, mode),
                    ) {
                        emission.bytes.extend_from_slice(&translation.bytes);
                    }
                    self.deferred_text_key = None;
                    return emission;
                }

                if !deferred.matches(*key, *physical_key) {
                    self.deferred_text_key = None;
                }
            } else if !pressed && deferred.matches(*key, *physical_key) {
                deferred.release_seen = true;
                deferred.release_translation = input::translate_key_event_with_physical(
                    key_identity,
                    input::KeyEventContext::new(false, *repeat, *modifiers, mode),
                );
                return emission;
            } else if !deferred.matches(*key, *physical_key) {
                emission.bytes.extend_from_slice(&deferred.flush_fallback());
                self.deferred_text_key = None;
            }
        }

        if let Some(translation) = input::translate_key_event_with_physical(key_identity, context) {
            if *pressed
                && translation.suppress_text.is_some()
                && (modifiers.alt
                    || mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL)
                    || input::should_defer_textual_key(*key, *physical_key, *pressed, *modifiers, mode))
            {
                self.deferred_text_key = Some(DeferredTextKey::new(
                    *key,
                    *physical_key,
                    input_event.key_without_modifiers_text.as_deref(),
                    *modifiers,
                    Some(translation),
                ));
                return emission;
            }

            if let Some(text) = translation.suppress_text {
                self.suppressed_text.push_back(text);
            }
            emission.bytes.extend_from_slice(&translation.bytes);
            return emission;
        }

        if input::should_defer_textual_key(*key, *physical_key, *pressed, *modifiers, mode) {
            self.deferred_text_key = Some(DeferredTextKey::new(
                *key,
                *physical_key,
                input_event.key_without_modifiers_text.as_deref(),
                *modifiers,
                None,
            ));
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
    key_without_modifiers_text: Option<String>,
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
        key_without_modifiers_text: Option<&str>,
        modifiers: egui::Modifiers,
        press_translation: Option<input::KeyTranslation>,
    ) -> Self {
        Self {
            key,
            physical_key,
            key_without_modifiers_text: key_without_modifiers_text.map(ToOwned::to_owned),
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

        if let Some(translation) = input::translate_text_event(
            input::KeyIdentity::new(self.key, self.physical_key, self.key_without_modifiers_text.as_deref()),
            text,
            input::KeyEventContext::new(true, false, self.modifiers, mode),
        ) {
            let mut bytes = translation.bytes;
            if self.release_seen
                && let Some(release) = input::translate_text_event(
                    input::KeyIdentity::new(self.key, self.physical_key, self.key_without_modifiers_text.as_deref()),
                    text,
                    input::KeyEventContext::new(false, false, self.modifiers, mode),
                )
            {
                bytes.extend_from_slice(&release.bytes);
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
    use super::{
        KeyboardInputForwarder, TerminalInputEvent, disconnected_ssh_reconnect_requested, pointer_button_event_pos,
        pointer_event_targets_rect, selection_copy_completed, should_request_primary_paste,
    };
    use alacritty_terminal::term::TermMode;
    use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect};
    use horizon_core::{PanelKind, SshConnectionStatus};

    #[test]
    fn middle_click_requests_primary_paste_only_on_linux_without_ctrl_or_cmd() {
        assert_eq!(
            should_request_primary_paste(PointerButton::Middle, true, Modifiers::NONE),
            cfg!(target_os = "linux")
        );
    }

    #[test]
    fn middle_click_does_not_request_primary_paste_with_ctrl_or_cmd() {
        assert!(!should_request_primary_paste(
            PointerButton::Middle,
            true,
            Modifiers::CTRL
        ));
        assert!(!should_request_primary_paste(
            PointerButton::Middle,
            true,
            Modifiers::COMMAND
        ));
    }

    #[test]
    fn selection_completion_triggers_primary_copy() {
        assert!(selection_copy_completed(true, false, false));
        assert!(selection_copy_completed(false, true, false));
        assert!(selection_copy_completed(false, false, true));
        assert!(!selection_copy_completed(false, false, false));
    }

    #[test]
    fn pointer_button_event_uses_press_position_inside_rect() {
        let rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(20.0, 20.0));
        let events = vec![Event::PointerButton {
            pos: Pos2::new(12.0, 6.0),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }];

        assert_eq!(
            pointer_button_event_pos(&events, None, PointerButton::Primary, true, rect),
            Some(Pos2::new(12.0, 6.0))
        );
    }

    #[test]
    fn pointer_events_detect_positions_inside_rect() {
        let rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(20.0, 20.0));
        let events = vec![Event::PointerMoved(Pos2::new(12.0, 6.0))];

        assert!(pointer_event_targets_rect(&events, None, rect));
    }

    #[test]
    fn disconnected_ssh_panels_request_reconnect_from_local_shortcut() {
        assert!(disconnected_ssh_reconnect_requested(
            PanelKind::Ssh,
            Some(SshConnectionStatus::Disconnected),
            &[key_event(
                Key::R,
                Some(Key::R),
                None,
                true,
                false,
                Modifiers::COMMAND | Modifiers::SHIFT,
            )],
        ));
    }

    #[test]
    fn connected_ssh_panels_ignore_local_reconnect_shortcut() {
        assert!(!disconnected_ssh_reconnect_requested(
            PanelKind::Ssh,
            Some(SshConnectionStatus::Connected),
            &[key_event(
                Key::R,
                Some(Key::R),
                None,
                true,
                false,
                Modifiers::COMMAND | Modifiers::SHIFT,
            )],
        ));
    }

    #[test]
    fn non_ssh_panels_ignore_local_reconnect_shortcut() {
        assert!(!disconnected_ssh_reconnect_requested(
            PanelKind::Shell,
            None,
            &[key_event(
                Key::R,
                Some(Key::R),
                None,
                true,
                false,
                Modifiers::COMMAND | Modifiers::SHIFT,
            )],
        ));
    }

    #[test]
    fn repeated_reconnect_shortcut_does_not_queue_another_restart() {
        assert!(!disconnected_ssh_reconnect_requested(
            PanelKind::Ssh,
            Some(SshConnectionStatus::Disconnected),
            &[key_event(
                Key::R,
                Some(Key::R),
                None,
                true,
                true,
                Modifiers::COMMAND | Modifiers::SHIFT,
            )],
        ));
    }

    #[test]
    fn altgr_text_after_release_stays_on_text_path_without_report_all_keys() {
        let events = vec![
            key_event(Key::Num2, Some(Key::Num2), Some("2"), true, false, Modifiers::ALT),
            key_event(Key::Num2, Some(Key::Num2), Some("2"), false, false, Modifiers::ALT),
            text_event("@"),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        assert_eq!(bytes, b"@");
    }

    #[test]
    fn shifted_symbol_uses_text_reconciliation_without_forcing_kitty_sequences() {
        let events = vec![
            key_event(Key::Num2, Some(Key::Num2), Some("2"), true, false, Modifiers::SHIFT),
            text_event("@"),
            key_event(Key::Num2, Some(Key::Num2), Some("2"), false, false, Modifiers::SHIFT),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        assert_eq!(bytes, b"@");
    }

    #[test]
    fn plain_space_stays_on_text_path_in_kitty_basic_mode() {
        let events = vec![
            key_event(Key::Space, Some(Key::Space), Some(" "), true, false, Modifiers::NONE),
            text_event(" "),
            key_event(Key::Space, Some(Key::Space), Some(" "), false, false, Modifiers::NONE),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        assert_eq!(bytes, b" ");
    }

    #[test]
    fn repeated_spaces_do_not_get_dropped_in_kitty_basic_mode() {
        let events = vec![
            key_event(Key::Space, Some(Key::Space), Some(" "), true, false, Modifiers::NONE),
            text_event(" "),
            key_event(Key::Space, Some(Key::Space), Some(" "), false, false, Modifiers::NONE),
            key_event(Key::Space, Some(Key::Space), Some(" "), true, false, Modifiers::NONE),
            text_event(" "),
            key_event(Key::Space, Some(Key::Space), Some(" "), false, false, Modifiers::NONE),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        assert_eq!(bytes, b"  ");
    }

    #[test]
    fn shifted_space_stays_on_text_path_in_kitty_basic_mode() {
        let events = vec![
            key_event(Key::Space, Some(Key::Space), Some(" "), true, false, Modifiers::SHIFT),
            text_event(" "),
            key_event(Key::Space, Some(Key::Space), Some(" "), false, false, Modifiers::SHIFT),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        assert_eq!(bytes, b" ");
    }

    /// Regression: on some Linux setups, `AltGr` is NOT reported as
    /// `modifiers.alt` by winit. The key event must not leak the base
    /// key ("2") ahead of the later text event ("@"), even when kitty
    /// keyboard mode is active.
    #[test]
    fn altgr_without_alt_modifier_in_kitty_mode_does_not_leak_base_key() {
        let events = vec![
            key_event(Key::Num2, Some(Key::Num2), Some("2"), true, false, Modifiers::NONE),
            text_event("@"),
            key_event(Key::Num2, Some(Key::Num2), Some("2"), false, false, Modifiers::NONE),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        assert_eq!(bytes, b"@");
    }

    /// Same scenario as above but in non-kitty mode: the text event
    /// should pass through as raw "@" with no preceding "2".
    #[test]
    fn altgr_without_alt_modifier_in_legacy_mode_emits_only_text() {
        let events = vec![
            key_event(Key::Num2, Some(Key::Num2), Some("2"), true, false, Modifiers::NONE),
            text_event("@"),
            key_event(Key::Num2, Some(Key::Num2), Some("2"), false, false, Modifiers::NONE),
        ];

        let bytes = forward_bytes(&events, TermMode::NONE);

        assert_eq!(bytes, b"@");
    }

    #[test]
    fn shifted_international_key_stays_on_text_path_without_report_all_keys() {
        let events = vec![
            key_event(
                Key::OpenBracket,
                Some(Key::OpenBracket),
                Some("å"),
                true,
                false,
                Modifiers::SHIFT,
            ),
            text_event("Å"),
            key_event(
                Key::OpenBracket,
                Some(Key::OpenBracket),
                Some("å"),
                false,
                false,
                Modifiers::SHIFT,
            ),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        assert_eq!(bytes, "Å".as_bytes());
    }

    #[test]
    fn report_all_keys_keeps_printable_text_on_kitty_sequence_path() {
        let events = vec![
            key_event(
                Key::OpenBracket,
                Some(Key::OpenBracket),
                Some("å"),
                true,
                false,
                Modifiers::SHIFT,
            ),
            text_event("Å"),
            key_event(
                Key::OpenBracket,
                Some(Key::OpenBracket),
                Some("å"),
                false,
                false,
                Modifiers::SHIFT,
            ),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES
                | TermMode::REPORT_EVENT_TYPES
                | TermMode::REPORT_ALTERNATE_KEYS
                | TermMode::REPORT_ALL_KEYS_AS_ESC,
        );

        assert_eq!(bytes, b"\x1b[229:197:91;2u\x1b[229:197:91;2:3u");
    }

    #[test]
    fn legacy_c0_key_events_are_forwarded_in_legacy_mode() {
        let cases: [(&str, TerminalInputEvent, &[u8]); 6] = [
            (
                "shift enter",
                key_event(Key::Enter, Some(Key::Enter), None, true, false, Modifiers::SHIFT),
                b"\r",
            ),
            (
                "alt escape",
                key_event(Key::Escape, Some(Key::Escape), None, true, false, Modifiers::ALT),
                b"\x1b\x1b",
            ),
            (
                "ctrl backspace",
                key_event(Key::Backspace, Some(Key::Backspace), None, true, false, Modifiers::CTRL),
                b"\x08",
            ),
            (
                "alt backspace",
                key_event(Key::Backspace, Some(Key::Backspace), None, true, false, Modifiers::ALT),
                b"\x1b\x7f",
            ),
            (
                "ctrl shift tab",
                key_event(
                    Key::Tab,
                    Some(Key::Tab),
                    None,
                    true,
                    false,
                    Modifiers::CTRL | Modifiers::SHIFT,
                ),
                b"\x1b[Z",
            ),
            (
                "alt shift tab",
                key_event(
                    Key::Tab,
                    Some(Key::Tab),
                    None,
                    true,
                    false,
                    Modifiers::ALT | Modifiers::SHIFT,
                ),
                b"\x1b\x1b[Z",
            ),
        ];

        for (name, event, expected) in cases {
            let bytes = forward_bytes(&[event], TermMode::NONE);
            assert_eq!(bytes, expected, "{name}");
        }
    }

    fn forward_bytes(events: &[TerminalInputEvent], mode: TermMode) -> Vec<u8> {
        let mut forwarder = KeyboardInputForwarder::default();
        let mut bytes = Vec::new();

        for event in events {
            let emission = match &event.event {
                Event::Text(text) | Event::Ime(egui::ImeEvent::Commit(text)) => forwarder.on_text(text, mode),
                Event::Key { .. } => forwarder.on_key(event, mode),
                _ => continue,
            };
            bytes.extend_from_slice(&emission.bytes);
        }

        bytes.extend_from_slice(&forwarder.finish().bytes);
        bytes
    }

    fn key_event(
        key: Key,
        physical_key: Option<Key>,
        key_without_modifiers_text: Option<&str>,
        pressed: bool,
        repeat: bool,
        modifiers: Modifiers,
    ) -> TerminalInputEvent {
        TerminalInputEvent {
            event: Event::Key {
                key,
                physical_key,
                pressed,
                repeat,
                modifiers,
            },
            key_without_modifiers_text: key_without_modifiers_text.map(ToOwned::to_owned),
            observed_key: None,
        }
    }

    fn text_event(text: &str) -> TerminalInputEvent {
        TerminalInputEvent {
            event: Event::Text(text.to_owned()),
            key_without_modifiers_text: None,
            observed_key: None,
        }
    }
}
