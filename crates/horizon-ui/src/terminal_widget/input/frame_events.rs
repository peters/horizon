use egui::emath::TSTransform;
use egui::{PointerButton, Pos2, Rect};

pub(super) struct PointerFrameEvents {
    pub(super) body_primary_press_pos: Option<Pos2>,
    pub(super) primary_release_pos: Option<Pos2>,
    pub(super) primary_release_ends_frame: bool,
    pub(super) body_middle_press_pos: Option<Pos2>,
}

impl PointerFrameEvents {
    pub(super) fn collect(events: &[egui::Event], from_global: Option<TSTransform>, body_rect: Rect) -> Self {
        Self {
            body_primary_press_pos: pointer_button_event_pos(
                events,
                from_global,
                PointerButton::Primary,
                true,
                body_rect,
            ),
            primary_release_pos: pointer_button_event_any_pos(events, from_global, PointerButton::Primary, false),
            primary_release_ends_frame: primary_release_ends_frame(events),
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

pub(super) fn primary_release_ends_frame(events: &[egui::Event]) -> bool {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            egui::Event::PointerButton {
                button: PointerButton::Primary,
                pressed,
                ..
            } => Some(!*pressed),
            _ => None,
        })
        .unwrap_or(false)
}

pub(super) fn final_primary_release_index(events: &[egui::Event]) -> Option<usize> {
    events.iter().rposition(|event| {
        matches!(
            event,
            egui::Event::PointerButton {
                button: PointerButton::Primary,
                pressed: false,
                ..
            }
        )
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
    use super::{
        final_primary_release_index, pointer_button_event_pos, pointer_event_targets_rect, primary_release_ends_frame,
    };
    use egui::{Event, Modifiers, PointerButton, Pos2, Rect};

    fn button_event(pressed: bool) -> Event {
        Event::PointerButton {
            pos: Pos2::new(4.0, 4.0),
            button: PointerButton::Primary,
            pressed,
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
    fn primary_release_only_ends_frame_when_it_is_the_last_transition() {
        assert!(primary_release_ends_frame(&[button_event(true), button_event(false)]));
        assert!(!primary_release_ends_frame(&[button_event(false), button_event(true)]));
        assert!(!primary_release_ends_frame(&[Event::PointerMoved(Pos2::ZERO)]));
    }

    #[test]
    fn final_primary_release_index_finds_the_last_release_event() {
        let events = vec![button_event(false), button_event(true), button_event(false)];
        assert_eq!(final_primary_release_index(&events), Some(2));
        assert_eq!(final_primary_release_index(&[button_event(true)]), None);
    }
}
