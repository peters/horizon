//! The wake of the orchestrator of a workspace: the line that Horizon sends to
//! the program of the dock when a press waits, the conversation that names that
//! turn, and the line that names one press of the person.

use std::path::Path;
use std::time::{Duration, Instant};

use egui::Context;
use horizon_browser_control::manifest::block::Operation;
use horizon_core::agent_bridge::Entry;
use horizon_core::{PanelId, PanelKind, RuntimeState, StartupDecision};

use super::*;
use crate::app::block_clicks::WakeTarget;
use crate::app::orchestrator_dock::WAKE_NOTICE;
use crate::app::test_support::{editor_workspace_state, test_app_with_startup};

/// The local id of the workspace of the orchestrator tests: the form that
/// `new_local_id` makes, which is the only form that an orchestrator actor
/// carries.
const DOCK_WORKSPACE: &str = "0f1e2d3c-4b5a-4697-8877-665544332211";

/// The stand-in of the program of the dock: it names its session and answers each
/// line of its input with one text and one result, so a test holds a running
/// bridge without an agent program and its turns end.
const ANSWERING_PROGRAM: &str = r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000001"}'
n=0
while IFS= read -r line; do
  n=$((n+1))
  printf '{"type":"assistant","message":{"id":"msg-%s","content":[{"type":"text","text":"the stand-in answers"}]}}\n' "$n"
  printf '%s\n' '{"type":"result","is_error":false,"duration_ms":5}'
done
"#;

/// A board of one workspace whose local id an orchestrator actor can name.
fn dock_app() -> (tempfile::TempDir, Context, HorizonApp) {
    let runtime = RuntimeState {
        workspaces: vec![editor_workspace_state(DOCK_WORKSPACE, [0.0, 0.0])],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    app.board.panels[0].kind = PanelKind::Claude;
    (temp, ctx, app)
}

/// Starts the program of the dock for the workspace of `app`, so the orchestrator
/// of that workspace is present, and answers the local id of its conversation.
fn start_dock(app: &mut HorizonApp, ctx: &Context, folder: &Path) -> String {
    let program = horizon_core::stand_in_program_in(folder, "stand-in.sh", ANSWERING_PROGRAM)
        .expect("the stand-in program of the dock");
    app.orchestrator_dock = crate::app::orchestrator_dock::OrchestratorDock::with_program(true, program);
    let local_id = DOCK_WORKSPACE.to_owned();
    assert!(
        app.say_to_orchestrator(ctx, &local_id, "hello"),
        "the message of the person reaches the conversation of the workspace"
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while !app.orchestrator_dock.running(&local_id) || app.orchestrator_dock.busy(&local_id) {
        assert!(Instant::now() < deadline, "the program of the dock starts and answers");
        app.poll_orchestrator_dock(&egui::Context::default());
        std::thread::sleep(Duration::from_millis(5));
    }
    local_id
}

/// A request of the orchestrator of the workspace of `app`.
fn orchestrator_request(operation: Operation) -> Request {
    Request {
        request_id: format!("test-orchestrator-{}", manifest::now_millis()),
        actor: horizon_core::agent_bridge::actor::of_workspace(DOCK_WORKSPACE).expect("the actor of a workspace"),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

/// Draws one card of the orchestrator of the workspace of `app`, and answers its
/// local id and its panel id.
fn orchestrator_card(app: &mut HorizonApp, ctx: &Context) -> (String, PanelId) {
    let outcome = app.apply_block_request(
        &orchestrator_request(Operation::Create {
            title: Some("Plan".into()),
            document: document("Plan"),
        }),
        ctx,
    );
    let Outcome::Created { panel_id, .. } = outcome else {
        panic!("the orchestrator draws its card: {outcome:?}");
    };
    let id = app
        .board
        .panel_id_by_local_id(&panel_id)
        .expect("the card is on the board");
    (panel_id, id)
}

/// The notices of the conversation of `local_id`.
fn notices(app: &HorizonApp, local_id: &str) -> Vec<String> {
    app.orchestrator_dock
        .conversation_entries(local_id)
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Notice { text } => Some(text),
            _ => None,
        })
        .collect()
}

#[test]
fn a_press_on_a_card_of_an_orchestrator_wakes_its_conversation_once() {
    let (temp, ctx, mut app) = dock_app();
    let local_id = start_dock(&mut app, &ctx, temp.path());
    let (_, id) = orchestrator_card(&mut app, &ctx);
    app.board.focused = None;

    press(&mut app, id, "continue");
    assert!(
        app.settle_block_clicks(),
        "the press wakes the conversation of the workspace"
    );

    let entries = app.orchestrator_dock.conversation_entries(&local_id);
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry, Entry::Notice { text } if text == WAKE_NOTICE)),
        "the conversation names the wake as a notice of Horizon: {entries:?}"
    );
    assert!(
        !entries
            .iter()
            .any(|entry| matches!(entry, Entry::Person { text, .. } if text == horizon_core::WAKE_LINE)),
        "the wake line is no message of the person"
    );
    assert_eq!(
        app.orchestrator_dock.person_messages(&local_id),
        vec!["hello".to_owned()],
        "the wake adds no message of the person"
    );
    assert!(
        app.orchestrator_dock.busy(&local_id),
        "the turn of the wake runs in the conversation"
    );
    // One wake for one streak: the second press of the same streak adds no line.
    press(&mut app, id, "continue");
    let _ = app.settle_block_clicks();
    assert_eq!(
        notices(&app, &local_id)
            .iter()
            .filter(|text| text.as_str() == WAKE_NOTICE)
            .count(),
        1,
        "the streak of presses gets one wake only"
    );
}

#[test]
fn a_press_that_the_wait_of_the_orchestrator_reads_is_no_wake() {
    let (temp, ctx, mut app) = dock_app();
    let local_id = start_dock(&mut app, &ctx, temp.path());
    let (panel_id, id) = orchestrator_card(&mut app, &ctx);
    let request = orchestrator_request(Operation::NextClick {
        panel_id: Some(panel_id),
        timeout_millis: Some(30_000),
    });
    assert!(
        app.apply_block_request_waiting(&request, &ctx).is_none(),
        "the wait of the orchestrator stays open"
    );

    press(&mut app, id, "continue");
    let _ = app.settle_block_clicks();

    assert!(
        !notices(&app, &local_id).iter().any(|text| text == WAKE_NOTICE),
        "a listening owner needs no wake"
    );
}

#[test]
fn a_wake_of_an_orchestrator_waits_for_the_interval_of_the_line() {
    let (temp, ctx, mut app) = dock_app();
    let local_id = start_dock(&mut app, &ctx, temp.path());
    let (_, id) = orchestrator_card(&mut app, &ctx);
    app.board.focused = None;
    let target = WakeTarget::Orchestrator(local_id.clone());
    // The clock of the last wake is the same map that the line of an agent panel
    // uses, so a test sets the age of the wake of this conversation.
    app.block_clicks.last_wake.insert(target.clone(), Instant::now());

    press(&mut app, id, "continue");
    let _ = app.settle_block_clicks();

    assert!(
        !notices(&app, &local_id).iter().any(|text| text == WAKE_NOTICE),
        "no second wake within thirty seconds of the last one"
    );
    let old = Instant::now().checked_sub(horizon_core::WAKE_LINE_INTERVAL + Duration::from_secs(1));
    app.block_clicks
        .last_wake
        .insert(target, old.expect("a time thirty one seconds ago"));
    assert!(
        app.settle_block_clicks(),
        "the hold of the interval is tried again on a later frame"
    );
    assert!(
        notices(&app, &local_id).iter().any(|text| text == WAKE_NOTICE),
        "the press wakes the conversation once the interval passed"
    );
}

#[test]
fn a_card_of_an_agent_panel_keeps_the_terminal_wake_line() {
    let (_temp, ctx, mut app) = app();
    let (_, id) = card(&mut app, &ctx, 0);
    let owner = app.board.panels[0].id;
    app.board.focused = None;

    press(&mut app, id, "continue");

    assert_eq!(
        app.planned_wakes(),
        vec![(id, WakeTarget::Panel(owner))],
        "the owner of the card is an agent panel, so its terminal is the target of the wake"
    );
    assert!(
        !app.block_clicks
            .last_wake
            .contains_key(&WakeTarget::Orchestrator(DOCK_WORKSPACE.to_owned())),
        "no conversation of a dock is woken for a card of an agent panel"
    );
}

#[test]
fn the_conversation_names_the_press_of_a_card_of_an_orchestrator_with_cut_text() {
    let (temp, ctx, mut app) = dock_app();
    let local_id = start_dock(&mut app, &ctx, temp.path());
    let long_label = "G".repeat(60);
    let long_heading = "H".repeat(60);
    let outcome = app.apply_block_request(
        &orchestrator_request(Operation::Create {
            title: Some("Plan".into()),
            document: serde_json::json!({
                "components": [
                    {"type": "heading", "text": long_heading},
                    {"type": "buttons", "items": [{"id": "go", "label": long_label}]}
                ]
            }),
        }),
        &ctx,
    );
    let Outcome::Created { panel_id, .. } = outcome else {
        panic!("the orchestrator draws its card: {outcome:?}");
    };
    let id = app
        .board
        .panel_id_by_local_id(&panel_id)
        .expect("the card is on the board");
    let document = card_document(&app, id);
    let revision = app.board.panel(id).and_then(Panel::block).expect("a card").revision;
    let card = app.panel_render_caches.card_runtime.entry(id).or_default();
    assert!(card.press("go", revision, &document, manifest::now_millis()));
    card.note_press(&long_label);

    assert!(app.settle_block_clicks());

    let line = notices(&app, &local_id)
        .into_iter()
        .find(|text| text.starts_with("You pressed "))
        .expect("the press of the person is named in the conversation");
    assert_eq!(
        line,
        format!("You pressed {} on {}", "G".repeat(40), "H".repeat(40)),
        "the label and the heading of the card are cut to forty characters each"
    );
}
