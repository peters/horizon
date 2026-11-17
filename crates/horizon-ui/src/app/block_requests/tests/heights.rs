//! A card has the height of its content, between the least and the greatest
//! height of a card, in a workspace that a preset arranges too.

use egui::{Event, Vec2};
use horizon_core::WorkspaceLayout;

use super::columns::{AGENT, GAP, WINDOW, agent_board, place, started, workspace};
use super::*;
use crate::app::test_support::{card_body, card_panel, drawn_text, drew_text, editor_panel_state};

/// The smallest window that the build opens.
const SMALL_WINDOW: [f32; 2] = [800.0, 600.0];
/// The greatest height of a card of the default width.
const GREATEST: f32 = 560.0;
/// The least height of a card.
const LEAST: f32 = 140.0;

/// A document of `rows` pairs with one button under them.
fn rows_and_button(heading: &str, rows: usize) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = (0..rows)
        .map(|index| serde_json::json!({"label": format!("Label {index}"), "value": "value"}))
        .collect();
    serde_json::json!({"components": [
        {"type": "heading", "text": heading},
        {"type": "kv", "rows": rows},
        {"type": "buttons", "items": [{"id": "delete", "label": "Delete", "tone": "danger"}]}
    ]})
}

/// The frames of a window of `window` that measure a document and fit its card.
fn frames(ctx: &Context, app: &mut HorizonApp, window: [f32; 2]) -> egui::FullOutput {
    for _ in 0..3 {
        let _ = run_app_frame_with_input(ctx, app, raw_input(window, None));
    }
    run_app_frame_with_input(ctx, app, raw_input(window, None))
}

/// Draws one card of the agent panel in a window of `window`.
fn draw_in(ctx: &Context, app: &mut HorizonApp, window: [f32; 2], document: serde_json::Value) -> String {
    let (card, _) = created(app.apply_block_request(&request(app, Operation::Create { title: None, document }), ctx));
    let _ = frames(ctx, app, window);
    card
}

fn height(app: &HorizonApp, card: &str) -> f32 {
    place(app, card)[3] - place(app, card)[1]
}

#[test]
fn a_card_of_two_lines_is_lower_than_a_card_of_twenty_rows_in_the_grid_of_a_new_workspace() {
    let (_temp, ctx, mut app) = agent_board(Some(WorkspaceLayout::Grid));
    let two_lines = serde_json::json!({"components": [
        {"type": "heading", "text": "short"}, {"type": "text", "text": "One line.\nA second line."}
    ]});

    let short = draw_in(&ctx, &mut app, WINDOW, two_lines);
    let output = frames(&ctx, &mut app, WINDOW);
    assert!(
        !drew_text(&output, "More below"),
        "a card that fits its content draws no mark"
    );
    let long = draw_in(&ctx, &mut app, WINDOW, rows_and_button("long", 20));

    let (short_height, long_height) = (height(&app, &short), height(&app, &long));
    assert!(
        (LEAST..240.0).contains(&short_height) && long_height > 2.0 * short_height && long_height <= GREATEST,
        "each card has the height of its own content: {short_height} and {long_height}"
    );
}

#[test]
fn a_card_above_the_greatest_height_keeps_that_height_in_every_window_and_scrolls_to_its_buttons() {
    for window in [SMALL_WINDOW, WINDOW, [5120.0, 1400.0]] {
        let (_temp, ctx, mut app) = started(workspace(None, vec![editor_panel_state(AGENT, [20.0, 20.0])]));
        app.template_config.canvas.fit_floor_zoom = 1.0;
        let card = draw_in(&ctx, &mut app, window, rows_and_button("tall", 24));

        assert!(
            (height(&app, &card) - GREATEST).abs() < f32::EPSILON,
            "the card has the greatest height of a card in a window of {window:?}: {}",
            height(&app, &card)
        );
        // The person looks at the card. The view of every window holds its title
        // bar and its body at zoom 1; the smallest window cuts the last 16 points
        // of the body, which the mark of the card brings into the view.
        let id = app.board.panel_id_by_local_id(&card).expect("the card");
        app.reveal_panel_visible(&ctx, id);
        let output = frames(&ctx, &mut app, window);
        let body = card_body(card_panel(&app, &ctx, &card), app.canvas_view.zoom);
        let canvas = app.canvas_rect(&ctx);
        assert!(
            (app.canvas_view.zoom - 1.0).abs() < f32::EPSILON
                && canvas.contains(body.min)
                && body.max.y - canvas.max.y <= 16.5,
            "the view of {window:?} shows the body of the card: {body:?} in {canvas:?} at zoom {}",
            app.canvas_view.zoom
        );
        assert!(
            drew_text(&output, "More below"),
            "the card says that it holds more ({window:?})"
        );
        let button = drawn_text(&output, "Delete");
        assert!(
            button.min.y > body.max.y,
            "the body cuts the button: {button:?} in {body:?}"
        );

        // The wheel over the body brings the button of the card into it.
        let mut input = raw_input(window, None);
        input.events = vec![
            Event::PointerMoved(body.center()),
            Event::MouseWheel {
                source: egui::MouseWheelSource::Wheel,
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -4_000.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        let _ = run_app_frame_with_input(&ctx, &mut app, input);
        let output = frames(&ctx, &mut app, window);
        let body = card_body(card_panel(&app, &ctx, &card), app.canvas_view.zoom);
        let button = drawn_text(&output, "Delete");
        assert!(
            body.contains_rect(button),
            "the wheel reaches the button of the card ({window:?}): {button:?} in {body:?}"
        );
        assert!(
            drew_text(&output, "More below") != app.canvas_rect(&ctx).contains_rect(body),
            "the mark goes at the end of a body that the view shows whole ({window:?})"
        );
    }
}

#[test]
fn an_update_that_adds_rows_makes_the_card_higher_and_moves_the_card_under_it() {
    for layout in [None, Some(WorkspaceLayout::Grid)] {
        let (_temp, ctx, mut app) = agent_board(layout);
        let under = draw_in(&ctx, &mut app, WINDOW, rows_and_button("under", 2));
        let over = draw_in(&ctx, &mut app, WINDOW, rows_and_button("over", 2));
        let (card_before, under_before) = (place(&app, &over), place(&app, &under));

        let updated = app.apply_block_request(
            &request(
                &app,
                Operation::Update {
                    panel_id: over.clone(),
                    document: rows_and_button("over", 8),
                },
            ),
            &ctx,
        );
        assert!(
            matches!(updated, Outcome::Updated { .. }),
            "the agent updates its card: {updated:?}"
        );
        let _ = frames(&ctx, &mut app, WINDOW);

        let (card_after, under_after) = (place(&app, &over), place(&app, &under));
        assert!(
            card_after[3] > card_before[3] + 100.0 && card_after[1] == card_before[1],
            "the card grew down with its content ({layout:?}): {card_before:?} to {card_after:?}"
        );
        assert!(
            (under_after[1] - (card_after[3] + GAP)).abs() < 0.5 && under_after[1] > under_before[1],
            "the card under it moved down ({layout:?}): {under_before:?} to {under_after:?}"
        );
    }
}
