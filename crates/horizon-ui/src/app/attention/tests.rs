//! The tests of the attention state of the agent panels: which panel needs the
//! person, what the sidebar shows and what a look clears.
use std::time::{Duration, Instant};

use horizon_core::agent_work::{TurnLedger, TurnState};
use horizon_core::{Attention, PanelId, PanelKind, PanelOptions};

use crate::app::test_support::{run_app_frame, test_app, test_app_with_config_and_startup};

/// A panel of the board that runs nothing: these tests read the attention state
/// of the sidebar, not a terminal.
fn agent_panel(app: &mut crate::app::HorizonApp, workspace: horizon_core::WorkspaceId, name: &str) -> PanelId {
    app.board
        .create_panel(
            PanelOptions {
                name: Some(name.to_owned()),
                kind: PanelKind::Claude,
                command: Some("seed".to_owned()),
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("panel")
}

/// A context of this test alone: a repaint request on it is a value that the
/// test reads, and it wakes no window.
fn wake() -> egui::Context {
    egui::Context::default()
}

fn local_id(app: &crate::app::HorizonApp, panel: PanelId) -> String {
    app.board.panel(panel).expect("panel").local_id.clone()
}

fn ledger(state: TurnState, updated_at_millis: i64) -> TurnLedger {
    let mut ledger = TurnLedger::default();
    ledger.session_id = "session".into();
    ledger.prompt_id = Some("prompt".into());
    ledger.state = state;
    ledger.updated_at_millis = updated_at_millis;
    ledger
}

#[test]
fn a_read_ledger_sets_the_state_of_its_panel_and_an_unchanged_one_repaints_nothing() {
    let (_temp, ctx, mut app) = test_app_with_config_and_startup(
        &horizon_core::Config::default(),
        horizon_core::StartupDecision::Ephemeral {
            runtime_state: Box::new(horizon_core::RuntimeState::default()),
        },
    );
    let workspace = app.board.create_workspace("a");
    let panel = agent_panel(&mut app, workspace, "Fix the sync");
    let local = local_id(&app, panel);
    let now = horizon_core::current_unix_millis();
    assert_eq!(app.panel_attention(&local), Attention::Idle);
    // The person looks at another panel while this one works.
    app.board.focused = None;

    let working = ledger(TurnState::Working, now - 2);
    assert!(
        app.apply_attention_values(vec![(panel, local.clone(), Some(working.clone()))], &ctx),
        "a changed state asks for one repaint"
    );
    assert_eq!(app.panel_attention(&local), Attention::Working);
    assert!(
        !app.apply_attention_values(vec![(panel, local.clone(), Some(working.clone()))], &ctx),
        "the same ledger of the same panel asks for no repaint"
    );
    assert_eq!(app.panel_attention(&local), Attention::Working);
    // A turn that ends asks for the one repaint that shows it.
    assert!(app.apply_attention_values(
        vec![(panel, local.clone(), Some(ledger(TurnState::Finished, now - 1)))],
        &ctx
    ));
    assert_eq!(app.panel_attention(&local), Attention::Done);
}

#[test]
fn a_finished_turn_is_done_until_the_person_clicks_the_row() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("a");
    let panel = agent_panel(&mut app, workspace, "Fix the sync");
    let local = local_id(&app, panel);
    let now = horizon_core::current_unix_millis();
    app.board.focused = None;
    app.apply_attention_values(
        vec![(panel, local.clone(), Some(ledger(TurnState::Working, now - 5)))],
        &wake(),
    );
    app.apply_attention_values(
        vec![(panel, local.clone(), Some(ledger(TurnState::Finished, now - 1)))],
        &wake(),
    );
    assert_eq!(app.panel_attention(&local), Attention::Done);
    app.note_panel_attention_look(panel);
    assert_eq!(app.panel_attention(&local), Attention::Idle);
}

#[test]
fn mark_as_unread_marks_an_idle_agent_panel_and_a_look_clears_it() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("a");
    let panel = agent_panel(&mut app, workspace, "Fix the sync");
    let local = local_id(&app, panel);
    assert_eq!(app.panel_attention(&local), Attention::Idle);
    app.mark_panel_unread(panel);
    assert_eq!(app.panel_attention(&local), Attention::Done);
    app.note_panel_attention_look(panel);
    assert_eq!(app.panel_attention(&local), Attention::Idle);
}

#[test]
fn an_open_question_of_the_ledger_does_not_clear_on_a_look() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("a");
    let panel = agent_panel(&mut app, workspace, "Fix the sync");
    let local = local_id(&app, panel);
    let now = horizon_core::current_unix_millis();
    app.apply_attention_values(
        vec![(panel, local.clone(), Some(ledger(TurnState::Blocked, now)))],
        &wake(),
    );
    assert_eq!(app.panel_attention(&local), Attention::NeedsYou);
    app.note_panel_attention_look(panel);
    assert_eq!(app.panel_attention(&local), Attention::NeedsYou);
}

#[test]
fn a_workspace_row_shows_the_heaviest_state_and_the_count_of_the_agents_that_wait() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("a");
    let quiet = agent_panel(&mut app, workspace, "Read the code");
    let asking = agent_panel(&mut app, workspace, "Fix the sync");
    let finished = agent_panel(&mut app, workspace, "Write the docs");
    let now = horizon_core::current_unix_millis();
    // The person looks at the first panel, so the others finish unseen.
    app.board.focused = Some(quiet);
    for (panel, states) in [
        (quiet, vec![TurnState::Working]),
        (asking, vec![TurnState::Blocked]),
        (finished, vec![TurnState::Working, TurnState::Finished]),
    ] {
        let local = local_id(&app, panel);
        let count = i64::try_from(states.len()).expect("states");
        for (index, state) in states.into_iter().enumerate() {
            let at = now - count + i64::try_from(index).expect("index");
            app.apply_attention_values(vec![(panel, local.clone(), Some(ledger(state, at)))], &wake());
        }
    }
    let rows = app.read_sidebar_rows();
    let row = rows.get(&workspace).expect("row");
    assert_eq!(row.attention, Attention::NeedsYou);
    assert_eq!(row.waiting.needs_you, 1);
    assert_eq!(row.waiting.done, 1);
    assert!(
        row.line.starts_with("1 need you \u{00B7} 1 done"),
        "the count comes first: {}",
        row.line
    );

    // A workspace of agents that work shows work and counts nothing.
    let other = app.board.create_workspace("b");
    let working = agent_panel(&mut app, other, "Run the tests");
    let local = local_id(&app, working);
    app.apply_attention_values(vec![(working, local, Some(ledger(TurnState::Working, now)))], &wake());
    let rows = app.read_sidebar_rows();
    let row = rows.get(&other).expect("row");
    assert_eq!(row.attention, Attention::Working);
    assert_eq!(row.waiting, horizon_core::workspace_list::Waiting::default());
    assert!(!row.line.contains("need you"));
}

#[test]
fn a_panel_without_a_ledger_keeps_its_working_indicator() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("a");
    let panel = agent_panel(&mut app, workspace, "Fix the sync");
    let local = local_id(&app, panel);
    let now = horizon_core::current_unix_millis();
    app.attention.states.observe_working(&local, true, true, now);
    assert_eq!(app.panel_attention(&local), Attention::Working);
    app.attention.states.observe_working(&local, false, false, now + 1);
    assert_eq!(app.panel_attention(&local), Attention::Done);
}

#[test]
fn the_attention_reads_never_run_on_the_frame() {
    let (_temp, ctx, mut app) = test_app_with_config_and_startup(
        &horizon_core::Config::default(),
        horizon_core::StartupDecision::Ephemeral {
            runtime_state: Box::new(horizon_core::RuntimeState::default()),
        },
    );
    let workspace = app.board.create_workspace("a");
    agent_panel(&mut app, workspace, "Fix the sync");
    // The whole suite runs the UI-thread guard in strict mode: a file read on a
    // frame panics here instead of freezing the window.
    let start = Instant::now();
    run_app_frame(&ctx, &mut app);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "the frame did not wait for a read"
    );
}

#[test]
fn the_attention_worker_reads_a_ledger_of_the_home_off_the_frame() {
    let temp = tempfile::tempdir().expect("home");
    let store = horizon_core::agent_work::WorkStore::new(temp.path());
    store
        .register_owner("panel-1", PanelKind::Claude, "owner", Some("session"), temp.path())
        .expect("owner");
    let state = super::AttentionState::new(temp.path());
    let worker = state.worker.as_ref().expect("the attention worker");
    worker.ask(vec![(PanelId(1), "panel-1".to_owned())]);
    let read = worker
        .reads
        .recv_timeout(Duration::from_secs(10))
        .expect("a read of the ledger");
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].1, "panel-1");
    assert_eq!(read[0].2.as_ref().expect("a registered ledger").session_id, "session");
}
