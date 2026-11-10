//! The click waits of a card, and the wake line that reaches its owner.

use std::path::Path;
use std::time::{Duration, Instant};

use egui::Context;
use horizon_browser_control::block::BlockDocument;
use horizon_browser_control::manifest::block;
use horizon_core::{PanelKind, RuntimeState, StartupDecision};

use super::*;
use crate::app::block_requests::ClaimedBlockRequest;
use crate::app::test_support::{editor_workspace_state, test_app_with_startup};

/// A board with two agent panels, each in its own workspace.
fn app() -> (tempfile::TempDir, Context, HorizonApp) {
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
    (temp, ctx, app)
}

fn document(title: &str) -> serde_json::Value {
    serde_json::json!({
        "components": [
            {"type": "heading", "text": title},
            {"type": "list", "selection": "single", "rows": [
                {"id": "size.cx22", "title": "CX22"},
                {"id": "size.cx32", "title": "CX32"}]},
            {"type": "buttons", "items": [{"id": "continue", "label": "Continue"}]}
        ]
    })
}

/// A request of the agent panel at `panel`, against the process runtime root.
fn request_from(app: &HorizonApp, panel: usize, operation: Operation) -> Request {
    Request {
        request_id: format!("test-{}", manifest::now_millis()),
        actor: format!("horizon:{}", app.board.panels[panel].local_id),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

/// Draws a card owned by the agent panel at `panel`, and returns its local id
/// and panel id.
fn card(app: &mut HorizonApp, ctx: &Context, panel: usize) -> (String, PanelId) {
    let outcome = app.apply_block_request(
        &request_from(
            app,
            panel,
            Operation::Create {
                title: Some("cloud-1".into()),
                document: document("cloud-1"),
            },
        ),
        ctx,
    );
    let Outcome::Created { panel_id, .. } = outcome else {
        panic!("the card was not created: {outcome:?}");
    };
    let id = app
        .board
        .panel_id_by_local_id(&panel_id)
        .expect("the card is on the board");
    (panel_id, id)
}

/// The document the card draws, as the widget sees it.
fn card_document(app: &HorizonApp, id: PanelId) -> BlockDocument {
    app.board
        .panel(id)
        .and_then(Panel::block)
        .map(|block| block.document.clone())
        .expect("a card panel holds its document")
}

/// A `next_click` that waits `wait` milliseconds after the host could apply it.
fn next_click(app: &HorizonApp, panel: usize, panel_id: &str, wait: i64) -> Request {
    let mut request = request_from(
        app,
        panel,
        Operation::NextClick {
            panel_id: panel_id.to_owned(),
            timeout_millis: Some(u64::try_from(wait).unwrap_or_default()),
        },
    );
    request.deadline_at_millis = manifest::now_millis() + host_apply_millis() + wait;
    request
}

/// Applies one request the way a claim does.
fn claim_request(app: &mut HorizonApp, ctx: &Context, root: &Path, request: &Request) {
    app.block_claim.claimed.push(ClaimedBlockRequest {
        root: Some(root.to_path_buf()),
        request: request.clone(),
    });
    let _ = app.drain_block_panel_requests(ctx, Some(root));
}

/// Waits for the answer of a request the host holds or answers later.
fn wait_answer(app: &mut HorizonApp, ctx: &Context, root: &Path, request: &Request) -> Outcome {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let _ = app.drain_block_panel_requests(ctx, Some(root));
        if let Some(outcome) = block::take_result_at(root, request).expect("the answer queue") {
            return outcome;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the card request was never answered");
}

/// Queues one click on a card, as the widget does.
fn press(app: &mut HorizonApp, id: PanelId, component_id: &str) {
    let document = card_document(app, id);
    let revision = app.board.panel(id).and_then(Panel::block).expect("a card").revision;
    let card = app.panel_render_caches.card_runtime.entry(id).or_default();
    assert!(
        card.press(component_id, revision, &document, manifest::now_millis()),
        "the card holds the click"
    );
}

#[test]
fn a_waiting_next_click_is_answered_with_the_click_that_arrives() {
    let (temp, ctx, mut app) = app();
    let (panel_id, id) = card(&mut app, &ctx, 0);
    let request = next_click(&app, 0, &panel_id, 60_000);
    let root = temp.path();
    claim_request(&mut app, &ctx, root, &request);
    assert!(app.block_clicks.waiters.contains_key(&id), "the wait is open");
    assert!(app.block_listening(id));

    // The person selects a row and presses a button.
    let document = card_document(&app, id);
    let card = app.panel_render_caches.card_runtime.entry(id).or_default();
    assert!(card.select(1, "size.cx32", &document));
    let revision = document_revision(&app, id);
    press(&mut app, id, "continue");

    let outcome = wait_answer(&mut app, &ctx, root, &request);
    let Outcome::Clicked { click } = outcome else {
        panic!("the wait must be answered with the click: {outcome:?}");
    };
    assert_eq!(click.panel_id, panel_id);
    assert_eq!(click.component_id, "continue");
    assert_eq!(click.revision, revision);
    assert_eq!(
        click.selected,
        vec!["size.cx32".to_owned()],
        "the click carries the selection the person made"
    );
    assert_eq!(click.event_id, 1);
    assert!(!click.stale);
    assert!(app.block_clicks.waiters.is_empty(), "a wait is answered once");
    assert!(!app.block_listening(id));
}

fn document_revision(app: &HorizonApp, id: PanelId) -> u64 {
    app.board.panel(id).and_then(Panel::block).expect("a card").revision
}

#[test]
fn a_click_that_waits_before_the_call_answers_at_once() {
    let (temp, ctx, mut app) = app();
    let (panel_id, id) = card(&mut app, &ctx, 0);
    press(&mut app, id, "continue");

    let held = app.begin_click_wait(Some(temp.path()), &next_click(&app, 0, &panel_id, 60_000), &panel_id);
    let Some(Outcome::Clicked { click }) = held else {
        panic!("a waiting click answers at once: {held:?}");
    };
    assert_eq!(click.component_id, "continue");
    assert!(app.block_clicks.waiters.is_empty());
}

#[test]
fn a_wait_that_no_click_ends_answers_timed_out() {
    let (temp, ctx, mut app) = app();
    let (panel_id, _id) = card(&mut app, &ctx, 0);
    let request = next_click(&app, 0, &panel_id, 20);
    claim_request(&mut app, &ctx, temp.path(), &request);
    assert!(!app.block_clicks.waiters.is_empty(), "the wait starts open");

    let outcome = wait_answer(&mut app, &ctx, temp.path(), &request);
    assert!(
        matches!(outcome, Outcome::TimedOut { timed_out: true }),
        "a wait nobody answers times out: {outcome:?}"
    );
    assert!(app.block_clicks.waiters.is_empty());
}

#[test]
fn a_second_wait_answers_the_older_one_with_timed_out() {
    let (temp, ctx, mut app) = app();
    let (panel_id, id) = card(&mut app, &ctx, 0);
    let older = next_click(&app, 0, &panel_id, 60_000);
    claim_request(&mut app, &ctx, temp.path(), &older);
    assert!(app.block_clicks.waiters.contains_key(&id));

    let newer = next_click(&app, 0, &panel_id, 60_000);
    claim_request(&mut app, &ctx, temp.path(), &newer);
    assert_eq!(app.block_clicks.waiters.len(), 1, "one wait per card");

    let answered = wait_answer(&mut app, &ctx, temp.path(), &older);
    assert!(
        matches!(answered, Outcome::TimedOut { timed_out: true }),
        "a replaced wait ends as a timeout: {answered:?}"
    );
    assert!(app.block_clicks.waiters.contains_key(&id), "the newer wait stays open");

    press(&mut app, id, "continue");
    let answered = wait_answer(&mut app, &ctx, temp.path(), &newer);
    assert!(matches!(answered, Outcome::Clicked { .. }), "{answered:?}");
}

#[test]
fn a_second_wait_leaves_the_click_to_the_wait_that_was_first() {
    let (temp, ctx, mut app) = app();
    let (panel_id, id) = card(&mut app, &ctx, 0);
    let older = next_click(&app, 0, &panel_id, 60_000);
    claim_request(&mut app, &ctx, temp.path(), &older);
    press(&mut app, id, "continue");

    let newer = next_click(&app, 0, &panel_id, 60_000);
    claim_request(&mut app, &ctx, temp.path(), &newer);

    let answered = wait_answer(&mut app, &ctx, temp.path(), &older);
    assert!(
        matches!(answered, Outcome::Clicked { .. }),
        "the wait that was first gets the click: {answered:?}"
    );
    assert!(
        app.block_clicks.waiters.contains_key(&id),
        "the newer wait replaces it and stays open"
    );
    press(&mut app, id, "continue");
    let answered = wait_answer(&mut app, &ctx, temp.path(), &newer);
    assert!(matches!(answered, Outcome::Clicked { .. }), "{answered:?}");
}

#[test]
fn closing_the_card_answers_its_wait_with_panel_unavailable() {
    let (temp, ctx, mut app) = app();
    let (panel_id, id) = card(&mut app, &ctx, 0);
    let request = next_click(&app, 0, &panel_id, 60_000);
    claim_request(&mut app, &ctx, temp.path(), &request);
    assert!(app.block_clicks.waiters.contains_key(&id));

    app.board.panels.retain(|panel| panel.id != id);
    let outcome = wait_answer(&mut app, &ctx, temp.path(), &request);
    let Outcome::Failed { code, .. } = outcome else {
        panic!("a closed card is a refusal: {outcome:?}");
    };
    assert_eq!(code, "panel_unavailable");
    assert!(app.block_clicks.waiters.is_empty());
    assert!(
        !app.panel_render_caches.card_runtime.contains_key(&id),
        "the runtime state of a closed card is forgotten"
    );
}

#[test]
fn another_agent_of_the_workspace_cannot_wait_on_the_card() {
    let (temp, ctx, mut app) = app();
    let (panel_id, id) = card(&mut app, &ctx, 0);
    app.board.panels[1].workspace_id = app.board.panels[0].workspace_id;

    let held = app.begin_click_wait(Some(temp.path()), &next_click(&app, 1, &panel_id, 60_000), &panel_id);
    let Some(Outcome::Failed { code, .. }) = held else {
        panic!("another agent is refused: {held:?}");
    };
    assert_eq!(code, "not_owner");
    assert!(app.block_clicks.waiters.is_empty());
    assert!(!app.block_listening(id));
}

#[test]
fn list_and_inspect_report_the_clicks_and_the_listening_state() {
    let (temp, ctx, mut app) = app();
    let (panel_id, id) = card(&mut app, &ctx, 0);
    let request = next_click(&app, 0, &panel_id, 60_000);
    claim_request(&mut app, &ctx, temp.path(), &request);
    press(&mut app, id, "continue");

    let Outcome::Panels { panels } = app.apply_block_request(&request_from(&app, 0, Operation::List), &ctx) else {
        panic!("list answers the cards of the workspace");
    };
    let listed = panels.iter().find(|card| card.panel_id == panel_id).expect("the card");
    assert_eq!(listed.pending_clicks, 1);
    assert!(listed.listening, "a wait of the owner listens");

    let inspect = app.apply_block_request(
        &request_from(
            &app,
            0,
            Operation::Inspect {
                panel_id: panel_id.clone(),
            },
        ),
        &ctx,
    );
    let Outcome::Inspected {
        pending_clicks,
        listening,
        agent_present,
        ..
    } = inspect
    else {
        panic!("inspect answers the card: {inspect:?}");
    };
    assert_eq!(pending_clicks, 1);
    assert!(listening);
    assert!(agent_present);
}

#[test]
fn a_click_delivered_after_a_new_document_is_stale() {
    let (temp, ctx, mut app) = app();
    let (panel_id, id) = card(&mut app, &ctx, 0);
    let revision = document_revision(&app, id);
    press(&mut app, id, "continue");
    app.apply_block_request(
        &request_from(
            &app,
            0,
            Operation::Update {
                panel_id: panel_id.clone(),
                document: document("cloud-1, second view"),
            },
        ),
        &ctx,
    );

    let request = next_click(&app, 0, &panel_id, 60_000);
    claim_request(&mut app, &ctx, temp.path(), &request);
    let outcome = wait_answer(&mut app, &ctx, temp.path(), &request);
    let Outcome::Clicked { click } = outcome else {
        panic!("the waiting click answers: {outcome:?}");
    };
    assert_eq!(click.revision, revision, "the click keeps the revision the person saw");
    assert!(click.stale, "the document changed before the agent read the click");
}

#[test]
fn a_queued_click_is_a_wake_for_an_idle_owner_that_is_not_typing() {
    let (_temp, ctx, mut app) = app();
    let (_panel_id, id) = card(&mut app, &ctx, 0);
    let owner = app.board.panels[0].id;
    press(&mut app, id, "continue");
    app.board.focused = None;
    let _ = app.settle_block_clicks();
    assert_eq!(app.planned_wakes(), vec![(id, owner)]);

    app.board.focused = Some(owner);
    assert!(
        app.planned_wakes().is_empty(),
        "the person may be typing in the owner's panel"
    );
}

#[test]
fn a_click_that_an_agent_waits_for_is_no_wake() {
    let (temp, ctx, mut app) = app();
    let (panel_id, id) = card(&mut app, &ctx, 0);
    let owner = app.board.panels[0].id;
    press(&mut app, id, "continue");
    // The wait answers the click at once and the click is gone.
    let request = next_click(&app, 0, &panel_id, 60_000);
    claim_request(&mut app, &ctx, temp.path(), &request);
    app.board.focused = None;
    let _ = app.settle_block_clicks();
    assert!(app.planned_wakes().is_empty(), "a listening owner needs no line");
    assert!(!app.block_clicks.last_wake.contains_key(&owner));
}

#[test]
fn a_card_without_clicks_holds_no_wake_streak() {
    let (_temp, ctx, mut app) = app();
    let (_panel_id, id) = card(&mut app, &ctx, 0);
    app.panel_render_caches
        .card_runtime
        .entry(id)
        .or_default()
        .mark_wake_sent();
    assert!(!app.settle_block_clicks(), "no click waits, so nothing wakes");
    let card = app
        .panel_render_caches
        .card_runtime
        .get(&id)
        .expect("a card on the board keeps its runtime state");
    assert!(!card.wake_sent(), "an empty queue starts a new wake streak");
}

#[test]
fn the_runtime_state_of_a_card_that_left_the_board_is_forgotten() {
    let (_temp, ctx, mut app) = app();
    let (_panel_id, id) = card(&mut app, &ctx, 0);
    app.panel_render_caches.card_runtime.entry(id).or_default();
    app.board.panels.retain(|panel| panel.id != id);
    let _ = app.settle_block_clicks();
    assert!(!app.panel_render_caches.card_runtime.contains_key(&id));
}
