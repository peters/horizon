//! The desktop notification of a change that no window of Horizon showed: which
//! change sends one, which one sends nothing, and the line that arrives.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use egui::Context;
use horizon_core::agent_work::{TurnLedger, TurnState};
use horizon_core::notify::{GROUP, Notice, Sink};
use horizon_core::{Attention, Config, NotificationSetting, PanelId, PanelKind, PanelOptions, StartupDecision};

use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_config_and_startup};

use super::Notifications;

/// The sink of a test: it records the line that it would show.
#[derive(Clone, Default)]
struct Recorder {
    sent: Arc<Mutex<Vec<Notice>>>,
    available: Option<bool>,
}

impl Sink for Recorder {
    fn send(&self, notice: Notice) {
        self.sent.lock().unwrap_or_else(PoisonError::into_inner).push(notice);
    }

    fn available(&self) -> Option<bool> {
        self.available
    }
}

impl Recorder {
    fn lines(&self) -> Vec<String> {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|notice| {
                assert_eq!(notice.title, horizon_core::notify::TITLE);
                notice.body.clone()
            })
            .collect()
    }
}

fn ledger(state: TurnState, updated_at_millis: i64) -> TurnLedger {
    let mut ledger = TurnLedger::default();
    ledger.session_id = "session".into();
    ledger.prompt_id = Some("prompt".into());
    ledger.state = state;
    ledger.updated_at_millis = updated_at_millis;
    ledger
}

/// The application of one test, with a sink that records instead of showing.
fn app_with(setting: NotificationSetting) -> (tempfile::TempDir, Context, crate::app::HorizonApp, Recorder) {
    let mut config = Config::default();
    config.features.notifications = setting;
    let (temp, ctx, mut app) = test_app_with_config_and_startup(
        &config,
        StartupDecision::Ephemeral {
            runtime_state: Box::new(horizon_core::RuntimeState::default()),
        },
    );
    let recorder = Recorder::default();
    app.notifications = Notifications::with_sink(Box::new(recorder.clone()));
    (temp, ctx, app, recorder)
}

/// A panel of the board that runs nothing: these tests read the notification of a
/// state, not a terminal.
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

fn local_id(app: &crate::app::HorizonApp, panel: PanelId) -> String {
    app.board.panel(panel).expect("panel").local_id.clone()
}

/// Runs one frame of a window with `focused` as the focus of the desktop. The
/// frame moves the keyboard focus of the board to its panel, so the tests look
/// away again after it.
fn focus(ctx: &Context, app: &mut crate::app::HorizonApp, focused: bool) {
    let mut input = raw_input([1400.0, 900.0], None);
    input.focused = focused;
    run_app_frame_with_input(ctx, app, input);
    app.board.focused = None;
    assert_eq!(super::window_in_front(ctx), focused, "the frame heard the focus");
}

/// Applies one state of a ledger to `panel`, with the clock at the end of the
/// group of the changes.
fn change_and_poll(app: &mut crate::app::HorizonApp, ctx: &Context, panel: PanelId, state: TurnState) {
    let local = local_id(app, panel);
    app.apply_attention_values(
        vec![(panel, local, Some(ledger(state, horizon_core::current_unix_millis())))],
        ctx,
    );
    app.notifications
        .poll(app.board.workspaces.len() > 1, Instant::now() + GROUP, ctx);
}

#[test]
fn a_change_while_horizon_is_in_front_sends_nothing() {
    let (_temp, ctx, mut app, recorder) = app_with(NotificationSetting::NeedsYouAndDone);
    let workspace = app.board.create_workspace("Lane");
    let panel = agent_panel(&mut app, workspace, "Fix the sync");
    focus(&ctx, &mut app, true);
    change_and_poll(&mut app, &ctx, panel, TurnState::Blocked);
    assert_eq!(app.panel_attention(&local_id(&app, panel)), Attention::NeedsYou);
    assert!(
        recorder.lines().is_empty(),
        "the sidebar shows the change of a person who is here"
    );
}

#[test]
fn a_change_while_no_window_is_in_front_sends_one_line() {
    let (_temp, ctx, mut app, recorder) = app_with(NotificationSetting::NeedsYouAndDone);
    let workspace = app.board.create_workspace("Lane");
    let panel = agent_panel(&mut app, workspace, "Fix the sync");
    focus(&ctx, &mut app, false);
    change_and_poll(&mut app, &ctx, panel, TurnState::Blocked);
    assert_eq!(recorder.lines(), vec!["Fix the sync needs you"]);
}

#[test]
fn one_workspace_needs_no_workspace_name() {
    let (_temp, ctx, mut app, recorder) = app_with(NotificationSetting::NeedsYouAndDone);
    let workspace = app.board.create_workspace("Lane");
    let panel = agent_panel(&mut app, workspace, "Fix the sync");
    focus(&ctx, &mut app, false);
    // The board holds one workspace, so the name of the panel is the whole line.
    change_and_poll(&mut app, &ctx, panel, TurnState::Blocked);
    assert_eq!(app.board.workspaces.len(), 1);
    assert_eq!(recorder.lines(), vec!["Fix the sync needs you"]);
}

#[test]
fn several_changes_become_one_count() {
    let (_temp, ctx, mut app, recorder) = app_with(NotificationSetting::NeedsYouAndDone);
    let workspace = app.board.create_workspace("Lane");
    let panels = [
        agent_panel(&mut app, workspace, "Fix the sync"),
        agent_panel(&mut app, workspace, "Write the docs"),
        agent_panel(&mut app, workspace, "Run the tests"),
    ];
    focus(&ctx, &mut app, false);
    for panel in panels {
        let local = local_id(&app, panel);
        app.apply_attention_values(
            vec![(
                panel,
                local,
                Some(ledger(TurnState::Blocked, horizon_core::current_unix_millis())),
            )],
            &ctx,
        );
    }
    app.notifications
        .poll(app.board.workspaces.len() > 1, Instant::now() + GROUP, &ctx);
    assert_eq!(recorder.lines(), vec!["3 agents need you"]);
}

#[test]
fn a_finished_panel_tells_its_line_when_the_setting_does() {
    let (_temp, ctx, mut app, recorder) = app_with(NotificationSetting::NeedsYouAndDone);
    let workspace = app.board.create_workspace("Lane");
    let panel = agent_panel(&mut app, workspace, "Write the docs");
    focus(&ctx, &mut app, false);
    let now = horizon_core::current_unix_millis();
    let local = local_id(&app, panel);
    // A turn that worked, and then ended while the person was elsewhere.
    app.apply_attention_values(
        vec![(panel, local.clone(), Some(ledger(TurnState::Working, now - 2)))],
        &ctx,
    );
    eprintln!("DEBUG after working: {:?}", app.panel_attention(&local_id(&app, panel)));
    app.apply_attention_values(vec![(panel, local, Some(ledger(TurnState::Finished, now - 1)))], &ctx);
    eprintln!(
        "DEBUG after finished: {:?} waiting={}",
        app.panel_attention(&local_id(&app, panel)),
        app.notifications.waiting()
    );
    assert_eq!(app.panel_attention(&local_id(&app, panel)), Attention::Done);
    app.notifications
        .poll(app.board.workspaces.len() > 1, Instant::now() + GROUP, &ctx);
    assert_eq!(recorder.lines(), vec!["Write the docs is done"]);
}

#[test]
fn a_finished_panel_sends_nothing_when_the_setting_tells_only_the_question() {
    let (_temp, ctx, mut app, recorder) = app_with(NotificationSetting::NeedsYou);
    let workspace = app.board.create_workspace("Lane");
    let panel = agent_panel(&mut app, workspace, "Write the docs");
    focus(&ctx, &mut app, false);
    let now = horizon_core::current_unix_millis();
    let local = local_id(&app, panel);
    app.apply_attention_values(
        vec![(panel, local.clone(), Some(ledger(TurnState::Working, now - 2)))],
        &ctx,
    );
    app.apply_attention_values(vec![(panel, local, Some(ledger(TurnState::Finished, now - 1)))], &ctx);
    assert_eq!(app.panel_attention(&local_id(&app, panel)), Attention::Done);
    app.notifications
        .poll(app.board.workspaces.len() > 1, Instant::now() + GROUP, &ctx);
    assert!(recorder.lines().is_empty(), "the setting tells the question alone");
}

#[test]
fn a_panel_that_the_person_marked_unread_sends_nothing() {
    let (_temp, ctx, mut app, recorder) = app_with(NotificationSetting::NeedsYouAndDone);
    let workspace = app.board.create_workspace("Lane");
    let panel = agent_panel(&mut app, workspace, "Write the docs");
    focus(&ctx, &mut app, false);
    app.mark_panel_unread(panel);
    assert_eq!(app.panel_attention(&local_id(&app, panel)), Attention::Done);
    app.notifications
        .poll(app.board.workspaces.len() > 1, Instant::now() + GROUP, &ctx);
    assert!(
        recorder.lines().is_empty(),
        "a mark of the person is no change that an agent made"
    );
}

#[test]
fn the_setting_off_sends_nothing() {
    let (_temp, ctx, mut app, recorder) = app_with(NotificationSetting::Off);
    let workspace = app.board.create_workspace("Lane");
    let panel = agent_panel(&mut app, workspace, "Fix the sync");
    focus(&ctx, &mut app, false);
    change_and_poll(&mut app, &ctx, panel, TurnState::Blocked);
    assert_eq!(app.panel_attention(&local_id(&app, panel)), Attention::NeedsYou);
    assert!(recorder.lines().is_empty());
}

#[test]
fn a_person_who_comes_back_inside_the_two_seconds_gets_no_notification() {
    let (_temp, ctx, mut app, recorder) = app_with(NotificationSetting::NeedsYouAndDone);
    let workspace = app.board.create_workspace("Lane");
    let panel = agent_panel(&mut app, workspace, "Fix the sync");
    focus(&ctx, &mut app, false);
    let local = local_id(&app, panel);
    app.apply_attention_values(
        vec![(
            panel,
            local,
            Some(ledger(TurnState::Blocked, horizon_core::current_unix_millis())),
        )],
        &ctx,
    );
    assert!(app.notifications.waiting(), "the change waits for its group");
    // The person comes back: the sidebar shows the change, and it stops waiting.
    focus(&ctx, &mut app, true);
    assert!(!app.notifications.waiting());
    app.notifications
        .poll(app.board.workspaces.len() > 1, Instant::now() + GROUP, &ctx);
    assert!(recorder.lines().is_empty());
}

#[test]
fn a_workspace_in_a_window_of_its_own_counts_as_horizon_in_front() {
    let (_temp, ctx, mut app, recorder) = app_with(NotificationSetting::NeedsYouAndDone);
    let workspace = app.board.create_workspace("Lane");
    let other = app.board.create_workspace("Elsewhere");
    let panel = agent_panel(&mut app, workspace, "Fix the sync");
    let local = app.board.workspace(other).expect("workspace").local_id.clone();
    app.detach_workspace(other);
    app.board.focused = None;

    let mut input = raw_input([1400.0, 900.0], None);
    input.focused = false;
    input
        .viewports
        .entry(super::super::detached_viewports::detached_viewport_id(&local))
        .or_default()
        .focused = Some(true);
    run_app_frame_with_input(&ctx, &mut app, input);
    assert!(
        super::window_in_front(&ctx),
        "the window of the workspace has the focus"
    );

    change_and_poll(&mut app, &ctx, panel, TurnState::Blocked);
    assert_eq!(app.panel_attention(&local_id(&app, panel)), Attention::NeedsYou);
    assert!(recorder.lines().is_empty(), "a window of Horizon shows the change");
}

#[test]
fn a_frame_waits_for_the_group_and_runs_no_program() {
    // The suite runs the UI-thread guard in strict mode: a lookup or a start of a
    // program on this frame would panic instead of waiting.
    let (_temp, ctx, mut app, recorder) = app_with(NotificationSetting::NeedsYouAndDone);
    let workspace = app.board.create_workspace("Lane");
    let panel = agent_panel(&mut app, workspace, "Fix the sync");
    focus(&ctx, &mut app, false);
    let local = local_id(&app, panel);
    app.apply_attention_values(
        vec![(
            panel,
            local,
            Some(ledger(TurnState::Blocked, horizon_core::current_unix_millis())),
        )],
        &ctx,
    );
    // A frame of the window polls the bounds, and it does not wait for the group.
    let mut input = raw_input([1400.0, 900.0], None);
    input.focused = false;
    run_app_frame_with_input(&ctx, &mut app, input);
    assert!(recorder.lines().is_empty(), "the frame runs before the group ends");
    assert!(app.notifications.waiting());
    app.notifications
        .poll(app.board.workspaces.len() > 1, Instant::now() + GROUP, &ctx);
    assert_eq!(recorder.lines(), vec!["Fix the sync needs you"]);
}

#[test]
fn the_sentence_of_a_computer_without_the_program_is_the_line_of_the_setting() {
    let (_temp, _ctx, mut app, _recorder) = app_with(NotificationSetting::NeedsYouAndDone);
    assert_eq!(
        app.notifications.missing_program_sentence(),
        None,
        "the run has not looked yet"
    );
    let absent = Recorder {
        available: Some(false),
        ..Recorder::default()
    };
    app.notifications = Notifications::with_sink(Box::new(absent));
    assert_eq!(
        app.notifications.missing_program_sentence().as_deref(),
        Some("notify-send is not installed")
    );
}

#[test]
fn a_change_of_a_panel_that_left_the_board_tells_nothing() {
    let (_temp, ctx, mut app, recorder) = app_with(NotificationSetting::NeedsYouAndDone);
    let _ = app.board.create_workspace("Lane");
    focus(&ctx, &mut app, false);
    app.note_attention_changes(vec![("gone".to_owned(), Attention::NeedsYou)], &ctx);
    assert!(!app.notifications.waiting(), "no panel of the board carries that id");
    app.notifications
        .poll(app.board.workspaces.len() > 1, Instant::now() + GROUP, &ctx);
    assert!(recorder.lines().is_empty());
}
