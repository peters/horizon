//! The draft of each workspace: the field holds the draft of the workspace that
//! is active, and the drafts of the others wait.

use super::frame::{Window, texts};
use crate::app::test_support::editor_workspace_state;

/// The two workspaces of these tests, with the stable id that a draft is keyed on.
fn two_workspaces() -> Vec<horizon_core::WorkspaceState> {
    vec![
        editor_workspace_state("dock-a", [40.0, 40.0]),
        editor_workspace_state("dock-b", [560.0, 40.0]),
    ]
}

/// The texts of the frame after `local_id` became the active workspace.
fn frame_of(window: &mut Window, local_id: &str) -> Vec<String> {
    window.focus_workspace(local_id);
    let output = window.frame(Vec::new());
    texts(&output).into_iter().map(|drawn| drawn.text).collect()
}

#[test]
fn the_draft_of_one_workspace_does_not_show_in_another() {
    let mut window = Window::new([2000.0, 1000.0], two_workspaces(), true);
    window.note_draft("dock-a", "for dock-a");
    window.note_draft("dock-b", "for dock-b");

    let texts_a = frame_of(&mut window, "dock-a");
    assert!(texts_a.iter().any(|text| text == "for dock-a"));
    assert!(!texts_a.iter().any(|text| text == "for dock-b"));

    let texts_b = frame_of(&mut window, "dock-b");
    assert!(texts_b.iter().any(|text| text == "for dock-b"));
    assert!(!texts_b.iter().any(|text| text == "for dock-a"));
}

#[test]
fn a_workspace_without_a_draft_shows_the_hint_of_the_field() {
    let mut window = Window::new([2000.0, 1000.0], two_workspaces(), true);
    window.note_draft("dock-a", "for dock-a");

    let texts_b = frame_of(&mut window, "dock-b");

    assert!(texts_b.iter().any(|text| text == "Write to the orchestrator"));
    assert!(!texts_b.iter().any(|text| text == "for dock-a"));
}
