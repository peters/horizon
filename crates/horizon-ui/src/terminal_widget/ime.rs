use alacritty_terminal::term::point_to_viewport;
use alacritty_terminal::vte::ansi::CursorShape;
use egui::{Id, Pos2, Rect, Vec2};
use horizon_core::Panel;

use crate::input::TerminalInputEvent;

use super::layout::{GridMetrics, TerminalInteraction, usize_to_f32};

#[derive(Clone, Copy, Default)]
struct TerminalImeState {
    enabled: bool,
}

pub(super) fn publish_terminal_ime_output(
    ui: &egui::Ui,
    panel: &Panel,
    interaction: &TerminalInteraction,
    metrics: &GridMetrics,
) {
    let to_global = ui.ctx().layer_transform_to_global(ui.layer_id()).unwrap_or_default();
    let body_rect = interaction.layout.body;
    let cursor_rect = terminal_cursor_rect(panel, body_rect, metrics).unwrap_or(body_rect);

    ui.ctx().output_mut(|output| {
        output.ime = Some(egui::output::IMEOutput {
            rect: to_global * body_rect,
            cursor_rect: to_global * cursor_rect,
        });
    });
}

pub(super) fn clear_terminal_ime_state(ui: &egui::Ui, terminal_id: Id) {
    ui.data_mut(|data| {
        data.remove_temp::<TerminalImeState>(terminal_id);
    });
}

pub(super) fn terminal_ime_enabled(ui: &egui::Ui, terminal_id: Id) -> bool {
    ui.data(|data| data.get_temp::<TerminalImeState>(terminal_id))
        .unwrap_or_default()
        .enabled
}

pub(super) fn store_terminal_ime_enabled(ui: &egui::Ui, terminal_id: Id, enabled: bool) {
    ui.data_mut(|data| {
        if enabled {
            data.insert_temp(terminal_id, TerminalImeState { enabled });
        } else {
            data.remove_temp::<TerminalImeState>(terminal_id);
        }
    });
}

pub(super) fn prepare_terminal_keyboard_events(
    events: &[TerminalInputEvent],
    ime_enabled: bool,
) -> Vec<TerminalInputEvent> {
    let ime_events_present = events.iter().any(|event| matches!(event.event, egui::Event::Ime(_)));
    if !ime_enabled && !ime_events_present {
        return events.to_vec();
    }

    let mut filtered = events.to_vec();
    filtered.retain(|event| !is_ime_incompatible_event(&event.event));
    filtered.sort_by_key(|event| !matches!(event.event, egui::Event::Ime(_)));
    filtered
}

fn is_ime_incompatible_event(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::Key { repeat: true, .. }
            | egui::Event::Key {
                key: egui::Key::Backspace
                    | egui::Key::ArrowUp
                    | egui::Key::ArrowDown
                    | egui::Key::ArrowLeft
                    | egui::Key::ArrowRight,
                ..
            }
    )
}

fn terminal_cursor_rect(panel: &Panel, body_rect: Rect, metrics: &GridMetrics) -> Option<Rect> {
    let terminal = panel.terminal()?;
    terminal.with_renderable_content(|content| {
        let point = point_to_viewport(content.display_offset, content.cursor.point)?;
        Some(cursor_rect_for_viewport_point(
            body_rect,
            metrics,
            point.line,
            point.column.0,
            content.cursor.shape,
        ))
    })
}

fn cursor_rect_for_viewport_point(
    body_rect: Rect,
    metrics: &GridMetrics,
    line: usize,
    column: usize,
    shape: CursorShape,
) -> Rect {
    let min = Pos2::new(
        body_rect.min.x + usize_to_f32(column) * metrics.char_width,
        body_rect.min.y + usize_to_f32(line) * metrics.line_height,
    );

    match shape {
        CursorShape::Underline => Rect::from_min_size(
            Pos2::new(min.x, min.y + metrics.line_height - 2.0),
            Vec2::new(metrics.char_width, 2.0),
        ),
        CursorShape::Beam => Rect::from_min_size(min, Vec2::new(2.0, metrics.line_height)),
        CursorShape::Block | CursorShape::HollowBlock | CursorShape::Hidden => {
            Rect::from_min_size(min, Vec2::new(metrics.char_width, metrics.line_height))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{cursor_rect_for_viewport_point, prepare_terminal_keyboard_events};
    use crate::input::TerminalInputEvent;
    use crate::terminal_widget::layout::GridMetrics;
    use alacritty_terminal::vte::ansi::CursorShape;
    use egui::{Event, FontId, Key, Modifiers, Rect, pos2, vec2};

    fn metrics() -> GridMetrics {
        GridMetrics {
            char_width: 8.0,
            line_height: 16.0,
            font_id: FontId::monospace(13.0),
        }
    }

    fn terminal_event(event: Event) -> TerminalInputEvent {
        TerminalInputEvent {
            event,
            key_without_modifiers_text: None,
            observed_key: None,
        }
    }

    #[test]
    fn prepare_terminal_keyboard_events_filters_ime_incompatible_keys() {
        let events = vec![
            terminal_event(Event::Key {
                key: Key::ArrowLeft,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }),
            terminal_event(Event::Ime(egui::ImeEvent::Commit("中".to_owned()))),
            terminal_event(Event::Key {
                key: Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }),
            terminal_event(Event::Key {
                key: Key::B,
                physical_key: None,
                pressed: true,
                repeat: true,
                modifiers: Modifiers::NONE,
            }),
        ];

        let filtered = prepare_terminal_keyboard_events(&events, true);

        assert!(matches!(
            filtered.first(),
            Some(TerminalInputEvent {
                event: Event::Ime(egui::ImeEvent::Commit(text)),
                ..
            }) if text == "中"
        ));
        assert!(filtered.iter().all(|event| !matches!(
            event.event,
            Event::Key {
                key: Key::ArrowLeft,
                ..
            } | Event::Key { repeat: true, .. }
        )));
        assert!(
            filtered
                .iter()
                .any(|event| matches!(event.event, Event::Key { key: Key::A, .. }))
        );
    }

    #[test]
    fn prepare_terminal_keyboard_events_leaves_regular_input_untouched() {
        let events = vec![
            terminal_event(Event::Text("a".to_owned())),
            terminal_event(Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }),
        ];

        assert_eq!(prepare_terminal_keyboard_events(&events, false), events);
    }

    #[test]
    fn cursor_rect_tracks_viewport_cell_position() {
        let rect = cursor_rect_for_viewport_point(
            Rect::from_min_size(pos2(10.0, 20.0), vec2(320.0, 240.0)),
            &metrics(),
            2,
            3,
            CursorShape::Block,
        );

        assert_eq!(rect.min, pos2(34.0, 52.0));
        assert_eq!(rect.size(), vec2(8.0, 16.0));
    }
}
