//! `Tidy` on the line of a workspace puts its panels in the places of its
//! preset and every card of an agent back into the column of that agent.
//!
//! Every lane draws its cards through the request of an agent panel, runs whole
//! frames of the application and presses the button as a person does.

use egui::{Event, Rect, Vec2};
use horizon_core::{BlockPanelState, CanvasViewState, CardPlace, PanelOptions, WindowConfig, WorkspaceLayout};

use super::columns::{
    AGENT, GAP, OTHER, WINDOW, agent_board, assert_column, beside_agent, drag_title, draw, place, pointer, started,
    workspace,
};
use super::*;
use crate::app::test_support::{drawn_text, drew_text, editor_panel_state};

/// The smallest window that the build opens.
const SMALL_WINDOW: [f32; 2] = [800.0, 600.0];
/// What the button says while the pointer rests on it.
const HOVER: &str = "Put the panels and the cards of this workspace in order";

/// One frame of a window of `window` with the events `events`.
fn frame(ctx: &Context, app: &mut HorizonApp, window: [f32; 2], events: Vec<Event>) -> egui::FullOutput {
    let mut input = raw_input(window, None);
    input.events = events;
    run_app_frame_with_input(ctx, app, input)
}

/// One press of the person on `Tidy` on the line of the workspace, and the
/// frames that settle the board after it.
fn press_tidy(ctx: &Context, app: &mut HorizonApp) {
    // The person looks at the whole workspace, and a press follows the canvas
    // that the person sees after the view rests.
    let workspace = app.board.panels[0].workspace_id;
    let _ = app.fit_workspace_visible(ctx, workspace);
    for _ in 0..120 {
        let _ = frame(ctx, app, WINDOW, Vec::new());
        if app.pan_target.is_none() {
            break;
        }
    }
    let button = drawn_text(&frame(ctx, app, WINDOW, Vec::new()), "Tidy").center();
    for events in [
        vec![Event::PointerMoved(button)],
        vec![pointer(button, true)],
        vec![pointer(button, false)],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ] {
        let _ = frame(ctx, app, WINDOW, events);
    }
}

/// Where the card `card` stands among the panels of its workspace.
fn card_place(app: &HorizonApp, card: &str) -> CardPlace {
    let id = app.board.panel_id_by_local_id(card).expect("the card");
    app.board
        .panel(id)
        .and_then(|panel| panel.block())
        .expect("a card")
        .place
}

/// Checks that no two panels of `panels` cover a common place.
fn assert_no_overlap(app: &HorizonApp, panels: &[&str]) {
    let rects: Vec<Rect> = panels
        .iter()
        .map(|panel| place(app, panel))
        .map(|[left, top, right, bottom]| Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom)))
        .collect();
    for (index, rect) in rects.iter().enumerate() {
        for (other, under) in rects.iter().enumerate().skip(index + 1) {
            assert!(
                !rect.shrink(0.5).intersects(*under),
                "{} and {} cover a common place: {rect:?} and {under:?}",
                panels[index],
                panels[other]
            );
        }
    }
}

/// Draws a card of Horizon itself in the workspace of the agent panel, as the
/// notice of Set up is, and returns its local id.
fn horizon_card(ctx: &Context, app: &mut HorizonApp) -> String {
    let document = serde_json::from_value(serde_json::json!({"components": [
        {"type": "heading", "text": "A notice of Horizon"}
    ]}))
    .expect("a document");
    let options = PanelOptions {
        kind: PanelKind::Block,
        block: Some(BlockPanelState::new(document)),
        size: Some([420.0, 160.0]),
        ..PanelOptions::default()
    };
    let workspace = app.board.panels[0].workspace_id;
    let id = app
        .create_panel_with_options(options, workspace)
        .expect("the card of Horizon");
    for _ in 0..3 {
        let _ = frame(ctx, app, WINDOW, Vec::new());
    }
    app.board.panel(id).expect("the card of Horizon").local_id.clone()
}

#[test]
fn tidy_puts_a_card_that_the_person_dragged_out_back_into_its_column() {
    let (_temp, ctx, mut app) = agent_board(Some(WorkspaceLayout::Grid));
    let notice = horizon_card(&ctx, &mut app);
    let first = draw(&ctx, &mut app, "first", 2);
    let second = draw(&ctx, &mut app, "second", 6);
    let third = draw(&ctx, &mut app, "third", 1);
    let column = place(&app, &third)[0];
    assert_column(&app, column, &[&third, &second, &first]);

    // The person puts the card far below its column.
    let from = place(&app, &second);
    let by = Vec2::new(-200.0, 1400.0 - from[1]) * app.canvas_view.zoom;
    drag_title(&ctx, &mut app, &second, by);
    assert_eq!(
        card_place(&app, &second),
        CardPlace::Free,
        "a card that the person dragged is free"
    );
    assert_column(&app, column, &[&third, &first]);
    let before = [place(&app, AGENT), place(&app, &notice)];

    press_tidy(&ctx, &mut app);

    assert_eq!(
        card_place(&app, &second),
        CardPlace::Column,
        "the card is in its column again"
    );
    assert_column(&app, column, &[&third, &second, &first]);
    assert_no_overlap(&app, &[AGENT, &notice, &first, &second, &third]);
    assert_eq!(
        [place(&app, AGENT), place(&app, &notice)],
        before,
        "the agent panel and the card of Horizon keep their places"
    );
    assert_eq!(
        card_place(&app, &notice),
        CardPlace::Tile,
        "a card of Horizon takes no column"
    );
}

#[test]
fn tidy_arranges_the_panels_that_the_person_moved_and_takes_the_cards_of_an_older_board() {
    let panels = vec![
        editor_panel_state(AGENT, [20.0, 20.0]),
        editor_panel_state(OTHER, [360.0, 20.0]),
    ];
    let (_temp, ctx, mut app) = started(workspace(None, panels));
    let first = draw(&ctx, &mut app, "first", 2);
    let second = draw(&ctx, &mut app, "second", 3);
    let gone = draw(&ctx, &mut app, "gone", 1);
    let arranged = [place(&app, AGENT), place(&app, OTHER)];

    // The person moves a panel of a workspace that has no preset.
    let by = Vec2::new(40.0, 500.0) * app.canvas_view.zoom;
    drag_title(&ctx, &mut app, OTHER, by);
    assert!(
        place(&app, OTHER)[1] > arranged[1][1] + 300.0 && app.board.workspaces[0].layout.is_none(),
        "the person moved the panel out of the preset: {:?}",
        place(&app, OTHER)
    );

    // A board that an older build saved holds each card as a tile, and the agent
    // panel of one card is gone.
    let mut saved = RuntimeState::from_board(&app.board, WindowConfig::default(), CanvasViewState::default());
    saved.workspaces[0].panels[0].kind = PanelKind::Editor;
    for card in saved.workspaces[0]
        .panels
        .iter_mut()
        .filter_map(|panel| panel.block.as_mut())
    {
        card.place = CardPlace::Tile;
        if card.title == "gone" {
            card.owner_actor = Some("horizon:2c7a51d3-9e0b-4f86-b1c4-6d3e8a9f0b25".into());
        }
    }
    let (_restored_temp, ctx, mut app) = started(saved);
    let gone_before = card_place(&app, &gone);

    press_tidy(&ctx, &mut app);

    assert_eq!(
        app.board.workspaces[0].layout,
        Some(WorkspaceLayout::Grid),
        "a workspace without a preset takes the grid"
    );
    let (agent, other) = (place(&app, AGENT), place(&app, OTHER));
    assert!(
        (other[1] - agent[1]).abs() < 0.5 && (other[0] - (agent[2] + GAP)).abs() < 0.5,
        "the panel that the person moved stands beside the agent panel again: {agent:?} and {other:?}"
    );
    assert_eq!(
        [
            card_place(&app, &first),
            card_place(&app, &second),
            card_place(&app, &gone)
        ],
        [CardPlace::Column, CardPlace::Column, gone_before],
        "the cards of the agent go to its column, and the card of an agent that is gone stays"
    );
    let column = place(&app, &second)[0];
    assert!(
        column > beside_agent(&app),
        "the column stands at the right of the panels: {column}"
    );
    assert_column(&app, column, &[&second, &first]);
    assert_no_overlap(&app, &[AGENT, OTHER, &first, &second, &gone]);
}

#[test]
fn the_line_of_a_workspace_holds_tidy_before_the_button_of_a_new_panel_in_the_smallest_window() {
    let (_temp, ctx, mut app) = agent_board(Some(WorkspaceLayout::Grid));
    // The workspace that the person makes next stands beside the first one.
    let next = app.board.create_workspace("next");
    let next_left = app.board.workspace(next).expect("the next workspace").position[0];
    let mut line = Vec::new();
    // The line stands in the canvas with the sidebar closed, and with it open.
    for sidebar in [true, false] {
        app.sidebar_visible = sidebar;
        for _ in 0..4 {
            let _ = frame(&ctx, &mut app, SMALL_WINDOW, Vec::new());
        }
        let output = frame(&ctx, &mut app, SMALL_WINDOW, Vec::new());
        let canvas = app.canvas_rect(&ctx);
        line = ["Default", "Rows", "Cols", "Grid", "Detach", "Tidy", "+ Panel"]
            .iter()
            .map(|label| drawn_text(&output, label))
            .collect::<Vec<Rect>>();

        assert!(
            line.windows(2).all(|pair| pair[0].max.x < pair[1].min.x),
            "the buttons stand in one row, with Tidy before the button of a new panel: {line:?}"
        );
        // The last button is 62 points wide, and the frame of the line ends 6
        // points after it.
        let end = line[6].center().x + (31.0 + 6.0) * app.canvas_view.zoom;
        assert!(
            line.iter().all(|button| canvas.contains_rect(*button)) && end <= canvas.max.x,
            "the canvas of the smallest window holds the line (sidebar {sidebar}): {line:?} to {end} in {canvas:?}"
        );
        assert!(
            app.screen_to_canvas(canvas, egui::pos2(end, line[6].center().y)).x < next_left,
            "the line ends before the next workspace at {next_left}: {end}"
        );
    }

    // The pointer rests on the button, and the button says what it does.
    let mut said = false;
    for step in 0..12 {
        let mut input = raw_input(SMALL_WINDOW, None);
        input.time = Some(10.0 + f64::from(step) * 0.2);
        if step == 0 {
            input.events = vec![Event::PointerMoved(line[5].center())];
        }
        said |= drew_text(&run_app_frame_with_input(&ctx, &mut app, input), HOVER);
    }
    assert!(said, "the button says what a press does");
}
