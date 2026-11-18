//! The close of a window that holds no session lease. A second start of this
//! computer finds a window through that lease, so a window without one never
//! hides: nothing could show it again, and its programs would run on unseen.

use egui::{Context, Key, ViewportCommand, ViewportId};
use horizon_core::{
    Config, HorizonHome, PanelOptions, RuntimeState, SessionOpenDisposition, SessionStore, StartupChooser,
    StartupDecision, StartupPromptReason, WindowCloseSetting,
};

use super::{
    Recorder, ask_frame, button, close_frame, close_request, commands, exit_status, fill_of, press, press_key, running,
    settled_ask, texts, warm_up,
};
use crate::app::HorizonApp;
use crate::app::notify::Notifications;
use crate::app::test_support::{app_in, test_app_with_config_and_startup, wait_for_lease_writes};
use crate::theme;

const HEADING: &str = "Close Horizon?";

/// The application of one test with the start `startup`, which gives its window
/// no session lease, and a sink that records instead of showing.
fn app_of(setting: WindowCloseSetting, startup: StartupDecision) -> (tempfile::TempDir, Context, HorizonApp, Recorder) {
    let mut config = Config::default();
    config.features.when_window_closes = setting;
    let (temp, ctx, mut app) = test_app_with_config_and_startup(&config, startup);
    app.root_viewport_stabilizer = None;
    assert!(!app.holds_session_lease(), "the start gave this window no lease");
    let recorder = Recorder::default();
    app.notifications = Notifications::with_sink(Box::new(recorder.clone()));
    (temp, ctx, app, recorder)
}

/// The window of a start with `--ephemeral`, with one program in a panel.
fn ephemeral_app_with_a_program(setting: WindowCloseSetting) -> (tempfile::TempDir, Context, HorizonApp, Recorder) {
    let startup = StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    };
    let (temp, ctx, mut app, recorder) = app_of(setting, startup);
    let workspace = app.board.create_workspace("Ephemeral");
    let _ = app
        .create_panel_with_options(running(PanelOptions::default()), workspace)
        .expect("a shell panel");
    warm_up(&ctx, &mut app);
    (temp, ctx, app, recorder)
}

/// The ask of a window without a lease offers the action that ends the programs,
/// in red, and `Cancel`. It offers no hide, none of the help of a hide, and no
/// box, because the choice would not hold for a start that takes a lease.
#[test]
fn the_ask_of_a_window_without_a_lease_offers_no_hide_and_no_box() {
    let (_temp, ctx, mut app, _recorder) = ephemeral_app_with_a_program(WindowCloseSetting::Ask);
    let _ = close_frame(&ctx, &mut app);

    let output = settled_ask(&ctx, &mut app, HEADING);
    let drawn = texts(&output);

    assert!(drawn.contains(&"1 program is running."), "the count: {drawn:?}");
    assert!(
        !drawn.iter().any(|text| text.contains("Keep running")),
        "no action hides a window that no start can show again: {drawn:?}"
    );
    assert!(
        !drawn.iter().any(|text| text.contains("Start Horizon again")),
        "no help line of a hide: {drawn:?}"
    );
    assert!(!drawn.contains(&"Remember this choice"), "no box: {drawn:?}");
    assert_eq!(
        fill_of(&output, "End them and quit", HEADING),
        theme::blend(theme::PANEL_BG_ALT(), theme::PALETTE_RED(), 0.22),
        "the action that ends the programs is drawn as Delete is"
    );
    assert_eq!(
        fill_of(&output, "Cancel", HEADING),
        theme::PANEL_BG_ALT(),
        "Cancel keeps the plain fill"
    );
    app.board.shutdown_terminal_panels();
}

/// Enter takes no action of that ask: no key press ends a program, and no action
/// is left that keeps them.
#[test]
fn enter_answers_nothing_in_a_window_without_a_lease() {
    let (_temp, ctx, mut app, _recorder) = ephemeral_app_with_a_program(WindowCloseSetting::Ask);
    let _ = close_frame(&ctx, &mut app);
    let _ = settled_ask(&ctx, &mut app, HEADING);

    let output = press_key(&ctx, &mut app, Key::Enter);
    let sent = output
        .viewport_output
        .get(&ViewportId::ROOT)
        .map(|output| output.commands.clone())
        .unwrap_or_default();

    assert!(app.keep_running.ask.is_some(), "Enter leaves the ask");
    assert!(app.shutdown_progress.is_none(), "Enter ends no program");
    assert!(!sent.contains(&ViewportCommand::Visible(false)), "{sent:?}");
    assert!(!app.keep_running.hidden, "Enter hides nothing");
    app.board.shutdown_terminal_panels();
}

/// `End them and quit` ends the programs and Horizon, and it writes no choice.
#[test]
fn end_them_and_quit_ends_a_window_without_a_lease() {
    let (_temp, ctx, mut app, _recorder) = ephemeral_app_with_a_program(WindowCloseSetting::KeepRunning);
    let _ = close_frame(&ctx, &mut app);
    let output = ask_frame(&ctx, &mut app);

    let _ = press(&ctx, &mut app, button(&output, "End them and quit", HEADING));

    assert!(app.shutdown_progress.is_some(), "the programs and Horizon end");
    assert!(!app.keep_running.hidden, "the window is not hidden");
    assert!(app.keep_running.ask.is_none(), "the ask is answered");
    assert_eq!(
        app.template_config.features.when_window_closes,
        WindowCloseSetting::KeepRunning,
        "the answer of a window without a lease changes no setting"
    );
    assert_eq!(exit_status(&ctx, &mut app), 0, "the answer ends the process");
}

/// With the choice `keep_running`, the close of a window without a lease hides
/// nothing: it asks, as the choice `ask` does, and the person chooses.
#[test]
fn the_choice_keep_running_asks_in_a_window_without_a_lease() {
    let (_temp, ctx, mut app, recorder) = ephemeral_app_with_a_program(WindowCloseSetting::KeepRunning);

    let sent = commands(&ctx, &mut app, close_request());

    assert!(
        !sent.contains(&ViewportCommand::Visible(false)),
        "the window is not hidden: {sent:?}"
    );
    assert!(
        sent.contains(&ViewportCommand::CancelClose),
        "the ask holds its window: {sent:?}"
    );
    assert!(!app.keep_running.hidden, "no window is hidden");
    assert!(app.keep_running.ask.is_some(), "the person chooses");
    assert!(app.shutdown_progress.is_none(), "the close ends nothing yet");
    assert!(recorder.lines().is_empty(), "nothing is said about a hide");
    app.board.shutdown_terminal_panels();
}

/// A window that took no session yet, which shows the chooser of its start,
/// holds no lease too: with the choice `keep_running` its close hides nothing.
///
/// The board of a chooser holds no panel. This test gives it one, so the close
/// has a program to ask for.
#[test]
fn the_choice_keep_running_hides_no_window_that_took_no_session() {
    let chooser = StartupChooser {
        reason: StartupPromptReason::MultipleRecoverable,
        config_path: "config.json".to_owned(),
        sessions: Vec::new(),
    };
    let (_temp, ctx, mut app, recorder) = app_of(WindowCloseSetting::KeepRunning, StartupDecision::Choose(chooser));
    assert!(app.active_session.is_none(), "the window took no session");
    let workspace = app.board.create_workspace("Chooser");
    let _ = app
        .create_panel_with_options(running(PanelOptions::default()), workspace)
        .expect("a shell panel");

    let sent = commands(&ctx, &mut app, close_request());

    assert!(
        !sent.contains(&ViewportCommand::Visible(false)),
        "the window is not hidden: {sent:?}"
    );
    assert!(
        sent.contains(&ViewportCommand::CancelClose),
        "the ask holds its window: {sent:?}"
    );
    assert!(!app.keep_running.hidden, "no window is hidden");
    assert!(app.keep_running.ask.is_some(), "the person chooses");
    assert!(app.shutdown_progress.is_none(), "the close ends nothing yet");
    assert!(recorder.lines().is_empty(), "nothing is said about a hide");
    app.board.shutdown_terminal_panels();
}

/// The real chooser of a start, whose board holds no panel: with the choice
/// `keep_running` its close hides nothing and ends Horizon, as the choice `ask`
/// does when nothing runs.
#[test]
fn the_choice_keep_running_ends_a_chooser_that_runs_nothing() {
    let chooser = StartupChooser {
        reason: StartupPromptReason::MultipleRecoverable,
        config_path: "config.json".to_owned(),
        sessions: Vec::new(),
    };
    let (_temp, ctx, mut app, recorder) = app_of(WindowCloseSetting::KeepRunning, StartupDecision::Choose(chooser));
    assert!(app.active_session.is_none(), "the window took no session");

    let sent = commands(&ctx, &mut app, close_request());

    assert!(
        !sent.contains(&ViewportCommand::Visible(false)),
        "the window is not hidden: {sent:?}"
    );
    assert!(app.keep_running.ask.is_none(), "nothing runs, so nothing is asked");
    assert!(app.shutdown_progress.is_some(), "the close ends Horizon");
    assert_eq!(exit_status(&ctx, &mut app), 0, "the close ends the process");
    assert!(!app.keep_running.hidden, "no window is hidden");
    assert!(recorder.lines().is_empty(), "nothing is said about a hide");
}

/// One frame that writes the heartbeat of the lease, and the answer of the
/// session writer to it.
fn heartbeat(ctx: &Context, app: &mut HorizonApp) {
    app.active_session.as_mut().expect("a session").last_lease_refresh = None;
    warm_up(ctx, app);
    wait_for_lease_writes(app);
}

/// A lease counts from the answer of the session writer that wrote it, and until
/// a heartbeat of it fails: a second start reads the file, not the window. With
/// the choice `keep_running`, a window whose heartbeat failed asks and offers no
/// hide, and it hides again after a heartbeat that the writer wrote.
#[test]
fn a_lease_that_the_session_writer_did_not_write_is_no_lease() {
    let temp = tempfile::tempdir().expect("temp dir");
    let home = HorizonHome::from_root(temp.path().join(".horizon"));
    let session = SessionStore::new(home.clone(), temp.path().join("config.json"))
        .create_session_from_runtime(RuntimeState::default())
        .expect("a saved session");
    let lease_path = home.session_lease_path(&session.session_id);
    let mut config = Config::default();
    config.features.when_window_closes = WindowCloseSetting::KeepRunning;
    let ctx = Context::default();
    let startup = StartupDecision::Open {
        disposition: SessionOpenDisposition::New,
        session: Box::new(session),
    };
    let mut app = app_in(&ctx, temp.path(), &config, startup);
    app.root_viewport_stabilizer = None;

    assert!(!app.holds_session_lease(), "no answer of the writer yet, so no lease");
    wait_for_lease_writes(&mut app);
    assert!(lease_path.exists(), "the writer wrote the lease");
    assert!(app.holds_session_lease(), "the written lease is held");

    // A folder stands where the writer puts the file before it renames it, so
    // the heartbeat fails.
    let blocker = lease_path.with_extension("tmp");
    std::fs::create_dir(&blocker).expect("the folder that blocks the write");
    heartbeat(&ctx, &mut app);
    assert!(!app.holds_session_lease(), "a heartbeat that failed is no lease");

    let workspace = app.board.create_workspace("Session");
    let _ = app
        .create_panel_with_options(running(PanelOptions::default()), workspace)
        .expect("a shell panel");
    warm_up(&ctx, &mut app);
    let sent = commands(&ctx, &mut app, close_request());
    assert!(!sent.contains(&ViewportCommand::Visible(false)), "{sent:?}");
    assert!(!app.keep_running.hidden, "no window is hidden");
    let output = ask_frame(&ctx, &mut app);
    let drawn = texts(&output);
    assert!(drawn.contains(&"End them and quit"), "the ask: {drawn:?}");
    assert!(
        !drawn.iter().any(|text| text.contains("Keep running")),
        "no action hides a window that no start can find: {drawn:?}"
    );
    let _ = press(&ctx, &mut app, button(&output, "Cancel", HEADING));
    assert!(app.keep_running.ask.is_none(), "the ask is answered");

    std::fs::remove_dir(&blocker).expect("the write is free again");
    heartbeat(&ctx, &mut app);
    assert!(
        app.holds_session_lease(),
        "the next heartbeat that is written is a lease"
    );
    let sent = commands(&ctx, &mut app, close_request());
    assert!(sent.contains(&ViewportCommand::Visible(false)), "{sent:?}");
    assert!(app.keep_running.hidden, "the window hides again");

    // The answer of a release is the answer of a lease that is gone.
    app.release_active_session_lease();
    wait_for_lease_writes(&mut app);
    assert!(!lease_path.exists(), "the writer removed the lease");
    assert!(!app.holds_session_lease(), "a released lease is no lease");
    app.board.shutdown_terminal_panels();
}
