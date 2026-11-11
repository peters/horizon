//! The link of a card: one browser panel of its address, beside the card.
//!
//! A browser panel of a test would start a browser engine and reach the network,
//! so the press stands an inert browser panel in its place. The panel carries the
//! address of the link as its start page and lives in the card's workspace, and
//! the card records no click.

use egui::Context;
use horizon_browser_control::manifest::block::{Operation, Outcome, Request};
use horizon_browser_control::manifest::{self};
use horizon_core::browser::BrowserPanelState;
use horizon_core::{Panel, PanelContent, PanelId, PanelKind, PanelOptions, RuntimeState, StartupDecision, WorkspaceId};

use super::HorizonApp;
use crate::app::test_support::{editor_workspace_state, test_app_with_startup};

const PAGE: &str = "https://cloud.example.test/sessions/7";

/// A board with an agent panel of the first workspace, and a card of that agent.
fn app() -> (tempfile::TempDir, Context, HorizonApp, PanelId, WorkspaceId) {
    let runtime = RuntimeState {
        workspaces: vec![
            editor_workspace_state("first", [0.0, 0.0]),
            editor_workspace_state("second", [900.0, 0.0]),
        ],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    app.board.panels[0].kind = PanelKind::Claude;
    app.board.panels[1].kind = PanelKind::Claude;
    let request = Request {
        request_id: "test-link".into(),
        actor: format!("horizon:{}", app.board.panels[0].local_id),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation: Operation::Create {
            title: Some("cloud-1".into()),
            document: serde_json::from_value(serde_json::json!({
                "components": [
                    {"type": "heading", "text": "cloud-1"},
                    {"type": "link", "label": "Open session", "url": PAGE}
                ]
            }))
            .expect("a card document"),
        },
    };
    let outcome = app.apply_block_request(&request, &ctx);
    let Outcome::Created { panel_id, .. } = outcome else {
        panic!("the card was not created: {outcome:?}");
    };
    let card = app
        .board
        .panel_id_by_local_id(&panel_id)
        .expect("the card is on the board");
    let workspace = app.board.panels[0].workspace_id;
    (temp, ctx, app, card, workspace)
}

impl HorizonApp {
    /// Stands an inert browser panel in the place of the browser engine, as the
    /// other browser tests of this crate do: the panel is a browser panel of
    /// `workspace_id` that asks for `options.command` as its page.
    pub(super) fn press_link_for_tests(
        &mut self,
        options: &PanelOptions,
        workspace_id: WorkspaceId,
        card: PanelId,
    ) -> PanelId {
        assert_eq!(options.kind, PanelKind::Browser, "a link opens a browser panel");
        assert!(
            self.board
                .panel(card)
                .is_some_and(|panel| panel.kind == PanelKind::Block && panel.workspace_id == workspace_id),
            "the panel goes beside the card of the press"
        );
        let id = PanelId(9_000 + self.board.panels.len() as u64);
        let mut browser = BrowserPanelState::inert();
        browser.requested_url.clone_from(&options.command);
        let panel = Panel::from_content(
            id,
            workspace_id,
            PanelKind::Browser,
            PanelContent::Browser(Box::new(browser)),
        );
        self.board.panels.push(panel);
        if let Some(workspace) = self.board.workspace_mut(workspace_id) {
            workspace.add_panel(id);
        }
        id
    }
}

#[test]
fn a_press_on_a_link_opens_one_browser_panel_of_that_address_in_the_cards_workspace() {
    let (_temp, ctx, mut app, card, workspace) = app();
    let before = app.board.panels.len();
    // The person looks at another workspace while the card of the first one
    // takes the press.
    app.board.focus(app.board.panels[1].id);

    app.open_card_link(&ctx, card, PAGE);

    assert_eq!(app.board.panels.len(), before + 1, "a press opens exactly one panel");
    let opened = app.board.panels.last().expect("the browser panel");
    assert_eq!(opened.kind, PanelKind::Browser);
    assert_eq!(
        app.board.panel_workspace_id(opened.id),
        Some(workspace),
        "the panel goes to the workspace of the card"
    );
    assert_eq!(
        opened.browser().and_then(|browser| browser.requested_url.clone()),
        Some(PAGE.to_owned()),
        "the panel starts at the address of the link"
    );
}

#[test]
fn a_press_on_a_link_records_no_click_and_needs_no_agent() {
    let (_temp, ctx, mut app, card, _) = app();
    // The agent leaves the workspace; the link still opens its page.
    let agent = app.board.panels[0].id;
    app.close_panel(agent);

    app.open_card_link(&ctx, card, PAGE);

    let now = manifest::now_millis();
    let mut runtime = horizon_core::CardRuntime::default();
    assert_eq!(runtime.pending_clicks(now), 0, "a press on a link is no click");
    assert_eq!(runtime.take_click(now), None);
    assert_eq!(
        app.board
            .panels
            .iter()
            .filter(|panel| panel.kind == PanelKind::Browser)
            .count(),
        1,
        "the link opened its page with no agent in the workspace"
    );
}

#[test]
fn a_press_on_a_panel_that_is_no_card_opens_nothing() {
    let (_temp, ctx, mut app, _, _) = app();
    let before = app.board.panels.len();
    let agent = app.board.panels[0].id;

    app.open_card_link(&ctx, agent, PAGE);

    assert_eq!(app.board.panels.len(), before);
}

/// The rectangle a panel covers, as `[min_x, min_y, max_x, max_y]`.
fn bounds(app: &HorizonApp, panel: PanelId) -> [f32; 4] {
    let layout = app.board.panel(panel).expect("the panel").layout;
    [
        layout.position[0],
        layout.position[1],
        layout.position[0] + layout.size[0],
        layout.position[1] + layout.size[1],
    ]
}

fn overlaps(left: [f32; 4], right: [f32; 4]) -> bool {
    !(left[2] <= right[0] || right[2] <= left[0] || left[3] <= right[1] || right[3] <= left[1])
}

#[test]
fn a_panel_created_beside_a_card_never_covers_it() {
    let (_temp, _ctx, mut app, card, workspace) = app();
    let card_bounds = bounds(&app, card);
    // The terminal runs `/bin/true` in place of a shell, so the test starts no
    // terminal session.
    let options = PanelOptions {
        launch_wrapper: vec!["/bin/true".to_owned()],
        ..PanelOptions::default()
    };

    let shell = app
        .create_panel_with_options_beside(options, workspace, Some(card))
        .expect("a terminal beside the card");
    let shell_bounds = bounds(&app, shell);

    assert!(
        !overlaps(shell_bounds, card_bounds),
        "the panel takes a free place: {shell_bounds:?} against {card_bounds:?}"
    );
    let right_of_the_card = shell_bounds[0] > card_bounds[2] && (shell_bounds[1] - card_bounds[1]).abs() < 1.0;
    let below_the_card = shell_bounds[1] > card_bounds[3] && (shell_bounds[0] - card_bounds[0]).abs() < 1.0;
    assert!(
        right_of_the_card || below_the_card,
        "the panel stands beside the card: {shell_bounds:?} against {card_bounds:?}"
    );
}
