//! The reference rows of the cards of a conversation: the row of a card that
//! stands on the canvas, the row of a card that left the board, and the press
//! that brings a card into view.

use egui::{Event, Modifiers, PointerButton, Pos2};
use horizon_browser_control::manifest;
use horizon_browser_control::manifest::block::Operation;
use horizon_core::agent_bridge::{Entry, actor};
use horizon_core::browser::manifest::block::{Outcome, Request};

use super::chat::{WIDE, stand_in};
use super::frame::{Drawn, Window, texts};
use crate::app::test_support::editor_workspace_state;

/// The local id of the workspace of this test: the form that `new_local_id`
/// makes, which is the only form that an orchestrator actor carries.
const WORKSPACE: &str = "0f1e2d3c-4b5a-4697-8877-665544332211";

/// The stand-in of the program of the dock: it names its session and then reads
/// the lines of the person until the bridge stops.
const SILENT: &str = r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000001"}'
while IFS= read -r line; do :; done
"#;

/// The document of the one card of this test.
fn document() -> serde_json::Value {
    serde_json::json!({
        "components": [
            {"type": "heading", "text": "Plan"},
            {"type": "buttons", "items": [{"id": "go", "label": "Go"}]}
        ]
    })
}

/// A request of the orchestrator of the workspace of this test.
fn request(operation: Operation) -> Request {
    Request {
        request_id: format!("test-row-{}", manifest::now_millis()),
        actor: actor::of_workspace(WORKSPACE).expect("the actor of a workspace"),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

/// Draws one card of the orchestrator of the workspace, and answers its local id
/// and its panel id.
fn draw_card(window: &mut Window) -> (String, horizon_core::PanelId) {
    let ctx = window.ctx().clone();
    let outcome = window.app().apply_block_request(
        &request(Operation::Create {
            title: Some("Plan".into()),
            document: document(),
        }),
        &ctx,
    );
    let Outcome::Created { panel_id, .. } = outcome else {
        panic!("the orchestrator draws its card: {outcome:?}");
    };
    let id = window
        .app()
        .board
        .panel_id_by_local_id(&panel_id)
        .expect("the card is on the board");
    (panel_id, id)
}

/// The reference rows of the conversation of this workspace, as the dock holds
/// them.
fn rows(window: &mut Window) -> Vec<(String, String)> {
    window
        .app()
        .orchestrator_dock
        .conversation_entries(WORKSPACE)
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Card { panel, title } => Some((panel, title)),
            _ => None,
        })
        .collect()
}

/// The one text of `drawn` that reads `text` inside the rectangle of the dock.
fn in_dock(drawn: &[Drawn], dock: egui::Rect, text: &str) -> Drawn {
    drawn
        .iter()
        .find(|drawn| drawn.text == text && dock.contains(drawn.rect.center()))
        .cloned()
        .unwrap_or_else(|| panic!("the dock draws {text:?}: {drawn:?}"))
}

/// One pointer press or release at `at`.
fn pointer(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// A window on one workspace of this test whose orchestrator runs the stand-in,
/// with the folder of that program, which the caller keeps alive.
fn window() -> (Window, tempfile::TempDir) {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let program = stand_in(folder.path(), SILENT);
    let mut window = Window::with_program(WIDE, vec![editor_workspace_state(WORKSPACE, [40.0, 40.0])], &program);
    window.focus_workspace(WORKSPACE);
    let _ = window.frame(Vec::new());
    (window, folder)
}

/// Starts the program of the dock of this workspace, so the orchestrator of the
/// workspace is a caller.
fn start(window: &mut Window) {
    let ctx = window.ctx().clone();
    assert!(
        window.app().say_to_orchestrator(&ctx, WORKSPACE, "hello"),
        "the message of the person reaches the conversation of the workspace"
    );
    window.wait_for("the program of the dock", |app| {
        app.orchestrator_dock.running(WORKSPACE)
    });
}

#[test]
fn the_dock_draws_one_row_for_a_card_of_its_orchestrator() {
    let (mut window, _folder) = window();
    start(&mut window);
    let (panel, _) = draw_card(&mut window);
    let _ = window.frame(Vec::new());

    assert_eq!(rows(&mut window), vec![(panel, "Plan".to_owned())]);
    let dock = window
        .app()
        .orchestrator_dock
        .frame_rect()
        .expect("the dock takes the right edge of the frame");
    let output = window.frame(Vec::new());
    let drawn = texts(&output);
    let title = in_dock(&drawn, dock, "Plan");
    let state = in_dock(&drawn, dock, "on the canvas");
    assert!(
        title.rect.left() < state.rect.left(),
        "the row puts the state at its right end"
    );
    assert!(
        title.rect.height() > state.rect.height(),
        "the heading of a row is 12.5 points and its state 11: {title:?} {state:?}"
    );
    assert_eq!(
        title.color,
        Some(crate::theme::FG()),
        "the heading of a row is the text colour"
    );
    assert!(title.rect.height() < 34.0, "the heading of a row fits its row");
}

#[test]
fn a_press_on_the_row_of_a_card_brings_the_card_into_view() {
    let (mut window, _folder) = window();
    start(&mut window);
    let (_, id) = draw_card(&mut window);
    window.app().board.focused = None;
    // The card stands away from the middle of the canvas, so the frame that
    // follows the press moves the canvas to it.
    let _ = window.frame(Vec::new());
    let dock = window
        .app()
        .orchestrator_dock
        .frame_rect()
        .expect("the dock of the frame");
    let output = window.frame(Vec::new());
    let row = in_dock(&texts(&output), dock, "on the canvas").rect;

    let at = row.center();
    let _ = window.frame(vec![Event::PointerMoved(at), pointer(at, true)]);
    let _ = window.frame(vec![pointer(at, false)]);

    assert_eq!(
        window.app().board.focused,
        Some(id),
        "the press on the row reveals the card of the row"
    );
}

#[test]
fn the_row_of_a_card_that_left_the_board_reads_closed_and_takes_no_press() {
    let (mut window, _folder) = window();
    start(&mut window);
    let (panel, id) = draw_card(&mut window);
    let ctx = window.ctx().clone();
    let closed = window.app().apply_block_request(
        &request(Operation::Close {
            panel_id: panel.clone(),
        }),
        &ctx,
    );
    assert!(matches!(closed, Outcome::Closed { .. }), "{closed:?}");
    assert!(
        window.app().board.panel_id_by_local_id(&panel).is_none(),
        "the card left the board"
    );
    window.app().board.focused = None;
    let _ = window.frame(Vec::new());
    let dock = window
        .app()
        .orchestrator_dock
        .frame_rect()
        .expect("the dock of the frame");
    let output = window.frame(Vec::new());
    let drawn = texts(&output);
    let title = in_dock(&drawn, dock, "Plan");
    let state = in_dock(&drawn, dock, "closed");
    assert_eq!(
        title.color,
        Some(crate::theme::FG_DIM()),
        "the row of a card that is gone reads dim"
    );
    assert_eq!(state.color, Some(crate::theme::FG_DIM()));

    let at = title.rect.center();
    let _ = window.frame(vec![Event::PointerMoved(at), pointer(at, true)]);
    let _ = window.frame(vec![pointer(at, false)]);

    assert_eq!(window.app().board.focused, None, "a closed row takes no press");
    assert_eq!(rows(&mut window), vec![(panel, "Plan".to_owned())], "the row stays");
    assert!(window.app().board.panel(id).is_none(), "no card came back");
}
