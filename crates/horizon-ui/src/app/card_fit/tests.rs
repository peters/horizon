//! A card takes the size its content needs, once per document and per set of
//! Horizon-owned frames.

use egui::{Pos2, Rect, Vec2};
use horizon_browser_control::block::{BlockDocument, BoardColumn, BoardItem, BoardRow, BoardState, Component, Width};
use horizon_core::browser::manifest::{
    self,
    block::{Operation, Outcome, Request},
};
use horizon_core::{BlockPanelState, CardFrames, Panel, PanelKind, PanelOptions, RuntimeState, StartupDecision};

use super::*;
use crate::app::test_support::{editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup};
use crate::block_widget::{CARD_CHROME_HEIGHT, CARD_FOOT_SPAN, CARD_HEIGHT_LIMITS};

/// A board with one card panel that measured `measured_content_height`.
fn app_with_card(measured_content_height: f32) -> (tempfile::TempDir, Context, HorizonApp, PanelId) {
    let runtime = RuntimeState {
        workspaces: vec![editor_workspace_state("first", [0.0, 0.0])],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    let workspace = app.board.panels[0].workspace_id;
    let id = app
        .board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Block,
                block: Some(BlockPanelState::new(BlockDocument {
                    width: None,
                    components: Vec::new(),
                })),
                size: Some([420.0, 220.0]),
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("a card panel spawns");
    app.panel_render_caches
        .card_runtime
        .entry(id)
        .or_default()
        .observe_content_height(measured_content_height);
    (temp, ctx, app, id)
}

fn height(app: &HorizonApp, id: PanelId) -> f32 {
    app.board.panel(id).expect("the card").layout.size[1]
}

#[test]
fn a_card_takes_the_height_of_the_content_that_the_pass_measured() {
    let (_temp, ctx, mut app, id) = app_with_card(600.0);

    app.fit_block_cards(&ctx);

    assert!(
        (height(&app, id) - (600.0 + CARD_CHROME_HEIGHT)).abs() < f32::EPSILON,
        "the card fits its content: {}",
        height(&app, id)
    );
}

#[test]
fn a_card_never_takes_less_than_the_floor_of_a_card() {
    let (_short, ctx, mut short, short_id) = app_with_card(5.0);
    short.fit_block_cards(&ctx);
    assert!((height(&short, short_id) - 140.0).abs() < f32::EPSILON, "the floor");
}

#[test]
fn one_fit_per_document_leaves_a_settled_card_alone() {
    let (_temp, ctx, mut app, id) = app_with_card(600.0);
    app.fit_block_cards(&ctx);
    assert!(app.board.resize_panel(id, [420.0, 320.0]), "the card is resized");

    app.fit_block_cards(&ctx);

    assert!(
        (height(&app, id) - 320.0).abs() < f32::EPSILON,
        "the fitted revision does not resize the card again: {}",
        height(&app, id)
    );
}

#[test]
fn a_new_document_fits_the_card_again() {
    let (_temp, ctx, mut app, id) = app_with_card(600.0);
    app.fit_block_cards(&ctx);

    let state = app.board.panel_mut(id).and_then(Panel::block_mut).expect("a card");
    let _ = state.replace_document(BlockDocument {
        width: None,
        components: Vec::new(),
    });
    app.panel_render_caches
        .card_runtime
        .entry(id)
        .or_default()
        .observe_content_height(300.0);
    app.fit_block_cards(&ctx);

    assert!(
        (height(&app, id) - (300.0 + CARD_CHROME_HEIGHT)).abs() < f32::EPSILON,
        "the next revision fits its own content: {}",
        height(&app, id)
    );
}

#[test]
fn a_card_that_the_person_resized_keeps_its_size() {
    let (_temp, ctx, mut app, id) = app_with_card(600.0);
    app.panel_render_caches
        .card_runtime
        .entry(id)
        .or_default()
        .mark_hand_resized();

    app.fit_block_cards(&ctx);
    assert!(
        (height(&app, id) - 220.0).abs() < f32::EPSILON,
        "the person's size stays"
    );

    let state = app.board.panel_mut(id).and_then(Panel::block_mut).expect("a card");
    let _ = state.replace_document(BlockDocument {
        width: None,
        components: Vec::new(),
    });
    app.panel_render_caches
        .card_runtime
        .entry(id)
        .or_default()
        .observe_content_height(300.0);
    app.fit_block_cards(&ctx);
    assert!(
        (height(&app, id) - 220.0).abs() < f32::EPSILON,
        "a later document does not undo the person's size: {}",
        height(&app, id)
    );
}

#[test]
fn a_card_fits_again_when_a_frame_of_the_host_appears_or_goes() {
    let (_temp, ctx, mut app, id) = app_with_card(600.0);
    app.fit_block_cards(&ctx);
    let fitted = height(&app, id);

    // The owner leaves: Horizon puts its frame at the top of the body, so the
    // body needs those points on top of the document.
    let card = app.panel_render_caches.card_runtime.entry(id).or_default();
    card.observe_frames(CardFrames {
        agent_gone: true,
        clicks_refused: false,
        link_refused: false,
    });
    card.observe_content_height(700.0);
    app.fit_block_cards(&ctx);

    assert!(
        (height(&app, id) - (700.0 + CARD_CHROME_HEIGHT)).abs() < f32::EPSILON,
        "the card grows for the frame of a gone agent: {} was {fitted}",
        height(&app, id)
    );

    // The owner comes back: the frame is gone, so the card takes the height of
    // its document again.
    let card = app.panel_render_caches.card_runtime.entry(id).or_default();
    card.observe_frames(CardFrames::default());
    card.observe_content_height(600.0);
    app.fit_block_cards(&ctx);

    assert!(
        (height(&app, id) - (600.0 + CARD_CHROME_HEIGHT)).abs() < f32::EPSILON,
        "the card drops the room of the frame when the agent is back: {}",
        height(&app, id)
    );
}

#[test]
fn a_card_that_the_person_resized_keeps_its_size_when_a_frame_appears() {
    let (_temp, ctx, mut app, id) = app_with_card(600.0);
    let card = app.panel_render_caches.card_runtime.entry(id).or_default();
    card.mark_hand_resized();
    card.observe_frames(CardFrames {
        agent_gone: true,
        clicks_refused: false,
        link_refused: false,
    });

    app.fit_block_cards(&ctx);

    assert!(
        (height(&app, id) - 220.0).abs() < f32::EPSILON,
        "the person's size outranks the frame: {}",
        height(&app, id)
    );
}

#[test]
fn a_workspace_preset_keeps_the_size_of_its_cards() {
    let (_temp, ctx, mut app, id) = app_with_card(600.0);
    let workspace = app.board.panel(id).expect("the card").workspace_id;
    app.board
        .arrange_workspace(workspace, horizon_core::WorkspaceLayout::Rows);
    let arranged = height(&app, id);
    let sibling = app
        .board
        .workspace(workspace)
        .expect("workspace")
        .panels
        .iter()
        .copied()
        .find(|panel| *panel != id)
        .expect("a sibling panel");
    let sibling_height = height(&app, sibling);

    app.fit_block_cards(&ctx);

    assert!(
        (height(&app, id) - arranged).abs() < f32::EPSILON,
        "the preset owns the size of its panels"
    );
    assert!((height(&app, sibling) - sibling_height).abs() < f32::EPSILON);
}

/// The width of the card panel `id`.
fn width(app: &HorizonApp, id: PanelId) -> f32 {
    app.board.panel(id).expect("the card").layout.size[0]
}

/// A document of `columns` waves, as the board of a plan card draws them.
fn board_document(columns: usize) -> BlockDocument {
    BlockDocument {
        width: Some(Width::Wide),
        components: vec![Component::Board {
            columns: (0..columns)
                .map(|index| BoardColumn {
                    label: format!("wave {index}"),
                    tag: Some(format!("{}", index + 1)),
                    note: None,
                    value: Some("5 m".to_owned()),
                    current: index == 0,
                })
                .collect(),
            rows: vec![BoardRow {
                label: "helper-1 · build-1".to_owned(),
                subtitle: None,
            }],
            items: vec![BoardItem {
                column: 0,
                row: 0,
                title: "Read the issue".to_owned(),
                state: BoardState::Done,
                value: None,
                note: None,
                tag: None,
            }],
        }],
    }
}

/// A document of `columns` waves that asks for the narrow card.
fn narrow_document(columns: usize) -> BlockDocument {
    BlockDocument {
        width: None,
        ..board_document(columns)
    }
}

/// Replaces the document of the card panel `id`.
fn replace(app: &mut HorizonApp, id: PanelId, document: BlockDocument) {
    let state = app.board.panel_mut(id).and_then(Panel::block_mut).expect("a card");
    let _ = state.replace_document(document);
}

/// The view of the window of this test: the whole screen, so that the canvas
/// keeps its width and no sidebar takes room from it.
const VIEW: [f32; 2] = [5120.0, 1200.0];

#[test]
fn a_card_takes_the_width_that_its_document_asks_for_at_every_update() {
    let (_temp, ctx, mut app, id) = app_with_card(600.0);
    app.sidebar_visible = false;

    // The card of a person who did not resize it: the width follows the document
    // at every update, wider and narrower again, and the card grows to the right.
    let left = app.board.panel(id).expect("the card").layout.position[0];
    for (document, expected) in [
        (narrow_document(1), 420.0),
        (board_document(2), 1222.0),
        (board_document(5), 2400.0),
        (board_document(2), 1222.0),
        (narrow_document(1), 420.0),
    ] {
        replace(&mut app, id, document);
        run_app_frame_with_input(&ctx, &mut app, raw_input(VIEW, None));
        assert!(
            (width(&app, id) - expected).abs() < 0.5,
            "the card takes the {expected} points that its document asks for, not {}",
            width(&app, id)
        );
        assert!(
            (app.board.panel(id).expect("the card").layout.position[0] - left).abs() < f32::EPSILON,
            "the left edge stays where the person put it"
        );
    }
}

#[test]
fn a_card_that_the_person_resized_by_hand_keeps_its_size_at_every_update() {
    let (_temp, ctx, mut app, id) = app_with_card(600.0);
    app.sidebar_visible = false;
    replace(&mut app, id, board_document(2));
    run_app_frame_with_input(&ctx, &mut app, raw_input(VIEW, None));
    let card = app.panel_render_caches.card_runtime.entry(id).or_default();
    card.mark_hand_resized();
    assert!(
        app.board.resize_panel(id, [700.0, 500.0]),
        "the person resizes the card"
    );

    for document in [board_document(5), board_document(2), narrow_document(1)] {
        replace(&mut app, id, document);
        run_app_frame_with_input(&ctx, &mut app, raw_input(VIEW, None));
        let size = app.board.panel(id).expect("the card").layout.size;
        assert!(
            (size[0] - 700.0).abs() < f32::EPSILON && (size[1] - 500.0).abs() < f32::EPSILON,
            "the person's own size stays: {size:?}"
        );
    }
}

/// An app whose first panel is an agent, so a card request of that panel applies.
fn agent_app() -> (tempfile::TempDir, Context, HorizonApp) {
    let runtime = RuntimeState {
        workspaces: vec![editor_workspace_state("first", [0.0, 0.0])],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    app.board.panels[0].kind = PanelKind::Claude;
    app.sidebar_visible = false;
    (temp, ctx, app)
}

/// One request of the agent panel of `app`, as the queue carries it.
fn request(app: &HorizonApp, operation: Operation) -> Request {
    Request {
        request_id: "test".into(),
        actor: format!("horizon:{}", app.board.panels[0].local_id),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

/// The id of the card that an answer of `create` names.
fn created_card(outcome: Outcome) -> String {
    match outcome {
        Outcome::Created { panel_id, .. } => panel_id,
        other => panic!("expected a created card: {other:?}"),
    }
}

/// The card that a setup agent draws for a machine that it rented: a heading
/// with a badge, nine pairs, six steps, three lines of text and eight buttons
/// that take three rows of the card width.
fn setup_document() -> serde_json::Value {
    let rows: Vec<serde_json::Value> = [
        ("Kind", "sample.2cpu"),
        ("Address", "203.0.113.10"),
        ("Session", "sample-session"),
        ("Folder", "/work/sample-folder"),
        ("Trusted for", "Sample Agent 1.0"),
        ("Label", "purpose=sample"),
        ("Key from", "any address, key only"),
        ("Rented at", "2026-01-01 10:00 UTC"),
        ("Cost", "0.033 EUR / hour, IPv4 extra"),
    ]
    .iter()
    .map(|(label, value)| serde_json::json!({"label": label, "value": value}))
    .collect();
    let steps: Vec<serde_json::Value> = [
        "Prepare the system",
        "Install the sample",
        "Bring the folder here",
        "Trust the folder",
        "Warm up",
        "Run the check",
    ]
    .iter()
    .map(|label| serde_json::json!({"label": label, "state": "done", "at": "1 s"}))
    .collect();
    serde_json::json!({
        "components": [
            {"type": "heading", "text": "Ready for sample-machine", "subtitle": "sample · example",
             "badge": {"text": "Ready", "tone": "ok"}},
            {"type": "kv", "rows": rows},
            {"type": "steps", "steps": steps},
            {"type": "text", "text": "The check passed.\nThe server and browser buttons continue the setup.\nThe rent is ready."},
            {"type": "buttons", "items": [
                {"id": "start", "label": "Start the machine", "tone": "primary"},
                {"id": "place", "label": "Put the server here", "tone": "neutral"},
                {"id": "browser", "label": "Add a browser", "tone": "neutral"},
                {"id": "network", "label": "Join a network", "tone": "neutral"},
                {"id": "state", "label": "Show the agent state", "tone": "neutral"},
                {"id": "stop", "label": "Stop", "tone": "warn"},
                {"id": "close", "label": "Close", "tone": "neutral"},
                {"id": "delete", "label": "Delete", "tone": "danger"}
            ]}
        ]
    })
}

/// The first document of the same card: the two controls that start a setup.
fn step_document() -> serde_json::Value {
    serde_json::json!({
        "components": [
            {"type": "heading", "text": "Preparing sample-machine", "badge": {"text": "Working", "tone": "info"}},
            {"type": "buttons", "items": [{"id": "start", "label": "Start the machine", "tone": "primary"}]}
        ]
    })
}

/// A document that needs more room than any window shows: every pair and every
/// step that a card holds.
fn taller_than_the_view_document() -> serde_json::Value {
    let rows: Vec<serde_json::Value> = (0..24)
        .map(|index| serde_json::json!({"label": format!("Label {index}"), "value": "value"}))
        .collect();
    let steps: Vec<serde_json::Value> = (0..16)
        .map(|index| serde_json::json!({"label": format!("Step {index}"), "state": "done"}))
        .collect();
    serde_json::json!({
        "components": [
            {"type": "heading", "text": "Ready for sample-machine"},
            {"type": "kv", "rows": rows},
            {"type": "steps", "steps": steps},
            {"type": "buttons", "items": [{"id": "delete", "label": "Delete", "tone": "danger"}]}
        ]
    })
}

/// The ink of the text `text` that a frame drew.
fn drawn_text(output: &egui::FullOutput, text: &str) -> Rect {
    output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::epaint::Shape::Text(shape) if shape.galley.text() == text => {
                Some(Rect::from_min_size(shape.pos, shape.galley.size()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("the frame drew no {text}"))
}

/// The room that the canvas view of this moment shows, in canvas points.
fn room_of_the_view(app: &HorizonApp, ctx: &Context) -> f32 {
    app.canvas_rect(ctx).height() / app.canvas_view.zoom
}

/// The card panel that `card` names, as the canvas draws it this frame.
fn card_panel(app: &HorizonApp, ctx: &Context, card: &str) -> Rect {
    let panel = app
        .board
        .panel(app.board.panel_id_by_local_id(card).expect("the card"))
        .expect("the card");
    let min = app.canvas_to_screen(
        app.canvas_rect(ctx),
        Pos2::new(panel.layout.position[0], panel.layout.position[1]),
    );
    Rect::from_min_size(min, app.canvas_size_to_screen(Vec2::from(panel.layout.size)))
}

/// The room that the body of a card panel draws in: below its titlebar and above
/// the footer that stays at its bottom.
fn card_body(panel: Rect, zoom: f32) -> Rect {
    Rect::from_min_max(
        Pos2::new(panel.min.x, panel.min.y + (CARD_CHROME_HEIGHT - CARD_FOOT_SPAN) * zoom),
        Pos2::new(panel.max.x, panel.max.y - CARD_FOOT_SPAN * zoom),
    )
}

/// The height of the card panel that `card` names.
fn card_height(app: &HorizonApp, card: &str) -> f32 {
    app.board
        .panel(app.board.panel_id_by_local_id(card).expect("the card"))
        .expect("the card")
        .layout
        .size[1]
}

/// Creates the card of a person's window from its first document and lets it
/// settle. Returns the card and the height it took.
fn card_of_first_document(ctx: &Context, app: &mut HorizonApp, window: [f32; 2]) -> (String, f32) {
    let card = created_card(app.apply_block_request(
        &request(
            app,
            Operation::Create {
                title: Some("sample-machine".into()),
                document: step_document(),
            },
        ),
        ctx,
    ));
    for _ in 0..2 {
        let _ = run_app_frame_with_input(ctx, app, raw_input(window, None));
    }
    let height = card_height(app, &card);
    (card, height)
}

/// Updates the card `card` to `document` and runs the frames that the layout
/// needs: one pass measures the body of the new document, and the host gives the
/// card the room that fits it.
fn grow_card(
    ctx: &Context,
    app: &mut HorizonApp,
    window: [f32; 2],
    card: &str,
    document: serde_json::Value,
) -> egui::FullOutput {
    let _ = app.apply_block_request(
        &request(
            app,
            Operation::Update {
                panel_id: card.to_owned(),
                document,
            },
        ),
        ctx,
    );
    let mut output = run_app_frame_with_input(ctx, app, raw_input(window, None));
    for _ in 0..2 {
        output = run_app_frame_with_input(ctx, app, raw_input(window, None));
    }
    output
}

#[test]
fn a_card_that_grows_by_an_update_shows_its_last_button_without_a_scroll() {
    let (_temp, ctx, mut app) = agent_app();
    let (card, opened) = card_of_first_document(&ctx, &mut app, VIEW);

    let output = grow_card(&ctx, &mut app, VIEW, &card, setup_document());

    let panel = card_panel(&app, &ctx, &card);
    let body = card_body(panel, app.canvas_view.zoom);
    let ink = drawn_text(&output, "Delete");
    assert!(
        body.contains_rect(ink),
        "the last button of the card lies inside the room that the card gives its body: {ink:?} in {body:?}"
    );
    let grown = card_height(&app, &card);
    assert!(
        grown > opened && grown > CARD_HEIGHT_LIMITS[1],
        "the card grew to the room that its document needs, past the ceiling of a narrow card: {opened} to {grown}"
    );
}

#[test]
fn a_card_that_the_person_resized_by_hand_keeps_its_size_when_it_grows() {
    let (_temp, ctx, mut app) = agent_app();
    let (card, _) = card_of_first_document(&ctx, &mut app, VIEW);
    let id = app.board.panel_id_by_local_id(&card).expect("the card");
    let card_of_a_person = app.panel_render_caches.card_runtime.entry(id).or_default();
    card_of_a_person.mark_hand_resized();
    assert!(
        app.board.resize_panel(id, [420.0, 500.0]),
        "the person resizes the card"
    );

    let _ = grow_card(&ctx, &mut app, VIEW, &card, setup_document());

    assert!(
        (card_height(&app, &card) - 500.0).abs() < f32::EPSILON,
        "the person's own size outranks the document: {}",
        card_height(&app, &card)
    );
}

#[test]
fn a_card_that_is_taller_than_the_room_of_the_view_scrolls_in_its_body() {
    let (_temp, ctx, mut app) = agent_app();
    let (card, _) = card_of_first_document(&ctx, &mut app, VIEW);
    let room = room_of_the_view(&app, &ctx);

    let output = grow_card(&ctx, &mut app, VIEW, &card, taller_than_the_view_document());

    let height = card_height(&app, &card);
    assert!(
        (height - room.max(CARD_HEIGHT_LIMITS[1])).abs() < f32::EPSILON,
        "a content that no window shows keeps a card at the room of the view: {height} of {room}"
    );
    let panel = card_panel(&app, &ctx, &card);
    let body = card_body(panel, app.canvas_view.zoom);
    let ink = drawn_text(&output, "Delete");
    assert!(
        ink.min.y > body.max.y,
        "the body scrolls the last button below the room that the card gives it: {ink:?} in {body:?}"
    );
}

/// The window of the person who read a card that its ceiling cut: the canvas
/// view shows about 854 points of it.
const PERSON_WINDOW: [f32; 2] = [1480.0, 900.0];

/// The setup document with two more step rows, so that its body needs more room
/// than the 900-point bound of a narrow card.
fn longer_than_the_bound_document() -> serde_json::Value {
    let mut document = setup_document();
    let steps = document["components"][2]["steps"]
        .as_array_mut()
        .expect("the steps of the setup document");
    for label in ["Read the key of the machine", "Write the sample file"] {
        steps.push(serde_json::json!({"label": label, "state": "done", "at": "1 s"}));
    }
    document
}

/// Whether the frame drew the words `text`.
fn drew_text(output: &egui::FullOutput, text: &str) -> bool {
    output
        .shapes
        .iter()
        .any(|clipped| matches!(&clipped.shape, egui::epaint::Shape::Text(shape) if shape.galley.text() == text))
}

/// One frame of the person's window that carries `events`.
fn window_input(window: [f32; 2], position: Pos2, events: Vec<egui::Event>) -> egui::RawInput {
    let mut input = raw_input(window, None);
    input.events.push(egui::Event::PointerMoved(position));
    input.events.extend(events);
    input
}

/// A primary press (`true`) or release (`false`) at `position`.
fn primary_button(position: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

/// Presses the pointer at `position` of the person's window, in the frames that
/// egui needs for a click.
fn press_at(ctx: &Context, app: &mut HorizonApp, window: [f32; 2], position: Pos2) {
    for events in [
        Vec::new(),
        vec![primary_button(position, true)],
        vec![primary_button(position, false)],
    ] {
        let _ = run_app_frame_with_input(ctx, app, window_input(window, position, events));
    }
}

#[test]
fn a_card_that_holds_more_than_it_shows_marks_the_lower_edge_of_its_body() {
    let (_temp, ctx, mut app) = agent_app();
    let (card, _) = card_of_first_document(&ctx, &mut app, PERSON_WINDOW);
    let _ = run_app_frame_with_input(&ctx, &mut app, raw_input(PERSON_WINDOW, None));
    let fitted = run_app_frame_with_input(&ctx, &mut app, raw_input(PERSON_WINDOW, None));
    assert!(
        !drew_text(&fitted, "More below"),
        "a card whose body holds all of its content draws no mark"
    );

    let _ = grow_card(&ctx, &mut app, PERSON_WINDOW, &card, longer_than_the_bound_document());
    // The card is taller than the view, so the person's view rests with its foot
    // at the lower edge of the canvas.
    let id = app.board.panel_id_by_local_id(&card).expect("the card");
    let canvas = app.canvas_rect(&ctx);
    let foot = app.screen_to_canvas(canvas, canvas.max).y - card_height(&app, &card);
    assert!(
        app.board.move_panel(id, [20.0, foot]),
        "the person moves the view to the foot of the card"
    );
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input(PERSON_WINDOW, None));

    let panel = card_panel(&app, &ctx, &card);
    let body = card_body(panel, app.canvas_view.zoom);
    let mark = drawn_text(&output, "More below");
    assert!(
        body.contains_rect(mark),
        "the mark stands inside the lower edge of the body: {mark:?} in {body:?}"
    );
    assert!(
        app.canvas_rect(&ctx).contains_rect(mark),
        "the mark stands in the view of the person: {mark:?}"
    );
    assert!(
        mark.min.y > body.center().y,
        "the mark stands at the lower edge of the body, not in its middle: {mark:?} in {body:?}"
    );
    let last = drawn_text(&output, "Delete");
    assert!(
        last.min.y > body.max.y,
        "the body holds the last button under its lower edge: {last:?} in {body:?}"
    );

    press_at(&ctx, &mut app, PERSON_WINDOW, mark.center());
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input(PERSON_WINDOW, None));

    assert!(
        !drew_text(&output, "More below"),
        "the mark is gone once the body shows its end"
    );
    let last = drawn_text(&output, "Delete");
    assert!(
        body.contains_rect(last),
        "the press showed the last button of the card: {last:?} in {body:?}"
    );
}
