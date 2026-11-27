//! The switch of the experiment: the environment variable that turns the dock
//! on, and the state that the command palette toggles.

use egui::Rect;

use crate::app::orchestrator_dock::OrchestratorDock;
use crate::app::orchestrator_dock::dock_switch;

#[test]
fn only_one_turns_the_dock_on() {
    for (value, on) in [
        (Some("1"), true),
        (Some("0"), false),
        (Some("true"), false),
        (Some("yes"), false),
        (Some(" 1"), false),
        (Some(""), false),
        (None, false),
    ] {
        assert_eq!(dock_switch(value), on, "the switch of {value:?}");
    }
}

#[test]
fn the_dock_of_a_run_that_did_not_turn_it_on_draws_nothing() {
    let mut dock = OrchestratorDock::new(false);

    assert!(!dock.visible());
    assert_eq!(dock.frame_rect(), None);

    dock.toggle();

    assert!(dock.visible());
    // A visible dock keeps the rectangle that the frame before gave it, until the
    // frame that draws it takes its own.
    assert_eq!(dock.frame_rect(), None);
    dock.set_frame_rect(Some(Rect::from_min_max(
        egui::Pos2::new(1460.0, 46.0),
        egui::Pos2::new(2000.0, 1000.0),
    )));
    assert!(dock.frame_rect().is_some());
    dock.toggle();
    assert!(!dock.visible());
}
