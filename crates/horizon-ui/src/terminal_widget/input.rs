use std::collections::VecDeque;

use alacritty_terminal::term::TermMode;
use egui::{Key, Pos2, Rect, Vec2};
use horizon_core::{Panel, SelectionType, TerminalSide};

use crate::input;

use super::layout::{GridMetrics, TerminalInteraction, cell_side, grid_point_from_position};
use super::scrollbar::{scrollbar_pointer_to_scrollback, scrollbar_thumb_height};

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

    let Some(terminal_mode) = panel.terminal_mut().map(|terminal| terminal.mode()) else {
        return;
    };
    let events: Vec<egui::Event> = ui.input(|input| input.events.clone());
    let pointer_buttons = ui.input(|input| input::PointerButtons {
        primary: input.pointer.primary_down(),
        middle: input.pointer.middle_down(),
        secondary: input.pointer.secondary_down(),
    });
    let current_modifiers = ui.input(|input| input.modifiers);
    let hovered_point = ui
        .input(|input| input.pointer.hover_pos())
        .filter(|position| interaction.layout.body.contains(*position))
        .and_then(|position| {
            grid_point_from_position(interaction.layout.body, position, metrics, visible_rows, visible_cols)
        });

    let mouse_mode_active = |modifiers: &egui::Modifiers| -> bool {
        !modifiers.shift && terminal_mode.intersects(alacritty_terminal::term::TermMode::MOUSE_MODE)
    };

    handle_pointer_events(
        &events,
        panel,
        interaction,
        metrics,
        visible_rows,
        visible_cols,
        terminal_mode,
        pointer_buttons,
        current_modifiers,
        hovered_point,
        &mouse_mode_active,
    );

    handle_scrollbar_drag(ui, panel, interaction, visible_rows);
}

#[allow(clippy::too_many_arguments)]
fn handle_pointer_events(
    events: &[egui::Event],
    panel: &mut Panel,
    interaction: &TerminalInteraction,
    metrics: &GridMetrics,
    visible_rows: u16,
    visible_cols: u16,
    terminal_mode: TermMode,
    pointer_buttons: input::PointerButtons,
    current_modifiers: egui::Modifiers,
    hovered_point: Option<input::GridPoint>,
    mouse_mode_active: &dyn Fn(&egui::Modifiers) -> bool,
) {
    for event in events {
        match event {
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers,
            } if interaction.layout.body.contains(*pos) => {
                if *pressed {
                    interaction.body.request_focus();
                }
                handle_pointer_button(
                    panel,
                    interaction,
                    *pos,
                    *button,
                    *pressed,
                    *modifiers,
                    metrics,
                    visible_rows,
                    visible_cols,
                    terminal_mode,
                    mouse_mode_active,
                );
            }
            egui::Event::PointerMoved(pos) => {
                let inside = interaction.layout.body.contains(*pos);
                if inside && mouse_mode_active(&current_modifiers) {
                    if let Some(point) =
                        grid_point_from_position(interaction.layout.body, *pos, metrics, visible_rows, visible_cols)
                        && let Some(bytes) =
                            input::mouse_motion_report(pointer_buttons, current_modifiers, terminal_mode, point)
                        && !bytes.is_empty()
                    {
                        panel.write_input(&bytes);
                    }
                } else if interaction.body.dragged() && panel.terminal_mut().is_some_and(|t| t.has_selection()) {
                    handle_pointer_selection_drag(
                        panel,
                        *pos,
                        interaction.layout.body,
                        metrics,
                        visible_rows,
                        visible_cols,
                    );
                }
            }
            egui::Event::MouseWheel { delta, unit, modifiers } => {
                if let Some(point) = hovered_point
                    && let Some(action) = input::wheel_action(
                        *delta,
                        *unit,
                        Vec2::new(metrics.char_width, metrics.line_height),
                        *modifiers,
                        terminal_mode,
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

#[allow(clippy::too_many_arguments)]
fn handle_pointer_button(
    panel: &mut Panel,
    interaction: &TerminalInteraction,
    pos: Pos2,
    button: egui::PointerButton,
    pressed: bool,
    modifiers: egui::Modifiers,
    metrics: &GridMetrics,
    visible_rows: u16,
    visible_cols: u16,
    terminal_mode: TermMode,
    mouse_mode_active: &dyn Fn(&egui::Modifiers) -> bool,
) {
    if mouse_mode_active(&modifiers) {
        if let Some(point) = grid_point_from_position(interaction.layout.body, pos, metrics, visible_rows, visible_cols)
            && let Some(bytes) = input::mouse_button_report(button, pressed, modifiers, terminal_mode, point)
            && !bytes.is_empty()
        {
            panel.write_input(&bytes);
        }
    } else if button == egui::PointerButton::Primary
        && pressed
        && let Some(point) = grid_point_from_position(interaction.layout.body, pos, metrics, visible_rows, visible_cols)
    {
        let sel_type = if interaction.body.triple_clicked() {
            SelectionType::Lines
        } else if interaction.body.double_clicked() {
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
    if (interaction.scrollbar.dragged() || interaction.scrollbar.clicked())
        && let Some(pointer_position) = ui.input(|input| input.pointer.interact_pos())
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
    let mut suppressed_text = VecDeque::new();

    // Alt-modified key translations are deferred until the next Text event
    // so we can distinguish true Alt (e.g. Alt+b → ESC+"b") from AltGr
    // (e.g. AltGr+2 → "@").  winit reports AltGr as Alt, so we compare
    // the suppress_text against the actual Text event to decide.
    let mut deferred_alt: Option<(Vec<u8>, String)> = None;

    for event in &events {
        match event {
            egui::Event::Text(text) | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                if let Some((alt_bytes, expected)) = deferred_alt.take() {
                    if expected == *text {
                        // True Alt key — send the alt-prefixed bytes, suppress text.
                        terminal.write_input(&alt_bytes);
                    } else {
                        // AltGr — discard alt bytes, send the actual character.
                        terminal.clear_selection();
                        terminal.write_input(text.as_bytes());
                    }
                } else if suppressed_text.front().is_some_and(|expected| expected == text) {
                    suppressed_text.pop_front();
                } else {
                    terminal.clear_selection();
                    terminal.write_input(text.as_bytes());
                }
            }
            egui::Event::Paste(text) => {
                terminal.clear_selection();
                let bytes = input::paste_bytes(text, mode, true);
                terminal.write_input(&bytes);
            }
            egui::Event::Copy => {
                if let Some(text) = terminal.selection_to_string() {
                    ui.ctx().copy_text(text);
                    terminal.clear_selection();
                } else {
                    terminal.write_input(&[3]);
                }
            }
            egui::Event::Cut => {
                if let Some(text) = terminal.selection_to_string() {
                    ui.ctx().copy_text(text);
                    terminal.clear_selection();
                }
                terminal.write_input(&[24]);
            }
            egui::Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } => {
                // Flush any pending deferred alt (a Key event arrived before
                // the expected Text event — treat as true Alt).
                if let Some((alt_bytes, expected)) = deferred_alt.take() {
                    terminal.write_input(&alt_bytes);
                    suppressed_text.push_back(expected);
                }

                if let Some(translation) = input::translate_key_event(*key, *pressed, *repeat, *modifiers, mode) {
                    if let Some(text) = translation.suppress_text {
                        if modifiers.alt && !translation.bytes.is_empty() {
                            // Defer: wait for the Text event to confirm true Alt vs AltGr.
                            deferred_alt = Some((translation.bytes, text));
                            continue;
                        }
                        suppressed_text.push_back(text);
                    }
                    if !translation.bytes.is_empty() {
                        terminal.write_input(&translation.bytes);
                    }
                }
            }
            _ => {}
        }
    }

    // Flush any remaining deferred alt (no Text event followed — true Alt
    // of a non-printable key, or platform didn't generate text).
    if let Some((alt_bytes, _)) = deferred_alt {
        terminal.write_input(&alt_bytes);
    }
}
