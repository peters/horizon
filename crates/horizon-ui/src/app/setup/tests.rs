//! The action of Set up, its notice and its second press, on a window that no
//! person sees.

use egui::Context;
use horizon_core::{PanelKind, SetupGoal};

use crate::app::test_support::test_app_with_startup;

use super::notice::{self, ProgramRow};

/// A window with the integrations off, so a press of Set up draws its notice
/// without a search of this computer's programs.
fn window_without_integrations() -> (tempfile::TempDir, Context, crate::app::HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(horizon_core::StartupDecision::Ephemeral {
        runtime_state: Box::new(horizon_core::RuntimeState::default()),
    });
    app.setup.integrations = false;
    (temp, ctx, app)
}

#[test]
fn set_up_with_the_integrations_off_draws_the_notice_of_horizon() {
    let (_temp, ctx, mut app) = window_without_integrations();
    let panels_before = app.board.panels.len();

    app.press_set_up(&ctx, SetupGoal::Rent);

    assert_eq!(app.board.panels.len(), panels_before + 1, "the notice is a panel");
    let panel = app
        .board
        .panels
        .iter()
        .find(|panel| panel.kind == PanelKind::Block)
        .expect("the notice is a card");
    let block = panel.block().expect("the card of the notice");
    assert_eq!(block.title, notice::TITLE);
    assert_eq!(block.owner_actor, None, "Horizon owns the notice, not an agent");
    assert!(
        app.card_host_owned(panel.id),
        "the buttons of the notice are Horizon's own controls"
    );
    let document = serde_json::to_string(&block.document).expect("the document serializes");
    assert!(document.contains(notice::INTEGRATIONS_OFF), "{document}");
    let chip = app
        .card_titlebar_chip(panel.id)
        .expect("a card of Horizon carries a chip in its titlebar");
    assert_eq!(
        chip.spoken(),
        ("", "", crate::block_widget::chip::HORIZON),
        "the titlebar of the notice names Horizon, not an agent that is not there"
    );
    assert_eq!(app.setup.notice_panel, Some(panel.id));
}

#[test]
fn a_second_press_of_set_up_reveals_its_panel_and_starts_no_second_one() {
    let (_temp, ctx, mut app) = window_without_integrations();
    // The notice stands in for the panel of the agent: the rule of the second
    // press tells a panel that lives from one that the person closed.
    app.press_set_up(&ctx, SetupGoal::Server);
    let notice = app.setup.notice_panel.expect("the notice");
    app.setup.panel = Some(notice);
    let panels_before = app.board.panels.len();

    app.press_set_up(&ctx, SetupGoal::Rent);

    assert_eq!(
        app.board.panels.len(),
        panels_before,
        "a press with a living panel starts nothing"
    );
    assert_eq!(app.setup.panel, Some(notice), "the panel of Set up lives on");
}

#[test]
fn a_panel_of_set_up_that_the_person_closed_is_forgotten_so_the_next_press_starts_one() {
    let (_temp, ctx, mut app) = window_without_integrations();
    app.press_set_up(&ctx, SetupGoal::Browser);
    let notice = app.setup.notice_panel.expect("the notice");
    app.setup.panel = Some(notice);
    app.close_panel(notice);

    assert_eq!(
        app.setup_panel_to_reveal(),
        None,
        "a closed panel is no panel of Set up"
    );
    assert_eq!(app.setup.panel, None);
}

#[test]
fn the_two_buttons_of_the_notice_do_what_they_say() {
    let (_temp, ctx, mut app) = window_without_integrations();
    app.press_set_up(&ctx, SetupGoal::Join);
    let notice = app.setup.notice_panel.expect("the notice");
    let panels_before = app.board.panels.len();

    // Look again runs Set up for the same goal, which needs a search here, and
    // the integrations are off, so the notice stays the answer.
    app.press_host_card_button(&ctx, notice, notice::LOOK_AGAIN);
    assert_eq!(app.board.panels.len(), panels_before, "one notice, never two");
    assert_eq!(app.setup.goal, SetupGoal::Join, "Look again keeps the goal");

    app.press_host_card_button(&ctx, notice, notice::OPEN_PRESETS);
    assert!(app.settings.is_some(), "the settings open");
}

#[test]
fn a_press_on_a_card_that_horizon_does_not_own_reaches_no_action() {
    let (_temp, ctx, mut app) = window_without_integrations();
    app.press_set_up(&ctx, SetupGoal::Choose);
    let notice = app.setup.notice_panel.expect("the notice");

    app.press_host_card_button(&ctx, horizon_core::PanelId(u64::MAX), notice::OPEN_PRESETS);
    assert!(app.settings.is_none(), "another card owns its own presses");
    assert_eq!(app.setup.notice_panel, Some(notice));
}

#[test]
fn the_notice_lists_one_row_for_each_agent_program_and_the_integrations() {
    let rows = vec![
        ProgramRow {
            name: "Claude".to_owned(),
            program: "claude".to_owned(),
            found: true,
        },
        ProgramRow {
            name: "Codex".to_owned(),
            program: "codex".to_owned(),
            found: false,
        },
    ];
    let document = serde_json::to_string(&notice::document(&rows, true)).expect("the document serializes");
    assert!(document.contains("\"Claude\""), "{document}");
    assert!(document.contains("\"found\""), "{document}");
    assert!(document.contains("\"not found\""), "{document}");
    assert!(document.contains("\"Agent integrations\""), "{document}");
    assert!(document.contains("\"on\""), "{document}");
}
