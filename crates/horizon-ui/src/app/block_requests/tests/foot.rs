//! The buttons at the end of a card that scrolls stay at its foot.
//!
//! Every lane draws the card of a machine through the request of an agent panel,
//! runs whole frames of the application and reads what the frame drew.

use egui::{Event, Rect, Vec2};

use super::columns::{WINDOW, agent_board, pointer};
use super::*;
use crate::app::test_support::{card_body, card_panel, drawn_text, drew_text};

/// The smallest window that the build opens.
const SMALL_WINDOW: [f32; 2] = [800.0, 600.0];
/// A window that shows a card of the greatest height whole at zoom 1.
const TALL_WINDOW: [f32; 2] = [1600.0, 1300.0];
/// The labels of the buttons of the card, in document order.
const BUTTONS: [&str; 9] = [
    "Connect",
    "Start Codex here",
    "Stop",
    "Restart",
    "Rebuild",
    "Read the log",
    "Rename",
    "Add a key",
    "Delete",
];
/// The words that follow the buttons in the document.
const LAST_WORDS: &str = "The machine costs money while it runs.";

/// The card of a machine: `rows` pairs in two tables, two components of buttons
/// and one line of text at its end.
fn machine(rows: usize) -> serde_json::Value {
    let table = |from: usize, to: usize| -> Vec<serde_json::Value> {
        (from..to)
            .map(|index| serde_json::json!({"label": format!("Label {index}"), "value": "value"}))
            .collect()
    };
    // The id of a button is its place among the buttons of the card.
    let button = |index: usize, tone: &str| serde_json::json!({"id": format!("b{index}"), "label": BUTTONS[index], "tone": tone});
    let mut more: Vec<serde_json::Value> = (2..8).map(|index| button(index, "neutral")).collect();
    more.push(button(8, "danger"));
    serde_json::json!({"components": [
        {"type": "heading", "text": "build-1"},
        {"type": "kv", "rows": table(0, rows / 2)},
        {"type": "kv", "rows": table(rows / 2, rows)},
        {"type": "buttons", "items": [button(0, "primary"), button(1, "neutral")]},
        {"type": "buttons", "items": more},
        {"type": "text", "text": LAST_WORDS}
    ]})
}

/// One frame of a window of `window` with the events `events`.
fn frame(ctx: &Context, app: &mut HorizonApp, window: [f32; 2], events: Vec<Event>) -> egui::FullOutput {
    let mut input = raw_input(window, None);
    input.events = events;
    run_app_frame_with_input(ctx, app, input)
}

/// The frames that measure a document and fit its card, and the frame after them.
fn frames(ctx: &Context, app: &mut HorizonApp, window: [f32; 2]) -> egui::FullOutput {
    for _ in 0..3 {
        let _ = frame(ctx, app, window, Vec::new());
    }
    frame(ctx, app, window, Vec::new())
}

/// Draws the card of a machine with `rows` pairs in a window of `window`, and
/// brings it into the view as a person who looks at it.
fn draw(ctx: &Context, app: &mut HorizonApp, window: [f32; 2], rows: usize) -> (String, egui::FullOutput) {
    let document = machine(rows);
    let (card, _) = created(app.apply_block_request(&request(app, Operation::Create { title: None, document }), ctx));
    let _ = frames(ctx, app, window);
    let id = app.board.panel_id_by_local_id(&card).expect("the card");
    app.reveal_panel_visible(ctx, id);
    (card, frames(ctx, app, window))
}

/// The part of the body of the card `card` that the window shows.
fn visible_body(ctx: &Context, app: &HorizonApp, card: &str) -> Rect {
    card_body(card_panel(app, ctx, card), app.canvas_view.zoom).intersect(app.canvas_rect(ctx))
}

/// The ink of the label of every button of the card, in document order.
fn buttons(output: &egui::FullOutput) -> Vec<Rect> {
    BUTTONS.iter().map(|label| drawn_text(output, label)).collect()
}

/// One press of the person at the centre of `at`, and the frame after it.
fn press(ctx: &Context, app: &mut HorizonApp, window: [f32; 2], at: Rect) -> egui::FullOutput {
    for events in [
        vec![Event::PointerMoved(at.center())],
        vec![pointer(at.center(), true)],
        vec![pointer(at.center(), false)],
    ] {
        let _ = frame(ctx, app, window, events);
    }
    frames(ctx, app, window)
}

/// One turn of the wheel over `at`, by `points` of the content, and the frame after it.
fn wheel(ctx: &Context, app: &mut HorizonApp, window: [f32; 2], at: Rect, points: f32) -> egui::FullOutput {
    let turn = Event::MouseWheel {
        source: egui::MouseWheelSource::Wheel,
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -points),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    };
    let _ = frame(ctx, app, window, vec![Event::PointerMoved(at.center()), turn]);
    frames(ctx, app, window)
}

/// The id of the button whose press the card holds for its agent.
fn next_click(ctx: &Context, app: &mut HorizonApp, card: &str) -> String {
    let operation = Operation::NextClick {
        panel_id: Some(card.to_owned()),
        timeout_millis: Some(0),
    };
    match app.apply_block_request(&request(app, operation), ctx) {
        Outcome::Clicked { click } => click.component_id,
        other => panic!("the card holds no click: {other:?}"),
    }
}

/// The label of every control of the frame, in the order that a reader walks them.
fn heard(output: &egui::FullOutput) -> Vec<&str> {
    let update = output.platform_output.accesskit_update.as_ref().expect("the controls");
    let node = |id| update.nodes.iter().find(|(node, _)| *node == id).map(|(_, node)| node);
    let mut walk = vec![update.tree.as_ref().expect("the tree of the window").root];
    let mut labels = Vec::new();
    while let Some(id) = walk.pop() {
        let Some(node) = node(id) else { continue };
        labels.extend(node.label());
        walk.extend(node.children().iter().rev());
    }
    labels
}

/// The height that the content of the card `card` needs, as its body measured it.
fn content_height(app: &HorizonApp, card: &str) -> f32 {
    let id = app.board.panel_id_by_local_id(card).expect("the card");
    app.panel_render_caches
        .card_runtime
        .get(&id)
        .and_then(horizon_core::CardRuntime::measured_content_height)
        .expect("the body measured its content")
}

#[test]
fn the_buttons_at_the_end_of_a_card_that_scrolls_stand_in_its_foot_with_no_wheel() {
    let (_temp, ctx, mut app) = agent_board(None);
    let (card, output) = draw(&ctx, &mut app, SMALL_WINDOW, 30);
    let body = visible_body(&ctx, &app, &card);
    let foot = buttons(&output);

    for (label, ink) in BUTTONS.iter().zip(&foot) {
        assert!(
            body.contains_rect(*ink),
            "the window shows the button {label} with no wheel: {ink:?} in {body:?}"
        );
    }
    let mark = drawn_text(&output, "More below");
    assert!(
        mark.max.y <= foot[0].min.y && body.contains_rect(mark),
        "the mark stands above the foot: {mark:?} above {:?}",
        foot[0]
    );
    assert!(
        body.contains_rect(drawn_text(&output, "Label 0")),
        "the body shows its first rows above the foot"
    );

    // A press on a button of the foot is a click for the agent of the card.
    let _ = press(&ctx, &mut app, SMALL_WINDOW, foot[1]);
    assert_eq!(next_click(&ctx, &mut app, &card), "b1");
    let output = press(&ctx, &mut app, SMALL_WINDOW, foot[8]);
    assert_eq!(next_click(&ctx, &mut app, &card), "b8");

    // The wheel moves the rows, and the buttons keep their place.
    let first_row = drawn_text(&output, "Label 2");
    let output = wheel(&ctx, &mut app, SMALL_WINDOW, first_row, 60.0);
    let moved = drawn_text(&output, "Label 2");
    assert!(
        first_row.min.y - moved.min.y > 20.0,
        "the wheel moves the rows of the body: {first_row:?} to {moved:?}"
    );
    assert_eq!(buttons(&output), foot, "the buttons keep their place");

    // The words after the buttons end the part that scrolls, above the foot.
    let output = wheel(&ctx, &mut app, SMALL_WINDOW, moved, 4_000.0);
    let words = drawn_text(&output, LAST_WORDS);
    assert!(
        body.contains_rect(words) && words.max.y <= foot[0].min.y,
        "the words after the buttons end the part that scrolls: {words:?} above {:?}",
        foot[0]
    );
    assert_eq!(buttons(&output), foot, "the buttons keep their place at the end");
    assert!(!drew_text(&output, "More below"), "a body at its end draws no mark");
}

/// The ink of every word that the frame drew in `panel`, with its place from the
/// upper left corner of `panel`, in the order of the frame.
fn ink_in(output: &egui::FullOutput, panel: Rect) -> Vec<(String, [f32; 4])> {
    output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            // The time of the footer follows the clock of the run.
            egui::epaint::Shape::Text(shape)
                if panel.contains(shape.pos) && !shape.galley.text().starts_with("updated ") =>
            {
                let at = shape.pos - panel.min;
                let size = shape.galley.size();
                let hundredths = |value: f32| (value * 100.0).round() / 100.0;
                Some((
                    shape.galley.text().to_owned(),
                    [
                        hundredths(at.x),
                        hundredths(at.y),
                        hundredths(size.x),
                        hundredths(size.y),
                    ],
                ))
            }
            _ => None,
        })
        .collect()
}

/// The place of every control of the machine card of four pairs, which fits its
/// window, as the commit before the foot drew it.
const FITTED: [(&str, [f32; 4]); 22] = [
    ("Card", [26.0, 9.0, 30.03, 16.0]),
    ("Claude", [310.19, 10.0, 37.81, 14.0]),
    ("build-1", [24.0, 58.0, 59.41, 22.0]),
    ("Label 0", [36.0, 112.0, 43.0, 15.0]),
    ("value", [352.31, 112.0, 31.69, 15.0]),
    ("Label 1", [36.0, 146.0, 40.66, 15.0]),
    ("value", [352.31, 146.0, 31.69, 15.0]),
    ("Label 2", [36.0, 204.0, 43.0, 15.0]),
    ("value", [352.31, 204.0, 31.69, 15.0]),
    ("Label 3", [36.0, 238.0, 43.0, 15.0]),
    ("value", [352.31, 238.0, 31.69, 15.0]),
    ("Connect", [40.0, 298.0, 53.25, 16.0]),
    ("Start Codex here", [129.25, 298.0, 103.78, 16.0]),
    ("Stop", [36.11, 358.0, 28.0, 16.0]),
    ("Restart", [96.22, 358.0, 43.59, 16.0]),
    ("Rebuild", [171.81, 358.0, 45.66, 16.0]),
    ("Read the log", [249.47, 358.0, 76.88, 16.0]),
    ("Rename", [36.0, 402.0, 49.69, 16.0]),
    ("Add a key", [117.69, 402.0, 60.97, 16.0]),
    ("Delete", [344.8, 402.0, 39.0, 16.0]),
    ("The machine costs money while it runs.", [24.0, 452.0, 233.5, 19.0]),
    ("Card drawn by the agent Claude", [24.0, 495.5, 168.13, 13.0]),
];

#[test]
fn a_card_that_fits_draws_every_control_where_it_stood_before_the_foot() {
    let (_temp, ctx, mut app) = agent_board(None);
    let (card, output) = draw(&ctx, &mut app, WINDOW, 4);
    assert!(
        (app.canvas_view.zoom - 1.0).abs() < f32::EPSILON && !drew_text(&output, "More below"),
        "the card fits its window at zoom {}",
        app.canvas_view.zoom
    );

    let drawn = ink_in(&output, card_panel(&app, &ctx, &card));
    let words = |ink: &[(String, [f32; 4])]| -> Vec<String> { ink.iter().map(|(words, _)| words.clone()).collect() };
    let expected: Vec<(String, [f32; 4])> = FITTED.iter().map(|(words, at)| ((*words).to_owned(), *at)).collect();
    assert_eq!(
        words(&drawn),
        words(&expected),
        "the card draws the words of its document"
    );
    for ((words, at), (_, before)) in drawn.iter().zip(&expected) {
        assert!(
            at.iter().zip(before).all(|(now, before)| (now - before).abs() < 0.02),
            "no control of a card that fits moved: {words} at {at:?}, before at {before:?}"
        );
    }
}

#[test]
fn the_foot_draws_the_rows_of_buttons_as_the_body_does_and_leaves_the_measure_alone() {
    // The card that fits draws the two components of buttons in its body.
    let (_temp, ctx, mut app) = agent_board(None);
    let (fits, output) = draw(&ctx, &mut app, TALL_WINDOW, 4);
    let in_body = buttons(&output);
    let short = content_height(&app, &fits);

    let (_temp, ctx, mut app) = agent_board(None);
    ctx.enable_accesskit();
    let (card, output) = draw(&ctx, &mut app, TALL_WINDOW, 30);
    let in_foot = buttons(&output);
    let body = visible_body(&ctx, &app, &card);
    assert!(
        drew_text(&output, "More below") && (app.canvas_view.zoom - 1.0).abs() < f32::EPSILON,
        "the card of thirty pairs scrolls at zoom {}",
        app.canvas_view.zoom
    );
    assert!(
        in_foot.iter().all(|ink| body.contains_rect(*ink)),
        "the window shows every button with no wheel: {in_foot:?} in {body:?}"
    );

    // The rows wrap, the dangerous button keeps the right edge, and the two
    // components keep their gap: every button stands at the same place from the
    // first one.
    let from_first = |rects: &[Rect]| -> Vec<Vec2> { rects.iter().map(|rect| rect.min - rects[0].min).collect() };
    assert_eq!(
        from_first(&in_foot),
        from_first(&in_body),
        "the foot draws the rows of the body"
    );
    assert!(
        in_foot[8].min.y > in_foot[2].min.y && in_foot[2].min.y > in_foot[0].min.y,
        "the buttons take three rows or more: {in_foot:?}"
    );

    // Each pair takes the same height, so 26 more pairs are the whole difference
    // of the measure: the foot adds nothing to it and takes nothing from it.
    let tall = content_height(&app, &card);
    let pair = (tall - short) / 26.0;
    assert!(
        (pair - pair.round()).abs() < 0.01 && (30.0..40.0).contains(&pair),
        "the measure of a card with a foot is the measure of its document: {short} and {tall}"
    );
    // The first pass of a card draws no foot, because no pass measured its
    // content before it: its measure is the measure of the passes with a foot.
    let (_first_temp, first_ctx, mut first_app) = agent_board(None);
    let create = Operation::Create {
        title: None,
        document: machine(30),
    };
    let (first, _) = created(first_app.apply_block_request(&request(&first_app, create), &first_ctx));
    let _ = frame(&first_ctx, &mut first_app, TALL_WINDOW, Vec::new());
    assert!(
        (content_height(&first_app, &first) - tall).abs() < 0.01,
        "a body measures the same content with a foot and without one: {} and {tall}",
        content_height(&first_app, &first)
    );

    // A reader hears the controls in the order of the document.
    let heard = heard(&output);
    let buttons_heard: Vec<&str> = heard.iter().copied().filter(|label| BUTTONS.contains(label)).collect();
    assert_eq!(
        buttons_heard, BUTTONS,
        "the buttons keep the order of the document: {heard:?}"
    );

    // Without its agent, the foot draws its buttons off, and a press is no click.
    app.board.panels[0].kind = PanelKind::Editor;
    let output = frames(&ctx, &mut app, TALL_WINDOW);
    assert_eq!(buttons(&output)[1..], in_foot[1..], "the foot stays where it was");
    let _ = press(&ctx, &mut app, TALL_WINDOW, in_foot[1]);
    let id = app.board.panel_id_by_local_id(&card).expect("the card");
    let queued = app
        .panel_render_caches
        .card_runtime
        .get(&id)
        .map(horizon_core::CardRuntime::queued);
    assert_eq!(queued, Some(0), "a button of a card without its agent takes no press");
}

#[test]
fn a_foot_that_would_take_more_than_half_of_the_card_is_not_pinned() {
    let (_temp, ctx, mut app) = agent_board(None);
    let (card, _) = draw(&ctx, &mut app, WINDOW, 30);
    // The person makes the card low: its buttons would take more than half of it.
    let id = app.board.panel_id_by_local_id(&card).expect("the card");
    let width = app.board.panel(id).expect("the card").layout.size[0];
    let _ = app.board.resize_panel(id, [width, 240.0]);
    if let Some(runtime) = app.panel_render_caches.card_runtime.get_mut(&id) {
        runtime.mark_hand_resized();
    }
    let output = frames(&ctx, &mut app, WINDOW);

    let body = visible_body(&ctx, &app, &card);
    assert!(
        drew_text(&output, "Label 0") && drew_text(&output, "More below"),
        "the low card shows its first rows and scrolls"
    );
    assert!(
        !body.contains_rect(drawn_text(&output, BUTTONS[8])),
        "the buttons scroll with the body of a card that is too low for a foot"
    );
}
