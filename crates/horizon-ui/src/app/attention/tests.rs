//! The tests of the attention state of the agent panels: which panel needs the
//! person, what the sidebar shows and what a look clears.
use std::time::{Duration, Instant};

use horizon_core::agent_work::{TurnLedger, TurnState};
use horizon_core::{Attention, PanelId, PanelKind, PanelOptions};

use crate::app::test_support::{run_app_frame, test_app, test_app_with_config_and_startup};

/// The time a test gives a worker of the window to answer.
///
/// The bound is a budget of the test and not a rule of the product: a frame
/// starts the worker, and a computer that runs this suite beside other work can
/// take seconds to schedule it and to write its file.
const PATIENCE: Duration = Duration::from_secs(180);

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
fn the_record_of_a_closed_panel_leaves_the_home() {
    let (temp, ctx, mut app) = test_app_with_config_and_startup(
        &horizon_core::Config::default(),
        horizon_core::StartupDecision::Ephemeral {
            runtime_state: Box::new(horizon_core::RuntimeState::default()),
        },
    );
    let workspace = app.board.create_workspace("a");
    let panel = agent_panel(&mut app, workspace, "Fix the sync");
    let local = local_id(&app, panel);
    // The launch of the panel and the close that follows it resolve the home of
    // the process, so the record of this test lies where Horizon keeps it.
    let store = horizon_core::agent_work::WorkStore::new(horizon_core::HorizonHome::resolve().root());
    store
        .register_owner(&local, PanelKind::Claude, "owner", Some("session"), temp.path())
        .expect("registered launch");
    assert!(store.read(&local).expect("read").is_some());
    // Closing a panel is a frame action, and the record must leave the home
    // without the frame waiting for the file. The whole suite runs the UI-thread
    // guard in strict mode, so a read or a delete on the frame panics here.
    run_app_frame(&ctx, &mut app);
    app.board.close_panel(panel);
    run_app_frame(&ctx, &mut app);
    let deadline = Instant::now() + PATIENCE;
    while store.read(&local).expect("read").is_some() {
        assert!(Instant::now() < deadline, "the record of the closed panel stays");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app.panel_attention(&local), Attention::Idle);
}

#[test]
fn an_observed_panel_with_the_policy_off_offers_no_continuation() {
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
    assert!(!app.board.panel(panel).expect("panel").work_resume.enabled);
    app.board.focused = None;
    app.apply_attention_values(
        vec![(panel, local.clone(), Some(ledger(TurnState::Blocked, now)))],
        &ctx,
    );
    // The sidebar follows the ledger of the panel, and the panel offers no
    // resume and shows no card of a continuation.
    assert_eq!(app.panel_attention(&local), Attention::NeedsYou);
    assert!(
        app.board
            .panel(panel)
            .and_then(horizon_core::Panel::terminal)
            .expect("terminal")
            .pending_work_resume()
            .is_none(),
        "an observed panel with the policy off waits for no continuation"
    );
    assert_eq!(app.work_resume_overlay_rect(&ctx), None, "no card is drawn");
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
    let panel = agent_panel(&mut app, workspace, "Fix the sync");
    let local = local_id(&app, panel);
    assert_eq!(app.panel_attention(&local), Attention::Idle);
    // The whole suite runs the UI-thread guard in strict mode: a file read on a
    // frame panics here instead of freezing the window.
    run_app_frame(&ctx, &mut app);
    // The frame asked the worker for one read and took no answer, so no frame of
    // the window waits for a file, however long a busy computer holds the worker.
    assert!(app.attention.in_flight, "the frame asks the worker for one read");
    assert_eq!(
        app.panel_attention(&local),
        Attention::Idle,
        "the frame reads no ledger of its own"
    );
    let worker = app.attention.worker.as_ref().expect("the attention worker");
    let read = worker
        .reads
        .recv_timeout(PATIENCE)
        .expect("the read that the frame asked for");
    assert_eq!(read.len(), 1, "the read names the one agent panel of the board");
    assert_eq!(read[0].1, local);
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
    let read = worker.reads.recv_timeout(PATIENCE).expect("a read of the ledger");
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

/// A card that asks the person `text` with `answers`, as the plan card does
/// before the go.
fn asking_card(text: &str, answers: &[(&str, &str)]) -> horizon_browser_control::block::BlockDocument {
    use horizon_browser_control::block::{BlockDocument, Button, Component, Tone};
    BlockDocument {
        width: None,
        finished: false,
        components: vec![
            Component::Text {
                text: "3 todos in 2 waves".to_owned(),
            },
            Component::Callout {
                tone: Tone::Neutral,
                caption: Some("Nothing starts before Go".to_owned()),
                text: text.to_owned(),
                buttons: answers
                    .iter()
                    .map(|(id, label)| Button {
                        id: (*id).to_owned(),
                        label: (*label).to_owned(),
                        tone: Tone::Neutral,
                    })
                    .collect(),
            },
        ],
    }
}

/// A card panel of `workspace` that draws `document`.
fn card_panel(
    app: &mut crate::app::HorizonApp,
    workspace: horizon_core::WorkspaceId,
    document: horizon_browser_control::block::BlockDocument,
) -> PanelId {
    app.board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Block,
                block: Some(horizon_core::BlockPanelState {
                    title: "The plan".to_owned(),
                    ..horizon_core::BlockPanelState::new(document)
                }),
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("card")
}

/// The attention of the sidebar row of `panel`, as the sidebar reads it.
fn row_attention(app: &crate::app::HorizonApp, panel: PanelId) -> Attention {
    app.panel_attention_of(app.board.panel(panel).expect("panel"))
}

/// The card panel that a restart of `app` brings back from `saved`, as the saved
/// state of the session holds it.
fn restored_card(
    app: &mut crate::app::HorizonApp,
    workspace: horizon_core::WorkspaceId,
    saved: &horizon_core::BlockPanelState,
) -> PanelId {
    let state: horizon_core::BlockPanelState =
        serde_json::from_str(&serde_json::to_string(saved).expect("the card saves")).expect("the card loads");
    app.board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Block,
                block: Some(state),
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("the restored card")
}

/// The callout of one document, as the card that asks it holds it.
fn callout_of(document: &horizon_browser_control::block::BlockDocument) -> horizon_browser_control::block::Component {
    document
        .components
        .iter()
        .find(|component| matches!(component, horizon_browser_control::block::Component::Callout { .. }))
        .cloned()
        .expect("the card asks a question")
}

#[test]
fn a_restored_card_reads_the_answer_that_was_saved_with_it() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("a");
    let document = asking_card("Run the plan?", &[("go", "Go"), ("stop", "Stop")]);
    let callout = callout_of(&document);

    // A card that the person answered before the restart stays quiet.
    let mut answered = horizon_core::BlockPanelState::new(document.clone());
    answered.record_answer(callout);
    let quiet = restored_card(&mut app, workspace, &answered);
    assert_eq!(
        row_attention(&app, quiet),
        Attention::Idle,
        "the answer is saved with the card, so a restart does not ask again"
    );

    // A callout that nobody answered still marks its row after a restart.
    let asking = restored_card(&mut app, workspace, &horizon_core::BlockPanelState::new(document));
    assert_eq!(row_attention(&app, asking), Attention::NeedsYou);

    // The same card with another question asks again, because that answer was
    // for the question that the person read.
    let mut other = horizon_core::BlockPanelState::new(asking_card("Push the branch?", &[("push", "Push the branch")]));
    other.record_answer(callout_of(&asking_card(
        "Run the plan?",
        &[("go", "Go"), ("stop", "Stop")],
    )));
    let asked_again = restored_card(&mut app, workspace, &other);
    assert_eq!(row_attention(&app, asked_again), Attention::NeedsYou);
}

#[test]
fn a_card_that_asks_the_person_in_a_callout_marks_its_sidebar_row_and_a_press_ends_it() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("a");
    let answers = [("go", "Go"), ("stop", "Stop")];
    let asking = card_panel(&mut app, workspace, asking_card("Run the plan?", &answers));
    assert_eq!(
        row_attention(&app, asking),
        Attention::NeedsYou,
        "a card that waits for a go marks its row"
    );

    let document = app
        .board
        .panel(asking)
        .and_then(|panel| panel.block())
        .map(|card| card.document.clone())
        .expect("the document of the card");
    let runtime = app.panel_render_caches.card_runtime.entry(asking).or_default();
    assert!(
        runtime.press("go", 1, &document, horizon_browser_control::manifest::now_millis()),
        "the press of the person is queued on the card"
    );
    assert_eq!(
        row_attention(&app, asking),
        Attention::Idle,
        "the answer of the person ends the mark of that callout"
    );

    // Another question is a new question, so the row asks again.
    let later = card_panel(
        &mut app,
        workspace,
        asking_card("Push the branch?", &[("push", "Push the branch")]),
    );
    assert_eq!(row_attention(&app, later), Attention::NeedsYou);
}

#[test]
fn a_card_without_a_callout_or_with_a_callout_of_no_answer_keeps_its_row_quiet() {
    use horizon_browser_control::block::{BlockDocument, Component};
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("a");
    let plain = card_panel(
        &mut app,
        workspace,
        BlockDocument {
            width: None,
            finished: false,
            components: vec![Component::Text {
                text: "the plan runs".to_owned(),
            }],
        },
    );
    assert_eq!(row_attention(&app, plain), Attention::Idle);
    let note = card_panel(&mut app, workspace, asking_card("Nothing to answer here", &[]));
    assert_eq!(row_attention(&app, note), Attention::Idle);
}

/// The five events of one turn of Claude Code 2.1.325, whose `Stop` sends
/// `background_tasks` and `session_crons` as lists.
const CLAUDE_2_1_325: &str = include_str!("../../agent_work_hook/claude-2.1.325.json");
/// The folder and the session of those events.
const CWD: &str = "/tmp/horizon-agent-work";
const SESSION: &str = "00000000-0000-0000-0000-000000000001";

/// Runs the frames of the window until the row of `local` shows `expected`. The
/// attention worker reads the records of the home, so an answer comes after a
/// frame or two.
fn wait_for_attention(ctx: &egui::Context, app: &mut crate::app::HorizonApp, local: &str, expected: Attention) {
    let deadline = Instant::now() + PATIENCE;
    while app.panel_attention(local) != expected {
        assert!(
            Instant::now() < deadline,
            "the row of the panel shows {expected:?} rather than {:?}",
            app.panel_attention(local)
        );
        run_app_frame(ctx, app);
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_turn_of_claude_2_1_325_moves_the_row_of_its_panel() {
    let (_temp, ctx, mut app) = test_app_with_config_and_startup(
        &horizon_core::Config::default(),
        horizon_core::StartupDecision::Ephemeral {
            runtime_state: Box::new(horizon_core::RuntimeState::default()),
        },
    );
    let workspace = app.board.create_workspace("a");
    let panel = agent_panel(&mut app, workspace, "Fix the sync");
    let local = local_id(&app, panel);
    app.board.focused = None;
    // The attention worker reads the records of the home of this window.
    let store = horizon_core::agent_work::WorkStore::new(app.session_store.home().root());
    store
        .register_owner(
            &local,
            PanelKind::Claude,
            "owner",
            Some(SESSION),
            std::path::Path::new(CWD),
        )
        .expect("the launch of the panel");
    let events: Vec<serde_json::Value> = serde_json::from_str(CLAUDE_2_1_325).expect("the fixture");
    let feed = |index: usize, at: i64| {
        let bytes = serde_json::to_vec(&events[index]).expect("the event");
        let input: horizon_core::agent_work::HookInput =
            serde_json::from_slice(&bytes).expect("this build reads every event of 2.1.325");
        store
            .apply_hook(&local, PanelKind::Claude, "owner", &input, at)
            .expect("the event is recorded");
    };
    let now = horizon_core::current_unix_millis();
    feed(1, now - 5);
    wait_for_attention(&ctx, &mut app, &local, Attention::Working);
    feed(4, now - 1);
    // The stop of that version ends the turn, so the row leaves `Working`. A lone
    // panel of the board has the focus of a frame, so its finished turn is the one
    // of a person who looks at it.
    let deadline = Instant::now() + PATIENCE;
    while app.panel_attention(&local) == Attention::Working {
        assert!(Instant::now() < deadline, "the stop of 2.1.325 ends the working turn");
        run_app_frame(&ctx, &mut app);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        matches!(app.panel_attention(&local), Attention::Idle | Attention::Done),
        "the finished turn of the panel: {:?}",
        app.panel_attention(&local)
    );
}
