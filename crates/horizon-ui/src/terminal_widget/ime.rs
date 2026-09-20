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
            purpose: egui::IMEPurpose::Terminal,
            rect: to_global * body_rect,
            cursor_rect: to_global * cursor_rect,
            should_interrupt_composition: false,
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

    // Only a live composition may swallow these keys, so track it per event
    // instead of treating the whole frame as composing. An IME event in the
    // frame is not evidence of one: on Wayland every text-input round trip
    // ends with a bare `Preedit("")` (winit's `Done` handler), which means
    // "nothing is composing" and used to take the frame's Backspace with it.
    // A trailing empty preedit cannot retroactively release the frame's keys:
    // reaching this point with a latched composition means this frame carries
    // its genuine end, so the Backspace that deleted the last preedit character
    // is the IME's, not the terminal's. A dismissal already reported in an
    // earlier frame has cleared the latch, so it never reaches this branch.
    let mut composing = ime_enabled;
    let mut filtered = Vec::with_capacity(events.len());
    for event in events {
        match &event.event {
            egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                // Composition state only; the text itself is the user's input.
                tracing::trace!(latched = ime_enabled, live = !text.is_empty(), "terminal IME preedit");
                composing = !text.is_empty();
            }
            egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                tracing::trace!(
                    latched = ime_enabled,
                    chars = text.chars().count(),
                    "terminal IME commit"
                );
                composing = false;
            }
            _ if composing && is_ime_incompatible_event(&event.event) => {
                // The one place a terminal panel drops a key it was given. A
                // session that loses navigation without a visible composition
                // is diagnosed from here.
                tracing::debug!(withheld = ?event.event, latched = ime_enabled, "terminal key held by a live IME composition");
                continue;
            }
            _ => {}
        }
        filtered.push(event.clone());
    }
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
            terminal_event(Event::Key {
                key: Key::B,
                physical_key: None,
                pressed: true,
                repeat: true,
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

    fn key(key: Key, repeat: bool) -> TerminalInputEvent {
        terminal_event(Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers: Modifiers::NONE,
        })
    }

    fn commit(text: &str) -> TerminalInputEvent {
        terminal_event(Event::Ime(egui::ImeEvent::Commit(text.to_owned())))
    }

    fn preedit(text: &str) -> TerminalInputEvent {
        terminal_event(Event::Ime(egui::ImeEvent::Preedit {
            text: text.to_owned(),
            active_range_chars: None,
        }))
    }

    /// Only a live composition may swallow Backspace. Wayland ends every
    /// text-input round trip with a bare `Preedit("")` meaning "not
    /// composing", which must not take the frame's Backspace with it.
    #[test]
    fn backspace_survives_every_frame_without_a_live_composition() {
        let bs = || key(Key::Backspace, false);
        let commit = terminal_event(Event::Ime(egui::ImeEvent::Commit("æ".to_owned())));
        let kept = |ime_enabled, events: Vec<TerminalInputEvent>| {
            prepare_terminal_keyboard_events(&events, ime_enabled)
                .iter()
                .any(|event| {
                    matches!(
                        event.event,
                        Event::Key {
                            key: Key::Backspace,
                            ..
                        }
                    )
                })
        };

        assert!(kept(false, vec![preedit(""), bs()]), "idle empty preedit");
        assert!(
            kept(true, vec![preedit(""), bs()]),
            "empty preedit ends a latched composition"
        );
        assert!(kept(true, vec![commit, bs()]), "commit releases later keys");
        assert!(
            kept(false, vec![preedit(""), key(Key::Backspace, true)]),
            "held backspace"
        );
        assert!(
            !kept(false, vec![preedit("中"), bs()]),
            "live composition still filters"
        );
        assert!(
            kept(false, vec![bs(), preedit("中")]),
            "key before a composition starts"
        );
    }

    /// A composition spans frames, and a key can be delivered before the
    /// compositor sends the preedit or commit that answers it, so an eventless
    /// frame is not evidence that the composition ended: the latch still owns
    /// these keys until an ending signal or a blur retires it.
    #[test]
    fn a_cross_frame_composition_keeps_its_keys_through_an_eventless_frame() {
        let events = vec![
            key(Key::ArrowDown, false),
            key(Key::ArrowUp, true),
            key(Key::Backspace, false),
        ];

        assert!(
            prepare_terminal_keyboard_events(&events, true).is_empty(),
            "a live composition keeps the keys the IME has yet to answer"
        );
        assert_eq!(
            prepare_terminal_keyboard_events(&events, false),
            events,
            "without a composition the same frame reaches the terminal"
        );
    }

    /// Backspace that deletes the last preedit character dismisses the
    /// composition, so the frame is `[Backspace, Preedit("")]` and the key
    /// belongs to the IME. The trailing empty preedit cannot hand it to the
    /// terminal: a dismissal already reported in an earlier frame would have
    /// cleared the latch before this one.
    #[test]
    fn a_composition_dismissed_by_its_last_character_keeps_that_key() {
        for trailing in [vec![preedit("")], vec![preedit(""), commit("")]] {
            let mut events = vec![key(Key::Backspace, false), key(Key::ArrowDown, false)];
            events.extend(trailing);

            let filtered = prepare_terminal_keyboard_events(&events, true);

            assert!(
                filtered.iter().all(|event| !matches!(event.event, Event::Key { .. })),
                "the composition keeps the keys that dismissed it"
            );
        }
    }

    /// A frame that also carries a live candidate or committed text is not a
    /// frame that says nothing was composing: the keys the IME answered stay
    /// filtered even though an empty preedit passed through with them.
    #[test]
    fn a_frame_that_also_carries_a_composition_keeps_the_latch() {
        for trailing in [
            vec![preedit(""), preedit("中")],
            vec![preedit("中"), preedit("")],
            vec![preedit(""), commit("中")],
        ] {
            let mut events = vec![key(Key::ArrowDown, false), key(Key::Backspace, true)];
            events.extend(trailing);

            let filtered = prepare_terminal_keyboard_events(&events, true);

            assert!(
                filtered.iter().all(|event| !matches!(
                    event.event,
                    Event::Key {
                        key: Key::ArrowDown,
                        ..
                    } | Event::Key { repeat: true, .. }
                )),
                "the composition keeps the keys it answered"
            );
        }
    }

    /// A composition that is still live owns these keys: a commit proves one
    /// produced text, and a fresh preedit proves one is still on screen.
    #[test]
    fn a_live_composition_still_swallows_arrows_and_repeats() {
        for events in [
            vec![key(Key::ArrowDown, false), key(Key::ArrowUp, true), commit("中")],
            vec![key(Key::ArrowDown, false), key(Key::ArrowUp, true), preedit("中")],
        ] {
            let filtered = prepare_terminal_keyboard_events(&events, true);

            assert!(
                filtered.iter().all(|event| !matches!(
                    event.event,
                    Event::Key {
                        key: Key::ArrowDown | Key::ArrowUp,
                        ..
                    }
                )),
                "a live composition keeps the keys it consumed"
            );
        }
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
