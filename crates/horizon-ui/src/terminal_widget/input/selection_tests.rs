use super::{TerminalSelectionDragState, pointer_button_event_any_pos, pointer_button_event_pos};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect};
use horizon_core::PanelId;

#[test]
fn pointer_button_event_any_pos_keeps_release_outside_rect() {
    let rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(20.0, 20.0));
    let events = vec![Event::PointerButton {
        pos: Pos2::new(42.0, 6.0),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }];

    assert_eq!(
        pointer_button_event_pos(&events, None, PointerButton::Primary, false, rect),
        None
    );
    assert_eq!(
        pointer_button_event_any_pos(&events, None, PointerButton::Primary, false),
        Some(Pos2::new(42.0, 6.0))
    );
}

#[test]
fn selection_drag_state_ignores_finish_for_other_panels() {
    let panel_id = PanelId(42);
    let other_panel_id = PanelId(7);
    let mut state = TerminalSelectionDragState::default();

    state.start(panel_id, Pos2::new(4.0, 4.0));

    assert!(state.active_for(panel_id));
    assert!(!state.active_for(other_panel_id));
    assert!(!state.finish(other_panel_id));
    assert!(state.active_for(panel_id));
}

#[test]
fn selection_drag_state_only_reports_copy_after_movement() {
    let panel_id = PanelId(42);
    let mut state = TerminalSelectionDragState::default();

    state.start(panel_id, Pos2::new(4.0, 4.0));
    state.mark_dragged(panel_id, Pos2::new(4.0, 4.0), 6.0);
    assert!(!state.finish(panel_id));

    state.start(panel_id, Pos2::new(4.0, 4.0));
    state.mark_dragged(panel_id, Pos2::new(16.0, 4.0), 6.0);
    assert!(state.finish(panel_id));
    assert!(!state.active_for(panel_id));
}

#[test]
fn selection_drag_state_uses_click_movement_threshold() {
    let panel_id = PanelId(42);
    let mut state = TerminalSelectionDragState::default();

    state.start(panel_id, Pos2::new(4.0, 4.0));
    state.mark_dragged(panel_id, Pos2::new(9.0, 4.0), 6.0);
    assert!(!state.finish(panel_id));

    state.start(panel_id, Pos2::new(4.0, 4.0));
    state.mark_dragged(panel_id, Pos2::new(11.0, 4.0), 6.0);
    assert!(state.finish(panel_id));
}
