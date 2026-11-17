//! The cards of one agent panel stand in one column at the right of that panel.
//!
//! Every lane draws its cards through the request of an agent panel, runs whole
//! frames of the application and reads the places that the board holds.

use egui::{Event, Modifiers, PointerButton, Pos2, Vec2};
use horizon_core::{CanvasViewState, PanelState, WindowConfig, WorkspaceLayout, WorkspaceState};

use super::*;
use crate::app::test_support::{card_panel, editor_panel_state, editor_workspace_state};

/// The window of the person.
const WINDOW: [f32; 2] = [1600.0, 1000.0];
/// The gap that two tiles of a workspace keep.
const GAP: f32 = 20.0;
/// The local id of the agent panel, in the form that Horizon gives a panel.
const AGENT: &str = "0b9c2f5e-6a1d-4c3b-9e7f-2d4a6b8c0e11";
/// The local id of a terminal of the person in the same workspace.
const OTHER: &str = "7d1e4a90-3c52-4f6b-8a1d-5e9c0b7f2a44";

/// One workspace with the panels `panels`, arranged by `layout`.
fn workspace(layout: Option<WorkspaceLayout>, panels: Vec<PanelState>) -> RuntimeState {
    RuntimeState {
        workspaces: vec![WorkspaceState {
            layout,
            panels,
            ..editor_workspace_state("first", [0.0, 0.0])
        }],
        ..RuntimeState::default()
    }
}

/// The application of a saved board whose first panel is the agent panel.
fn started(runtime: RuntimeState) -> (tempfile::TempDir, Context, HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    // No live agent process is needed to exercise the caller's identity.
    app.board.panels[0].kind = PanelKind::Claude;
    app.sidebar_visible = false;
    settle(&ctx, &mut app);
    (temp, ctx, app)
}

/// A board with one agent panel.
fn agent_board(layout: Option<WorkspaceLayout>) -> (tempfile::TempDir, Context, HorizonApp) {
    started(workspace(layout, vec![editor_panel_state(AGENT, [20.0, 20.0])]))
}

/// The frames that measure a new document and give its card the height that fits.
fn settle(ctx: &Context, app: &mut HorizonApp) {
    for _ in 0..3 {
        let _ = run_app_frame_with_input(ctx, app, raw_input(WINDOW, None));
    }
}

/// A document of `rows` pairs under the heading `heading`.
fn rows_document(heading: &str, rows: usize) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = (0..rows)
        .map(|index| serde_json::json!({"label": format!("Label {index}"), "value": "value"}))
        .collect();
    serde_json::json!({"components": [{"type": "heading", "text": heading}, {"type": "kv", "rows": rows}]})
}

/// Draws one card of the agent panel through its request and lets it settle.
fn draw(ctx: &Context, app: &mut HorizonApp, heading: &str, rows: usize) -> String {
    let (card, _) = created(app.apply_block_request(
        &request(
            app,
            Operation::Create {
                title: Some(heading.to_owned()),
                document: rows_document(heading, rows),
            },
        ),
        ctx,
    ));
    settle(ctx, app);
    card
}

/// The rectangle `[left, top, right, bottom]` of the panel `local_id` on the canvas.
fn place(app: &HorizonApp, local_id: &str) -> [f32; 4] {
    let panel = app
        .board
        .panel(app.board.panel_id_by_local_id(local_id).expect("the panel"))
        .expect("the panel");
    let [x, y] = panel.layout.position;
    [x, y, x + panel.layout.size[0], y + panel.layout.size[1]]
}

/// Checks that `cards`, from the top, stand in one column whose left edge is
/// `left` and whose top is the top of the agent panel, with the gap of tiles.
fn assert_column(app: &HorizonApp, left: f32, cards: &[&str]) {
    let mut top = place(app, AGENT)[1];
    for card in cards {
        let rect = place(app, card);
        assert!(
            (rect[0] - left).abs() < 0.5 && (rect[1] - top).abs() < 0.5,
            "the card {card} stands at {left}, {top} of its column: {rect:?}"
        );
        top = rect[3] + GAP;
    }
}

/// The left edge of a column that stands directly at the right of the agent panel.
fn beside_agent(app: &HorizonApp) -> f32 {
    place(app, AGENT)[2] + GAP
}

fn pointer(position: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: position,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// Drags the title bar of the panel `local_id` by `by` screen points, as a person does.
fn drag_title(ctx: &Context, app: &mut HorizonApp, local_id: &str, by: Vec2) {
    let from = card_panel(app, ctx, local_id).min + Vec2::new(60.0, 17.0 * app.canvas_view.zoom);
    let frames = [
        vec![Event::PointerMoved(from)],
        vec![pointer(from, true)],
        // The first short move starts the drag, and the second one carries the panel.
        vec![Event::PointerMoved(from + by.normalized() * 12.0)],
        vec![Event::PointerMoved(from + by)],
        vec![pointer(from + by, false)],
        Vec::new(),
    ];
    for events in frames {
        let mut input = raw_input(WINDOW, None);
        input.events = events;
        let _ = run_app_frame_with_input(ctx, app, input);
    }
}

#[test]
fn the_cards_of_an_agent_stand_in_one_column_at_its_right_with_the_newest_at_the_top() {
    for layout in [None, Some(WorkspaceLayout::Grid)] {
        let (_temp, ctx, mut app) = agent_board(layout);
        let agent = place(&app, AGENT);

        let first = draw(&ctx, &mut app, "first", 2);
        let second = draw(&ctx, &mut app, "second", 6);
        let third = draw(&ctx, &mut app, "third", 1);

        assert_eq!(place(&app, AGENT), agent, "a card moves no agent panel ({layout:?})");
        assert_column(&app, beside_agent(&app), &[&third, &second, &first]);
        let mut lines = Vec::new();
        app.board.fill_agent_links(&mut lines);
        assert_eq!(
            lines.len(),
            3,
            "every card keeps its line to the agent panel ({layout:?})"
        );
        let heights: Vec<f32> = [&first, &second, &third]
            .iter()
            .map(|card| place(&app, card)[3] - place(&app, card)[1])
            .collect();
        assert!(
            heights[1] > heights[0] && heights[0] > heights[2],
            "every card of the column has the height of its own content ({layout:?}): {heights:?}"
        );
    }
}

#[test]
fn a_closed_card_leaves_no_hole_in_the_column() {
    let (_temp, ctx, mut app) = agent_board(None);
    let first = draw(&ctx, &mut app, "first", 2);
    let second = draw(&ctx, &mut app, "second", 6);
    let third = draw(&ctx, &mut app, "third", 1);

    let closed = app.apply_block_request(&request(&app, Operation::Close { panel_id: second }), &ctx);
    assert!(
        matches!(closed, Outcome::Closed { .. }),
        "the agent closes its card: {closed:?}"
    );
    settle(&ctx, &mut app);

    assert_column(&app, beside_agent(&app), &[&third, &first]);
}

#[test]
fn the_column_moves_with_the_agent_panel_that_the_person_drags() {
    let (_temp, ctx, mut app) = agent_board(None);
    let first = draw(&ctx, &mut app, "first", 2);
    let second = draw(&ctx, &mut app, "second", 3);
    let before = place(&app, AGENT);

    drag_title(&ctx, &mut app, AGENT, Vec2::new(90.0, 140.0));

    let after = place(&app, AGENT);
    assert!(
        after[0] > before[0] + 40.0 && after[1] > before[1] + 60.0,
        "the person moved the agent panel: {before:?} to {after:?}"
    );
    assert_column(&app, beside_agent(&app), &[&second, &first]);
}

#[test]
fn a_card_that_the_person_dragged_stays_where_it_was_put_and_a_restore_keeps_the_picture() {
    let (_temp, ctx, mut app) = agent_board(None);
    let first = draw(&ctx, &mut app, "first", 2);
    let second = draw(&ctx, &mut app, "second", 3);
    let third = draw(&ctx, &mut app, "third", 1);
    let in_column = place(&app, &second);

    // The person puts the card under the agent panel, far below the column.
    let by = Vec2::new(place(&app, AGENT)[0] - in_column[0], 900.0 - in_column[1]) * app.canvas_view.zoom;
    drag_title(&ctx, &mut app, &second, by);
    settle(&ctx, &mut app);

    let put = place(&app, &second);
    assert!(
        put[1] > 800.0 && put[0] < in_column[0],
        "the card stands where the person put it: {in_column:?} to {put:?}"
    );
    assert_column(&app, beside_agent(&app), &[&third, &first]);

    // A later card of the agent goes to the column and leaves the card of the person alone.
    let fourth = draw(&ctx, &mut app, "fourth", 1);
    assert_eq!(place(&app, &second), put, "the card of the person stays");
    assert_column(&app, beside_agent(&app), &[&fourth, &third, &first]);

    let mut saved = RuntimeState::from_board(&app.board, WindowConfig::default(), CanvasViewState::default());
    // The saved agent panel starts no agent program in a lane.
    saved.workspaces[0].panels[0].kind = PanelKind::Editor;
    let picture: Vec<[f32; 4]> = [AGENT, &first, &second, &third, &fourth]
        .iter()
        .map(|panel| place(&app, panel))
        .collect();
    let (_restored_temp, ctx, mut app) = started(saved);

    let restored: Vec<[f32; 4]> = [AGENT, &first, &second, &third, &fourth]
        .iter()
        .map(|panel| place(&app, panel))
        .collect();
    assert_eq!(restored, picture, "a restore gives the same picture");
    // The column is still a column, and the card of the person is still outside it.
    let fifth = draw(&ctx, &mut app, "fifth", 1);
    assert_eq!(
        place(&app, &second),
        put,
        "a restore keeps the card where the person put it"
    );
    assert_column(&app, beside_agent(&app), &[&fifth, &fourth, &third, &first]);
}

#[test]
fn a_column_stands_at_the_right_of_a_panel_that_holds_the_place_beside_the_agent() {
    for layout in [None, Some(WorkspaceLayout::Grid)] {
        let (_temp, ctx, mut app) = started(workspace(
            layout,
            vec![
                editor_panel_state(AGENT, [20.0, 20.0]),
                editor_panel_state(OTHER, [360.0, 20.0]),
            ],
        ));
        let panels = [place(&app, AGENT), place(&app, OTHER)];

        let first = draw(&ctx, &mut app, "first", 2);
        let second = draw(&ctx, &mut app, "second", 3);

        assert_eq!(
            [place(&app, AGENT), place(&app, OTHER)],
            panels,
            "a card moves no panel of the person ({layout:?})"
        );
        assert_column(&app, place(&app, OTHER)[2] + GAP, &[&second, &first]);
    }
}
