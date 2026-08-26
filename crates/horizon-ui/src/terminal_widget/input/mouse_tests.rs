use super::routing::{
    pointer_button_checks_clickable_target, pointer_button_event_needs_handling, pointer_button_opens_osc8_hyperlink,
    pointer_button_routes_to_pty_mouse, pointer_button_starts_local_selection, pointer_drag_updates_local_selection,
    pointer_motion_routes_to_pty_mouse,
};
use super::selection_drag::{CapturedPrimaryGesture, TerminalSelectionDragState};
use super::suppress_unowned_primary_motion;
use alacritty_terminal::term::TermMode;
use egui::{Modifiers, PointerButton};
use horizon_core::PanelId;

use crate::input::PointerButtons;

#[test]
fn plain_primary_click_reports_to_pty_in_mouse_mode() {
    let buttons = PointerButtons {
        primary: true,
        middle: false,
        secondary: false,
    };
    let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG;

    assert!(!pointer_button_starts_local_selection(
        mode,
        PointerButton::Primary,
        true,
        Modifiers::NONE
    ));
    assert!(!pointer_drag_updates_local_selection(mode, buttons, Modifiers::NONE));
    assert!(pointer_button_routes_to_pty_mouse(
        mode,
        PointerButton::Primary,
        Modifiers::NONE
    ));
    assert!(pointer_motion_routes_to_pty_mouse(mode, buttons, Modifiers::NONE));
}

#[test]
fn shift_primary_drag_selects_locally_in_mouse_mode() {
    let buttons = PointerButtons {
        primary: true,
        middle: false,
        secondary: false,
    };
    let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG;

    assert!(pointer_button_starts_local_selection(
        mode,
        PointerButton::Primary,
        true,
        Modifiers::SHIFT
    ));
    assert!(pointer_drag_updates_local_selection(mode, buttons, Modifiers::SHIFT));
    assert!(!pointer_button_routes_to_pty_mouse(
        mode,
        PointerButton::Primary,
        Modifiers::SHIFT
    ));
    assert!(!pointer_motion_routes_to_pty_mouse(mode, buttons, Modifiers::SHIFT));
}

#[test]
fn ctrl_or_cmd_primary_click_still_checks_clickable_targets() {
    let mode = TermMode::MOUSE_REPORT_CLICK;

    assert!(pointer_button_checks_clickable_target(
        PointerButton::Primary,
        true,
        Modifiers::CTRL
    ));
    assert!(pointer_button_checks_clickable_target(
        PointerButton::Primary,
        true,
        Modifiers::COMMAND
    ));
    assert!(!pointer_button_starts_local_selection(
        mode,
        PointerButton::Primary,
        true,
        Modifiers::CTRL
    ));
    assert!(!pointer_button_starts_local_selection(
        mode,
        PointerButton::Primary,
        true,
        Modifiers::COMMAND
    ));
    assert!(pointer_button_event_needs_handling(
        mode,
        PointerButton::Primary,
        true,
        Modifiers::CTRL
    ));
    for modifiers in [Modifiers::CTRL, Modifiers::COMMAND] {
        assert!(!pointer_button_starts_local_selection(
            TermMode::NONE,
            PointerButton::Primary,
            true,
            modifiers,
        ));
    }
}

#[test]
fn non_selection_mouse_reporting_remains_available() {
    let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION;
    let secondary_drag = PointerButtons {
        primary: false,
        middle: false,
        secondary: true,
    };

    assert!(pointer_button_routes_to_pty_mouse(
        mode,
        PointerButton::Secondary,
        Modifiers::NONE
    ));
    assert!(pointer_button_routes_to_pty_mouse(
        mode,
        PointerButton::Primary,
        Modifiers::ALT
    ));
    assert!(pointer_motion_routes_to_pty_mouse(
        mode,
        secondary_drag,
        Modifiers::NONE
    ));
    assert!(pointer_motion_routes_to_pty_mouse(
        mode,
        PointerButtons::default(),
        Modifiers::NONE
    ));
}

#[test]
fn unmodified_primary_click_opens_osc8_hyperlink() {
    assert!(pointer_button_opens_osc8_hyperlink(
        PointerButton::Primary,
        Modifiers::NONE
    ));
    assert!(!pointer_button_opens_osc8_hyperlink(
        PointerButton::Primary,
        Modifiers::SHIFT
    ));
    assert!(!pointer_button_opens_osc8_hyperlink(
        PointerButton::Primary,
        Modifiers::CTRL
    ));
    assert!(!pointer_button_opens_osc8_hyperlink(
        PointerButton::Secondary,
        Modifiers::NONE
    ));
}

#[test]
fn primary_pty_gesture_keeps_press_routing_until_its_release() {
    let panel_id = PanelId(42);
    let other_panel_id = PanelId(7);
    let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
    let mut state = TerminalSelectionDragState::default();
    let primary_down = PointerButtons {
        primary: true,
        ..PointerButtons::default()
    };

    assert!(suppress_unowned_primary_motion(&state, panel_id, primary_down, 8, 0));

    state.capture_primary_gesture(
        panel_id,
        CapturedPrimaryGesture::Pty {
            terminal_mode: mode,
            modifiers: Modifiers::ALT,
        },
        8,
        1,
    );
    assert!(!suppress_unowned_primary_motion(&state, panel_id, primary_down, 8, 2));
    assert!(suppress_unowned_primary_motion(
        &state,
        other_panel_id,
        primary_down,
        8,
        2
    ));

    assert!(state.finish_primary_gesture(other_panel_id, 8, 2).is_none());
    assert!(matches!(
        state.finish_primary_gesture(panel_id, 8, 2),
        Some(CapturedPrimaryGesture::Pty {
            terminal_mode,
            modifiers: Modifiers::ALT,
        }) if terminal_mode == mode
    ));
    assert!(state.has_primary_gesture());
    assert!(state.finish_primary_gesture(panel_id, 8, 3).is_none());
    state.expire_completed_primary_gesture(8);
    assert!(state.has_primary_gesture());
    state.expire_completed_primary_gesture(9);
    assert!(!state.has_primary_gesture());
}

#[test]
fn newer_primary_capture_wins_same_frame() {
    let panel_id = PanelId(42);
    let other_panel_id = PanelId(7);
    let mut state = TerminalSelectionDragState::default();
    state.capture_primary_gesture(panel_id, CapturedPrimaryGesture::Horizon, 10, 4);
    state.capture_primary_gesture(other_panel_id, CapturedPrimaryGesture::Horizon, 10, 1);

    assert!(state.primary_gesture_for_event(panel_id, 10, 5).is_some());
    assert!(state.finish_primary_gesture(panel_id, 10, 3).is_none());
    assert!(state.finish_primary_gesture(panel_id, 10, 5).is_some());
}

#[test]
fn earlier_owner_can_finish_after_a_later_capture_is_registered() {
    let earlier_panel_id = PanelId(42);
    let later_panel_id = PanelId(7);
    let mut state = TerminalSelectionDragState::default();
    state.capture_primary_gesture(earlier_panel_id, CapturedPrimaryGesture::Horizon, 9, 1);
    state.capture_primary_gesture(later_panel_id, CapturedPrimaryGesture::Horizon, 10, 2);

    assert!(state.finish_primary_gesture(earlier_panel_id, 10, 1).is_some());
    assert!(state.primary_gesture_for_event(later_panel_id, 10, 3).is_some());
}

#[test]
fn interrupted_primary_gesture_clears_after_the_physical_release() {
    let panel_id = PanelId(42);
    let mut state = TerminalSelectionDragState::default();
    state.capture_primary_gesture(panel_id, CapturedPrimaryGesture::Horizon, 8, 1);

    state.cancel_interrupted_primary_gesture(true, false, true);
    assert!(state.has_primary_gesture());
    state.cancel_interrupted_primary_gesture(false, false, false);
    assert!(state.has_primary_gesture());
    state.cancel_interrupted_primary_gesture(false, true, true);
    assert!(state.has_primary_gesture());
    state.cancel_interrupted_primary_gesture(false, false, true);
    assert!(!state.has_primary_gesture());

    state.capture_primary_gesture(panel_id, CapturedPrimaryGesture::Horizon, 8, 1);
    assert!(state.finish_primary_gesture(panel_id, 8, 2).is_some());
    state.cancel_interrupted_primary_gesture(false, false, true);
    assert!(state.has_primary_gesture());
}
