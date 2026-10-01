use egui::Context;
use horizon_core::PanelId;

use super::*;
use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app};

fn frame(ctx: &Context, app: &mut HorizonApp) {
    run_app_frame_with_input(ctx, app, raw_input([1400.0, 900.0], None));
}

#[test]
fn toggle_opens_then_closes_and_returns_focus() {
    let (_temp, mut app) = test_app();
    let previous = PanelId(7);
    app.board.focused = Some(previous);

    app.toggle_assistant();
    assert!(app.assistant.open);
    assert!(app.assistant_has_keyboard());
    assert_eq!(app.board.focused, None, "the drawer takes the keyboard from the canvas");

    app.toggle_assistant();
    assert!(!app.assistant.open);
    assert!(!app.assistant_has_keyboard());
    // The remembered panel no longer exists on this empty board, so nothing is restored.
    assert_eq!(app.board.focused, None);
}

#[test]
fn clicking_a_canvas_panel_takes_the_keyboard_back() {
    let (_temp, mut app) = test_app();
    app.toggle_assistant();
    assert!(app.assistant_has_keyboard());

    app.board.focused = Some(PanelId(3));
    app.sync_assistant_focus();

    assert!(!app.assistant_has_keyboard());
}

#[test]
fn settings_and_fullscreen_hide_the_drawer_without_closing_it() {
    let (_temp, mut app) = test_app();
    app.toggle_assistant();
    assert!(app.assistant_visible());

    app.toggle_settings();
    assert!(!app.assistant_visible());
    assert!(app.assistant.open, "the agent and drawer state survive");
    app.toggle_settings();
    assert!(app.assistant_visible());

    app.fullscreen_panel = Some(PanelId(1));
    assert!(!app.assistant_visible());
}

#[test]
fn drawer_takes_width_from_the_canvas_and_gives_it_back() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    frame(&ctx, &mut app);
    let closed = app.canvas_rect(&ctx);

    app.toggle_assistant();
    frame(&ctx, &mut app);
    let open = app.canvas_rect(&ctx);
    assert!(
        closed.max.x - open.max.x >= MIN_WIDTH,
        "canvas should lose at least the drawer's minimum width: {closed:?} vs {open:?}"
    );
    assert!(app.assistant_right_inset(&ctx) >= MIN_WIDTH);

    app.toggle_assistant();
    frame(&ctx, &mut app);
    assert!((app.canvas_rect(&ctx).max.x - closed.max.x).abs() < 0.5);
    assert!(app.assistant_right_inset(&ctx).abs() < f32::EPSILON);
}

#[test]
fn opening_starts_one_hidden_assistant_panel_only_once() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    app.toggle_assistant();
    frame(&ctx, &mut app);
    frame(&ctx, &mut app);

    let id = app.board.assistant_panel().expect("assistant should be created");
    let panel = app.board.panel(id).expect("panel");
    assert!(panel.is_assistant());
    assert!(!panel.visible, "the assistant is never drawn on the canvas");
    assert_eq!(app.board.panels.iter().filter(|panel| panel.is_assistant()).count(), 1);
}

#[test]
fn restart_replaces_the_agent_at_the_next_drawer_render() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    app.toggle_assistant();
    frame(&ctx, &mut app);
    let first = app.board.assistant_panel().expect("assistant should be created");

    app.restart_assistant();
    frame(&ctx, &mut app);
    frame(&ctx, &mut app);

    let second = app.board.assistant_panel().expect("assistant should be recreated");
    assert_ne!(first, second);
}

fn first_card_kind(app: &HorizonApp) -> Option<&cards::CardKind> {
    app.assistant.cards.first_kind()
}

#[test]
fn a_note_opens_the_drawer_and_shows_one_card() {
    let (_temp, mut app) = test_app();
    assert!(!app.assistant.open);
    app.assistant_post_note("Summary".to_string(), "- **done**".to_string());
    assert!(app.assistant.open, "a note must be visible to the person");
    assert!(matches!(
        first_card_kind(&app),
        Some(cards::CardKind::Note { title, .. }) if title == "Summary"
    ));
}

#[test]
fn declining_an_approval_types_nothing_and_says_so() {
    let (_temp, mut app) = test_app();
    app.assistant_request_approval(PanelId(5), "codex".to_string(), "run the tests".to_string(), true);
    assert!(app.assistant.open, "an approval must be visible to the person");
    let id = app.assistant.cards.first_id().expect("card");

    app.resolve_approval(id, false);

    assert!(matches!(
        first_card_kind(&app),
        Some(cards::CardKind::Declined { reason, .. }) if reason.contains("declined")
    ));
}

#[test]
fn approving_for_an_agent_that_is_gone_is_declined_with_the_reason() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    app.toggle_assistant();
    frame(&ctx, &mut app);
    app.assistant_request_approval(PanelId(424_242), "ghost".to_string(), "hello".to_string(), true);
    let id = app.assistant.cards.first_id().expect("card");

    app.resolve_approval(id, true);

    assert!(matches!(first_card_kind(&app), Some(cards::CardKind::Declined { .. })));
}

#[test]
fn asking_before_sending_is_on_by_default() {
    let (_temp, app) = test_app();
    assert!(app.assistant_asks_before_send());
}
