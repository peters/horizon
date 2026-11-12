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

/// The record of a launch whose hooks have not run yet: the launch registered
/// its session and its generation, and no event has arrived.
fn quiet_ledger(session: &str, generation: u64) -> TurnLedger {
    let mut ledger = TurnLedger::default();
    ledger.session_id = session.into();
    ledger.generation = generation;
    ledger
}

#[test]
fn a_ledger_that_has_heard_no_event_keeps_the_working_indicator_of_its_panel() {
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
    app.board.focused = None;
    // A read of the registered launch, before any event of it: the panel keeps
    // the signal of its own screen.
    app.apply_attention_values(vec![(panel, local.clone(), Some(quiet_ledger("session", 1)))], &ctx);
    assert!(!app.attention.states.from_ledger(&local));
    app.attention.states.observe_working(&local, true, false, now);
    assert_eq!(app.panel_attention(&local), Attention::Working);
    // The session start of the agent is the first event, and the ledger answers
    // from then on.
    let mut session_start = quiet_ledger("session", 1);
    session_start.updated_at_millis = now;
    assert!(app.apply_attention_values(vec![(panel, local.clone(), Some(session_start))], &ctx));
    assert!(app.attention.states.from_ledger(&local));
    assert_eq!(app.panel_attention(&local), Attention::Idle);
    app.attention.states.observe_working(&local, true, false, now + 1);
    assert_eq!(
        app.panel_attention(&local),
        Attention::Idle,
        "the screen cannot decide a panel that its ledger describes"
    );
}

#[test]
fn the_state_of_an_observed_panel_with_the_policy_off_comes_from_a_read_alone() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("a");
    let panel = agent_panel(&mut app, workspace, "Fix the sync");
    let local = local_id(&app, panel);
    let now = horizon_core::current_unix_millis();
    // The panel's continuation policy is off, and Horizon still reads its
    // ledger: the sidebar shows the state that the hooks report.
    assert!(!app.board.panel(panel).expect("panel").work_resume.enabled);
    app.board.focused = None;
    app.apply_attention_values(
        vec![(panel, local.clone(), Some(ledger(TurnState::Blocked, now)))],
        &wake(),
    );
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
        row.line.starts_with("1 needs you \u{00B7} 1 done"),
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

#[test]
fn the_jump_goes_to_the_oldest_agent_that_waits_across_workspaces() {
    let (_temp, ctx, mut app) = test_app_with_config_and_startup(
        &horizon_core::Config::default(),
        horizon_core::StartupDecision::Ephemeral {
            runtime_state: Box::new(horizon_core::RuntimeState::default()),
        },
    );
    let first_workspace = app.board.create_workspace("first");
    let second_workspace = app.board.create_workspace("second");
    let done = agent_panel(&mut app, first_workspace, "Write the docs");
    let asking = agent_panel(&mut app, second_workspace, "Fix the sync");
    let now = horizon_core::current_unix_millis();
    app.board.focused = None;
    for (panel, states) in [
        (done, vec![TurnState::Working, TurnState::Finished]),
        (asking, vec![TurnState::Blocked]),
    ] {
        let local = local_id(&app, panel);
        let count = i64::try_from(states.len()).expect("states");
        for (index, state) in states.into_iter().enumerate() {
            let at = now - count + i64::try_from(index).expect("index");
            app.apply_attention_values(vec![(panel, local.clone(), Some(ledger(state, at)))], &wake());
        }
    }
    assert_eq!(app.panel_attention(&local_id(&app, asking)), Attention::NeedsYou);
    assert_eq!(app.panel_attention(&local_id(&app, done)), Attention::Done);
    assert_eq!(app.attention_targets().len(), 2);

    // The panel that needs the person comes first, also from another workspace.
    app.go_to_next_attention(&ctx);
    assert_eq!(app.board.focused, Some(asking));
    assert_eq!(app.board.active_workspace, Some(second_workspace));
    // The next press leaves that panel out and reaches the finished one.
    app.go_to_next_attention(&ctx);
    assert_eq!(app.board.focused, Some(done));
    assert_eq!(app.board.active_workspace, Some(first_workspace));
}

#[test]
fn the_jump_says_one_line_when_no_agent_waits() {
    let (_temp, ctx, mut app) = test_app_with_config_and_startup(
        &horizon_core::Config::default(),
        horizon_core::StartupDecision::Ephemeral {
            runtime_state: Box::new(horizon_core::RuntimeState::default()),
        },
    );
    let workspace = app.board.create_workspace("a");
    let panel = agent_panel(&mut app, workspace, "Run the tests");
    let local = local_id(&app, panel);
    let now = horizon_core::current_unix_millis();
    app.apply_attention_values(vec![(panel, local, Some(ledger(TurnState::Working, now - 1)))], &wake());
    app.board.focused = None;
    app.go_to_next_attention(&ctx);
    assert_eq!(app.sidebar_notice.as_deref(), Some(super::NOBODY_WAITS));
    assert_eq!(
        app.sidebar_notice_tone,
        crate::app::sidebar::machines::SidebarNoticeTone::Plain,
        "an answer to a key press is not an error"
    );
    assert_eq!(app.board.focused, None, "nothing took the focus");
}

/// One press of `binding` in the input of a frame.
fn shortcut_press(binding: horizon_core::ShortcutBinding) -> egui::Event {
    let key = match binding.key {
        horizon_core::ShortcutKey::Letter(letter) => {
            egui::Key::from_name(&letter.to_string()).expect("key of the letter")
        }
        other => panic!("the test presses a letter: {other:?}"),
    };
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers {
            alt: binding.modifiers.alt(),
            ctrl: binding.modifiers.ctrl(),
            shift: binding.modifiers.shift(),
            mac_cmd: binding.modifiers.mac_cmd(),
            command: binding.modifiers.command(),
        },
    }
}

/// The key events that are left for the panel that a shortcut focused.
fn remaining_keys(ctx: &egui::Context) -> Vec<String> {
    ctx.input(|input| {
        input
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Key { key, pressed: true, .. } => Some(format!("{key:?}")),
                egui::Event::Text(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    })
}

#[test]
fn a_shortcut_that_moves_the_focus_types_nothing_into_the_panel_that_it_focuses() {
    use crate::app::test_support::{raw_input, run_app_frame_with_input};

    let (_temp, ctx, mut app) = test_app_with_config_and_startup(
        &horizon_core::Config::default(),
        horizon_core::StartupDecision::Ephemeral {
            runtime_state: Box::new(horizon_core::RuntimeState::default()),
        },
    );
    let here = app.board.create_workspace("here");
    let there = app.board.create_workspace("there");
    let shell = agent_panel(&mut app, here, "Notes");
    let asking = agent_panel(&mut app, there, "Fix the sync");
    let now = horizon_core::current_unix_millis();
    app.apply_attention_values(
        vec![(
            asking,
            local_id(&app, asking),
            Some(ledger(TurnState::Blocked, now - 1)),
        )],
        &wake(),
    );
    assert_eq!(app.panel_attention(&local_id(&app, asking)), Attention::NeedsYou);
    app.board.focused = Some(shell);
    app.board.active_workspace = Some(here);

    // The jump of Ctrl+Shift+G takes the focus into the other workspace.
    let mut input = raw_input([1400.0, 900.0], None);
    input.focused = true;
    input.events.push(shortcut_press(app.shortcuts.next_attention));
    run_app_frame_with_input(&ctx, &mut app, input);
    assert_eq!(app.board.focused, Some(asking), "the jump reached the agent that waits");
    assert!(
        remaining_keys(&ctx).is_empty(),
        "the panel that the jump focused reads no key of the chord: {:?}",
        remaining_keys(&ctx)
    );

    // An older shortcut that moves the focus keeps the same rule.
    app.board.active_workspace = Some(there);
    app.board.focused = None;
    let mut input = raw_input([1400.0, 900.0], None);
    input.focused = true;
    input.events.push(shortcut_press(app.shortcuts.focus_active_workspace));
    run_app_frame_with_input(&ctx, &mut app, input);
    assert!(
        app.board.focused.is_some(),
        "the shortcut focused the panel of the workspace"
    );
    assert!(
        remaining_keys(&ctx).is_empty(),
        "the panel that the shortcut focused reads no key of the chord: {:?}",
        remaining_keys(&ctx)
    );
}
