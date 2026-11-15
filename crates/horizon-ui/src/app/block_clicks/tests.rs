//! The click waits of a card, and the wake line that reaches its owner.

use std::path::Path;
use std::time::{Duration, Instant};

use egui::Context;
use horizon_browser_control::block::BlockDocument;
use horizon_browser_control::manifest::block;
use horizon_core::machine_watch::{AgentState, MachineAgent, MachineWatchSnapshot, MachineWatchState, WatchLink};
use horizon_core::{Machine, MachineRegistry, Machines, PanelKind, RuntimeState, StartupDecision};

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
            panel_id: Some(panel_id.to_owned()),
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

/// A document that asks the person one question with `answers`.
fn asking_document(answers: &[(&str, &str)]) -> serde_json::Value {
    serde_json::json!({
        "components": [
            {"type": "heading", "text": "Run the plan?"},
            {"type": "callout", "tone": "neutral", "text": "Nothing starts before Go.",
             "buttons": answers
                .iter()
                .map(|(id, label)| serde_json::json!({"id": id, "label": label}))
                .collect::<Vec<_>>()}
        ]
    })
}

/// Replaces the document of `panel_id` with `document`, and answers its panel id.
fn update_card(app: &mut HorizonApp, ctx: &Context, panel_id: &str, document: serde_json::Value) -> PanelId {
    app.apply_block_request(
        &request_from(
            app,
            0,
            Operation::Update {
                panel_id: panel_id.to_owned(),
                document,
            },
        ),
        ctx,
    );
    app.board
        .panel_id_by_local_id(panel_id)
        .expect("the card is on the board")
}

#[test]
fn the_answer_of_a_callout_is_saved_with_the_card() {
    let (_temp, ctx, mut app) = app();
    let (panel_id, _) = card(&mut app, &ctx, 0);
    let id = update_card(&mut app, &ctx, &panel_id, asking_document(&[("go", "Go")]));
    assert!(
        app.board
            .panel(id)
            .and_then(Panel::block)
            .and_then(BlockPanelState::answered_callout)
            .is_none(),
        "a new document asks its question"
    );

    press(&mut app, id, "go");
    assert!(app.settle_block_clicks(), "the answer of the person is saved");
    assert!(app.runtime_is_dirty(), "the card asks for the save of its panel");
    let state = app.board.panel(id).and_then(Panel::block).expect("a card");
    let answer = state.answered_callout().expect("the card holds the answer");
    assert!(
        matches!(answer, horizon_browser_control::block::Component::Callout { .. }),
        "the card saves the callout that the person answered"
    );
    let saved = serde_json::to_string(state).expect("the card saves");
    assert!(
        saved.contains("\"answered_callout\""),
        "the answer travels with the saved card: {saved}"
    );
    let loaded: BlockPanelState = serde_json::from_str(&saved).expect("a saved card loads");
    assert_eq!(loaded, *state, "a restart reads the same answer back");
}

#[test]
fn a_restored_card_answers_its_callout_with_the_saved_answer() {
    let (_temp, ctx, mut app) = app();
    let (panel_id, _) = card(&mut app, &ctx, 0);
    let id = update_card(&mut app, &ctx, &panel_id, asking_document(&[("go", "Go")]));
    press(&mut app, id, "go");
    let _ = app.settle_block_clicks();

    // A restart keeps the saved card and starts the runtime state of the card
    // empty, as the first frame that draws it does.
    app.panel_render_caches.card_runtime.remove(&id);
    app.panel_render_caches.card_runtime.entry(id).or_default();
    let _ = app.settle_block_clicks();
    let callout = app
        .board
        .panel(id)
        .and_then(Panel::block)
        .and_then(BlockPanelState::answered_callout)
        .cloned()
        .expect("the saved answer");
    let runtime = app
        .panel_render_caches
        .card_runtime
        .get(&id)
        .expect("the card is on the board");
    assert_eq!(
        runtime.answered_callout(),
        Some(&callout),
        "the live card of a restored run starts with the saved answer"
    );
}

#[test]
fn a_waiting_next_click_is_answered_with_the_click_that_arrives() {
    let (temp, ctx, mut app) = app();
    let (panel_id, id) = card(&mut app, &ctx, 0);
    let request = next_click(&app, 0, &panel_id, 60_000);
    let root = temp.path();
    claim_request(&mut app, &ctx, root, &request);
    assert!(app.block_clicks.card_waiters.contains_key(&id), "the wait is open");
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
    assert!(app.block_clicks.card_waiters.is_empty(), "a wait is answered once");
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

    let held = app.begin_click_wait(
        Some(temp.path()),
        &next_click(&app, 0, &panel_id, 60_000),
        Some(&panel_id),
    );
    let Some(Outcome::Clicked { click }) = held else {
        panic!("a waiting click answers at once: {held:?}");
    };
    assert_eq!(click.component_id, "continue");
    assert!(app.block_clicks.card_waiters.is_empty());
}

#[test]
fn a_wait_that_no_click_ends_answers_timed_out() {
    let (temp, ctx, mut app) = app();
    let (panel_id, _id) = card(&mut app, &ctx, 0);
    let request = next_click(&app, 0, &panel_id, 20);
    claim_request(&mut app, &ctx, temp.path(), &request);
    assert!(!app.block_clicks.card_waiters.is_empty(), "the wait starts open");

    let outcome = wait_answer(&mut app, &ctx, temp.path(), &request);
    assert!(
        matches!(outcome, Outcome::TimedOut { timed_out: true }),
        "a wait nobody answers times out: {outcome:?}"
    );
    assert!(app.block_clicks.card_waiters.is_empty());
}

#[test]
fn a_second_wait_answers_the_older_one_with_timed_out() {
    let (temp, ctx, mut app) = app();
    let (panel_id, id) = card(&mut app, &ctx, 0);
    let older = next_click(&app, 0, &panel_id, 60_000);
    claim_request(&mut app, &ctx, temp.path(), &older);
    assert!(app.block_clicks.card_waiters.contains_key(&id));

    let newer = next_click(&app, 0, &panel_id, 60_000);
    claim_request(&mut app, &ctx, temp.path(), &newer);
    assert_eq!(app.block_clicks.card_waiters.len(), 1, "one wait per card");

    let answered = wait_answer(&mut app, &ctx, temp.path(), &older);
    assert!(
        matches!(answered, Outcome::TimedOut { timed_out: true }),
        "a replaced wait ends as a timeout: {answered:?}"
    );
    assert!(
        app.block_clicks.card_waiters.contains_key(&id),
        "the newer wait stays open"
    );

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
        app.block_clicks.card_waiters.contains_key(&id),
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
    assert!(app.block_clicks.card_waiters.contains_key(&id));

    app.board.panels.retain(|panel| panel.id != id);
    let outcome = wait_answer(&mut app, &ctx, temp.path(), &request);
    let Outcome::Failed { code, .. } = outcome else {
        panic!("a closed card is a refusal: {outcome:?}");
    };
    assert_eq!(code, "panel_unavailable");
    assert!(app.block_clicks.card_waiters.is_empty());
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

    let held = app.begin_click_wait(
        Some(temp.path()),
        &next_click(&app, 1, &panel_id, 60_000),
        Some(&panel_id),
    );
    let Some(Outcome::Failed { code, .. }) = held else {
        panic!("another agent is refused: {held:?}");
    };
    assert_eq!(code, "not_owner");
    assert!(app.block_clicks.card_waiters.is_empty());
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

/// A `next_click` of an agent that names no card.
fn next_click_any(app: &HorizonApp, panel: usize, wait: i64) -> Request {
    let mut request = request_from(
        app,
        panel,
        Operation::NextClick {
            panel_id: None,
            timeout_millis: Some(u64::try_from(wait).unwrap_or_default()),
        },
    );
    request.deadline_at_millis = manifest::now_millis() + host_apply_millis() + wait;
    request
}

#[test]
fn one_wait_covers_every_card_of_an_agent_and_takes_the_oldest_click() {
    let (temp, ctx, mut app) = app();
    let (_, first) = card(&mut app, &ctx, 0);
    let (_, second) = card(&mut app, &ctx, 0);
    let request = next_click_any(&app, 0, 60_000);
    claim_request(&mut app, &ctx, temp.path(), &request);
    assert!(app.block_clicks.agent_waiters.contains_key(&app_actor(&app, 0)));
    assert!(app.block_listening(first), "every card of the agent is listened to");
    assert!(app.block_listening(second), "every card of the agent is listened to");

    // The person presses the second card first, then the first card.
    press(&mut app, second, "continue");
    std::thread::sleep(Duration::from_millis(2));
    press(&mut app, first, "continue");

    let outcome = wait_answer(&mut app, &ctx, temp.path(), &request);
    let Outcome::Clicked { click } = outcome else {
        panic!("the wait is answered with the oldest click: {outcome:?}");
    };
    assert_eq!(
        app.board.panel_id_by_local_id(&click.panel_id),
        Some(second),
        "the older click of the two cards wins"
    );
    assert!(!app.block_listening(first), "the wait is answered once");
}

#[test]
fn a_second_any_card_wait_answers_the_older_one_with_timed_out() {
    let (temp, ctx, mut app) = app();
    let (_, id) = card(&mut app, &ctx, 0);
    let older = next_click_any(&app, 0, 60_000);
    claim_request(&mut app, &ctx, temp.path(), &older);
    let newer = next_click_any(&app, 0, 60_000);
    claim_request(&mut app, &ctx, temp.path(), &newer);
    assert_eq!(app.block_clicks.agent_waiters.len(), 1, "one wait per agent");

    let answered = wait_answer(&mut app, &ctx, temp.path(), &older);
    assert!(
        matches!(answered, Outcome::TimedOut { timed_out: true }),
        "the replaced wait ends as a timeout: {answered:?}"
    );
    press(&mut app, id, "continue");
    let answered = wait_answer(&mut app, &ctx, temp.path(), &newer);
    assert!(matches!(answered, Outcome::Clicked { .. }), "{answered:?}");
}

#[test]
fn a_card_wait_takes_its_own_click_before_the_any_card_wait_of_its_agent() {
    let (temp, ctx, mut app) = app();
    let (first_id, first) = card(&mut app, &ctx, 0);
    let (second_id, second) = card(&mut app, &ctx, 0);
    let any = next_click_any(&app, 0, 60_000);
    claim_request(&mut app, &ctx, temp.path(), &any);
    let one = next_click(&app, 0, &second_id, 60_000);
    claim_request(&mut app, &ctx, temp.path(), &one);

    press(&mut app, second, "continue");
    let answered = wait_answer(&mut app, &ctx, temp.path(), &one);
    let Outcome::Clicked { click } = answered else {
        panic!("the wait that names the card answers first: {answered:?}");
    };
    assert_eq!(click.panel_id, second_id);

    std::thread::sleep(Duration::from_millis(2));
    press(&mut app, first, "continue");
    let answered = wait_answer(&mut app, &ctx, temp.path(), &any);
    let Outcome::Clicked { click } = answered else {
        panic!("the any-card wait takes what is left: {answered:?}");
    };
    assert_eq!(click.panel_id, first_id);
}

#[test]
fn an_any_card_wait_that_owns_no_card_is_refused_at_once() {
    let (temp, _ctx, mut app) = app();
    let held = app.begin_click_wait(Some(temp.path()), &next_click_any(&app, 0, 60_000), None);
    let Some(Outcome::Failed { code, .. }) = held else {
        panic!("an agent without a card learns it at once: {held:?}");
    };
    assert_eq!(code, "no_cards");
    assert!(app.block_clicks.agent_waiters.is_empty());
}

#[test]
fn an_any_card_wait_does_not_take_a_click_of_another_agents_card() {
    let (temp, ctx, mut app) = app();
    let (_, mine) = card(&mut app, &ctx, 0);
    app.board.panels[1].workspace_id = app.board.panels[0].workspace_id;
    let (_, theirs) = card(&mut app, &ctx, 1);
    let request = next_click_any(&app, 0, 60_000);
    claim_request(&mut app, &ctx, temp.path(), &request);
    assert!(!app.block_listening(theirs), "another agent's card is not listened to");

    press(&mut app, theirs, "continue");
    assert!(app.block_clicks.agent_waiters.contains_key(&app_actor(&app, 0)));
    press(&mut app, mine, "continue");

    let answered = wait_answer(&mut app, &ctx, temp.path(), &request);
    let Outcome::Clicked { click } = answered else {
        panic!("the wait is answered: {answered:?}");
    };
    assert_eq!(app.board.panel_id_by_local_id(&click.panel_id), Some(mine));
}

/// The actor string of the agent panel at `panel`.
fn app_actor(app: &HorizonApp, panel: usize) -> String {
    format!("horizon:{}", app.board.panels[panel].local_id)
}

/// Keeps the machine `build-1` in the list of the app, with the mark of its cards.
fn keep_machine(app: &mut HorizonApp) {
    let machine = Machine {
        name: "build-1".to_owned(),
        host: "127.0.0.1".to_owned(),
        user: None,
        port: Some(2222),
        session: None,
        provider: None,
        repository: None,
        added_at_millis: 0,
        cards: true,
    };
    app.machine_list = Machines::with_records(MachineRegistry::from_home(app.session_store.home()), vec![machine]);
}

/// Keeps a watch snapshot that reports the session `session` of `build-1`.
fn watch_reports(app: &mut HorizonApp, session: &str) {
    app.machine_watch.snapshot = Some(std::sync::Arc::new(MachineWatchSnapshot {
        version: 1,
        machines: vec![MachineWatchState {
            machine: "build-1".to_owned(),
            link: WatchLink::Watching,
            sessions: vec![session.to_owned()],
            agents: vec![MachineAgent {
                machine: "build-1".to_owned(),
                session: session.to_owned(),
                pane: "%1".to_owned(),
                kind: PanelKind::Claude,
                name: "night".to_owned(),
                state: AgentState::default(),
            }],
        }],
        ..MachineWatchSnapshot::default()
    }));
}

/// A request of the session `session` of `build-1`.
fn session_request(session: &str, operation: Operation) -> Request {
    Request {
        request_id: format!("test-{session}"),
        actor: horizon_core::node_cards::actor("build-1", session).expect("a plain pair"),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

#[test]
fn a_press_on_a_card_of_a_machine_session_reaches_that_session_and_no_other() {
    let (temp, ctx, mut app) = app();
    keep_machine(&mut app);
    watch_reports(&mut app, "night");
    let create = session_request(
        "night",
        Operation::Create {
            title: Some("cloud-1".into()),
            document: document("cloud-1"),
        },
    );
    let Outcome::Created { panel_id, .. } = app.apply_block_request(&create, &ctx) else {
        panic!("the session of the machine draws a card");
    };
    let id = app
        .board
        .panel_id_by_local_id(&panel_id)
        .expect("the card is on the board");

    // The session of the owner waits for a click on its card.
    let waiting = session_request(
        "night",
        Operation::NextClick {
            panel_id: Some(panel_id.clone()),
            timeout_millis: Some(60_000),
        },
    );
    let root = temp.path();
    claim_request(&mut app, &ctx, root, &waiting);
    assert!(app.block_clicks.card_waiters.contains_key(&id), "the wait is open");
    assert!(app.block_listening(id), "the card of the session says that it listens");

    press(&mut app, id, "continue");
    let Outcome::Clicked { click } = wait_answer(&mut app, &ctx, root, &waiting) else {
        panic!("the click of the person reaches the session of the machine");
    };
    assert_eq!(click.panel_id, panel_id);
    assert_eq!(click.component_id, "continue");

    // A second session of the same machine owns nothing of this card.
    let stranger = session_request(
        "plan",
        Operation::NextClick {
            panel_id: Some(panel_id.clone()),
            timeout_millis: Some(1_000),
        },
    );
    let refusal = app.apply_block_request(&stranger, &ctx);
    assert!(
        matches!(refusal, Outcome::Failed { ref code, .. } if code == "not_owner"),
        "the card of one session against another session: {refusal:?}"
    );
}
