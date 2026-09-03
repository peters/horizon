use alacritty_terminal::term::TermMode;
use egui::emath::TSTransform;
use egui::{PointerButton, Pos2, Rect};

use super::routing::{
    pointer_button_checks_clickable_target, pointer_button_opens_osc8_hyperlink, pointer_button_starts_local_selection,
};

pub(super) struct PointerFrameEvents {
    pub(super) body_primary_press_pos: Option<Pos2>,
    pub(super) osc8_replay_index: Option<usize>,
    pub(super) primary_release_pos: Option<Pos2>,
    pub(super) primary_release_index: Option<usize>,
    pub(super) body_middle_press_pos: Option<Pos2>,
}

impl PointerFrameEvents {
    pub(super) fn collect(
        events: &[egui::Event],
        from_global: Option<TSTransform>,
        body_rect: Rect,
        terminal_mode: TermMode,
    ) -> Self {
        let body_primary_press = events.iter().enumerate().rev().find_map(|(index, event)| {
            body_local_selection_press(event, from_global, body_rect, terminal_mode)
                .map(|(pos, modifiers)| (index, pos, modifiers))
        });
        let release_search_start = body_primary_press.map_or(0, |(index, _, _)| index.saturating_add(1));
        let primary_release =
            events
                .iter()
                .enumerate()
                .skip(release_search_start)
                .find_map(|(index, event)| match event {
                    egui::Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed: false,
                        ..
                    } => Some((index, transform_pos(from_global, *pos))),
                    _ => None,
                });
        let osc8_replay_index = body_primary_press.and_then(|(index, _, modifiers)| {
            pointer_button_opens_osc8_hyperlink(PointerButton::Primary, modifiers).then_some(index)
        });

        Self {
            body_primary_press_pos: body_primary_press.map(|(_, pos, _)| pos),
            osc8_replay_index,
            primary_release_pos: primary_release.map(|(_, pos)| pos),
            primary_release_index: primary_release.map(|(index, _)| index),
            body_middle_press_pos: pointer_button_event_pos(
                events,
                from_global,
                PointerButton::Middle,
                true,
                body_rect,
            ),
        }
    }
}

pub(super) struct LocalSelectionEventTracker {
    primary_owned: bool,
}

impl LocalSelectionEventTracker {
    pub(super) fn new(primary_owned: bool) -> Self {
        Self { primary_owned }
    }

    pub(super) fn claims(
        &mut self,
        event: &egui::Event,
        from_global: Option<TSTransform>,
        body_rect: Rect,
        terminal_mode: TermMode,
    ) -> bool {
        match event {
            egui::Event::PointerButton {
                button: PointerButton::Primary,
                pressed: true,
                ..
            } if body_local_selection_press(event, from_global, body_rect, terminal_mode).is_some() => {
                self.primary_owned = true;
                true
            }
            egui::Event::PointerButton {
                button: PointerButton::Primary,
                pressed: false,
                ..
            } if self.primary_owned => {
                self.primary_owned = false;
                true
            }
            egui::Event::PointerMoved(_) => self.primary_owned,
            _ => false,
        }
    }
}

fn body_local_selection_press(
    event: &egui::Event,
    from_global: Option<TSTransform>,
    body_rect: Rect,
    terminal_mode: TermMode,
) -> Option<(Pos2, egui::Modifiers)> {
    let egui::Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed: true,
        modifiers,
    } = event
    else {
        return None;
    };
    let pos = transform_pos(from_global, *pos);
    (body_rect.contains(pos)
        && !pointer_button_checks_clickable_target(PointerButton::Primary, true, *modifiers)
        && pointer_button_starts_local_selection(terminal_mode, PointerButton::Primary, true, *modifiers))
    .then_some((pos, *modifiers))
}

pub(super) fn transform_pos(from_global: Option<TSTransform>, pos: Pos2) -> Pos2 {
    from_global.map_or(pos, |transform| transform * pos)
}

pub(super) fn pointer_event_targets_rect(events: &[egui::Event], from_global: Option<TSTransform>, rect: Rect) -> bool {
    events.iter().any(|event| match event {
        egui::Event::PointerButton { pos, .. } | egui::Event::PointerMoved(pos) => {
            rect.contains(transform_pos(from_global, *pos))
        }
        _ => false,
    })
}

pub(super) fn pointer_button_event_pos(
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

#[cfg(test)]
pub(super) fn pointer_button_event_any_pos(
    events: &[egui::Event],
    from_global: Option<TSTransform>,
    button: PointerButton,
    pressed: bool,
) -> Option<Pos2> {
    events.iter().rev().find_map(|event| match event {
        egui::Event::PointerButton {
            pos,
            button: event_button,
            pressed: event_pressed,
            ..
        } if *event_button == button && *event_pressed == pressed => Some(transform_pos(from_global, *pos)),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::{LocalSelectionEventTracker, PointerFrameEvents, pointer_button_event_pos, pointer_event_targets_rect};
    use alacritty_terminal::term::TermMode;
    use egui::{Event, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect, Vec2};

    fn button_event(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn wheel_event() -> Event {
        Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: Vec2::Y,
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        }
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
    fn pointer_events_ignore_positions_outside_rect() {
        let rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(20.0, 20.0));
        let events = vec![
            Event::PointerMoved(Pos2::new(40.0, 6.0)),
            Event::PointerButton {
                pos: Pos2::new(40.0, 6.0),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ];

        assert!(!pointer_event_targets_rect(&events, None, rect));
        assert_eq!(
            PointerFrameEvents::collect(&events, None, rect, TermMode::NONE).body_primary_press_pos,
            None
        );
    }

    #[test]
    fn osc8_lookup_replays_at_the_press_position() {
        let inside = Pos2::new(4.0, 4.0);
        let rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(20.0, 20.0));

        let wheel_then_press =
            PointerFrameEvents::collect(&[wheel_event(), button_event(inside, true)], None, rect, TermMode::NONE);
        let press_then_wheel =
            PointerFrameEvents::collect(&[button_event(inside, true), wheel_event()], None, rect, TermMode::NONE);

        assert_eq!(wheel_then_press.osc8_replay_index, Some(1));
        assert_eq!(press_then_wheel.osc8_replay_index, Some(0));
    }

    #[test]
    fn release_after_last_local_press_completes_that_press() {
        let inside = Pos2::new(4.0, 4.0);
        let outside = Pos2::new(40.0, 40.0);
        let events = vec![
            button_event(inside, true),
            button_event(inside, false),
            button_event(outside, true),
        ];
        let frame_events = PointerFrameEvents::collect(
            &events,
            None,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(20.0, 20.0)),
            TermMode::NONE,
        );

        assert_eq!(frame_events.body_primary_press_pos, Some(inside));
        assert_eq!(frame_events.primary_release_pos, Some(inside));
        assert_eq!(frame_events.primary_release_index, Some(1));
    }

    #[test]
    fn release_before_last_local_press_does_not_complete_restarted_drag() {
        let inside = Pos2::new(4.0, 4.0);
        let events = vec![button_event(inside, false), button_event(inside, true)];
        let frame_events = PointerFrameEvents::collect(
            &events,
            None,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(20.0, 20.0)),
            TermMode::NONE,
        );

        assert_eq!(frame_events.body_primary_press_pos, Some(inside));
        assert_eq!(frame_events.primary_release_pos, None);
        assert_eq!(frame_events.primary_release_index, None);
    }

    #[test]
    fn command_press_remains_available_for_clickable_targets() {
        let inside = Pos2::new(4.0, 4.0);
        let events = vec![Event::PointerButton {
            pos: inside,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::COMMAND,
        }];
        let frame_events = PointerFrameEvents::collect(
            &events,
            None,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(20.0, 20.0)),
            TermMode::NONE,
        );

        assert_eq!(frame_events.body_primary_press_pos, None);
        assert_eq!(frame_events.primary_release_index, None);
    }

    #[test]
    fn completed_local_interaction_does_not_claim_later_command_press() {
        let inside = Pos2::new(4.0, 4.0);
        let rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(20.0, 20.0));
        let command_press = Event::PointerButton {
            pos: inside,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::COMMAND,
        };
        let events = [button_event(inside, true), button_event(inside, false), command_press];
        let mut tracker = LocalSelectionEventTracker::new(false);
        let claims: Vec<_> = events
            .iter()
            .map(|event| tracker.claims(event, None, rect, TermMode::NONE))
            .collect();

        assert_eq!(claims, [true, true, false]);
    }

    #[test]
    fn shift_press_remains_local_selection_in_mouse_mode() {
        let inside = Pos2::new(4.0, 4.0);
        let events = vec![Event::PointerButton {
            pos: inside,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::SHIFT,
        }];
        let frame_events = PointerFrameEvents::collect(
            &events,
            None,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(20.0, 20.0)),
            TermMode::MOUSE_MODE,
        );

        assert_eq!(frame_events.body_primary_press_pos, Some(inside));
    }

    #[test]
    fn unmodified_press_in_mouse_mode_is_not_local_selection() {
        let inside = Pos2::new(4.0, 4.0);
        let events = vec![button_event(inside, true)];
        let frame_events = PointerFrameEvents::collect(
            &events,
            None,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(20.0, 20.0)),
            TermMode::MOUSE_MODE,
        );

        assert_eq!(frame_events.body_primary_press_pos, None);
    }

    #[test]
    fn completed_shift_selection_does_not_claim_later_pty_mouse_press() {
        let inside = Pos2::new(4.0, 4.0);
        let rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(20.0, 20.0));
        let shift_press = Event::PointerButton {
            pos: inside,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::SHIFT,
        };
        let release = Event::PointerButton {
            pos: inside,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::SHIFT,
        };
        let alt_press = Event::PointerButton {
            pos: inside,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::ALT,
        };
        let events = [shift_press, release, alt_press];
        let mut tracker = LocalSelectionEventTracker::new(false);
        let claims: Vec<_> = events
            .iter()
            .map(|event| tracker.claims(event, None, rect, TermMode::MOUSE_MODE))
            .collect();

        assert_eq!(claims, [true, true, false]);
    }
}
