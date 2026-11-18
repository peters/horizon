//! The close of the main window with the setting on and off, the notice of the
//! first hide, and the end of Horizon from the menu.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Context, RawInput, ViewportCommand, ViewportId};
use horizon_core::notify::{Notice, Sink};
use horizon_core::{Config, RuntimeState, StartupDecision};

use crate::app::HorizonApp;
use crate::app::notify::Notifications;
use crate::app::root_chrome::ToolbarAction;
use crate::app::test_support::{run_app_frame_with_input, test_app_with_config_and_startup, wait_for_session_saves};

/// The sink of a test: it records the notices that Horizon would show.
#[derive(Clone, Default)]
struct Recorder {
    sent: Arc<Mutex<Vec<String>>>,
}

impl Sink for Recorder {
    fn send(&self, notice: Notice) {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(notice.body);
    }
}

impl Recorder {
    fn lines(&self) -> Vec<String> {
        self.sent.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// The input of a frame in which the person closes the main window.
fn close_request() -> RawInput {
    let mut input = RawInput::default();
    input
        .viewports
        .entry(ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    input
}

/// The commands that the frame of `input` sent to the main window.
fn commands(ctx: &Context, app: &mut HorizonApp, input: RawInput) -> Vec<ViewportCommand> {
    let output = run_app_frame_with_input(ctx, app, input);
    output
        .viewport_output
        .get(&ViewportId::ROOT)
        .map(|output| output.commands.clone())
        .unwrap_or_default()
}

/// The application of one test, with a sink that records instead of showing, and
/// a session whose board the run saves.
fn app_with(keep_running: bool) -> (tempfile::TempDir, Context, HorizonApp, Recorder) {
    let mut config = Config::default();
    config.features.keep_running_when_closed = keep_running;
    let (temp, ctx, mut app) = test_app_with_config_and_startup(
        &config,
        StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        },
    );
    app.root_viewport_stabilizer = None;
    app.active_session = Some(crate::app::ActiveSession {
        session_id: "keep-running".to_string(),
        lease: None,
        lease_answers: Vec::new(),
        last_lease_refresh: None,
        persistent: true,
    });
    let recorder = Recorder::default();
    app.notifications = Notifications::with_sink(Box::new(recorder.clone()));
    (temp, ctx, app, recorder)
}

fn saved_board(temp: &tempfile::TempDir) -> RuntimeState {
    RuntimeState::load(&temp.path().join(".horizon/sessions/keep-running/runtime.yaml"))
        .expect("the saved board is readable")
        .expect("the hide saved the board")
}

/// Without the setting, the close of the window ends Horizon as it did: the frame
/// cancels the close of the native window and the exit of the panels runs.
#[test]
fn the_close_of_the_window_ends_horizon_when_the_setting_is_off() {
    let (_temp, ctx, mut app, recorder) = app_with(false);

    let sent = commands(&ctx, &mut app, close_request());

    assert!(
        sent.contains(&ViewportCommand::CancelClose),
        "the exit holds its window: {sent:?}"
    );
    assert!(
        !sent.contains(&ViewportCommand::Visible(false)),
        "the window is not hidden: {sent:?}"
    );
    assert!(app.shutdown_progress.is_some(), "the exit of the panels runs");
    assert!(!app.keep_running.hidden, "no window is hidden");
    assert!(recorder.lines().is_empty(), "nothing is said about a hide");
}

/// With the setting on, the close hides the window and ends nothing: the panels
/// run, and the board of the session is on disk at the hide.
#[test]
fn the_close_of_the_window_hides_it_and_keeps_horizon_running() {
    let (temp, ctx, mut app, _recorder) = app_with(true);
    let workspace = app.board.create_workspace("Kept");
    app.board.focus_workspace(workspace);

    let sent = commands(&ctx, &mut app, close_request());
    wait_for_session_saves(&app);

    assert!(
        sent.contains(&ViewportCommand::Visible(false)),
        "the frame hides the window: {sent:?}"
    );
    assert!(
        sent.contains(&ViewportCommand::CancelClose),
        "the native close does not end the program: {sent:?}"
    );
    assert!(app.shutdown_progress.is_none(), "no panel is stopped");
    assert!(app.keep_running.hidden, "the window is hidden");
    let saved = saved_board(&temp);
    assert!(
        saved.workspaces.iter().any(|saved| saved.name == "Kept"),
        "the hide saved the board as a close does"
    );
}

/// The notice of the first hide speaks once for the run, not for each hide.
#[test]
fn the_notice_of_a_hidden_window_speaks_once_for_the_run() {
    let (_temp, ctx, mut app, recorder) = app_with(true);

    let first = commands(&ctx, &mut app, close_request());
    let second = commands(&ctx, &mut app, close_request());

    assert!(first.contains(&ViewportCommand::Visible(false)), "{first:?}");
    assert!(second.contains(&ViewportCommand::Visible(false)), "{second:?}");
    let lines = recorder.lines();
    assert_eq!(lines.len(), 1, "one notice for two hides: {lines:?}");
    assert!(lines[0].contains("Horizon keeps running"), "{}", lines[0]);
    assert!(lines[0].contains("Start Horizon again to show"), "{}", lines[0]);
    assert!(lines[0].contains("Quit Horizon"), "{}", lines[0]);
}

/// The window is shown again and raised for a second start of this computer.
#[test]
fn a_second_start_shows_the_window_again_and_raises_it() {
    let (_temp, ctx, mut app, _recorder) = app_with(true);
    let _ = commands(&ctx, &mut app, close_request());
    assert!(app.keep_running.hidden);

    app.show_main_window(&ctx);
    let sent = commands(&ctx, &mut app, RawInput::default());

    assert!(sent.contains(&ViewportCommand::Visible(true)), "{sent:?}");
    assert!(sent.contains(&ViewportCommand::Focus), "{sent:?}");
    assert!(!app.keep_running.hidden, "the window is shown again");
}

/// The menu ends Horizon with the setting on, and with it off.
#[test]
fn quit_horizon_ends_the_program_whatever_the_setting_says() {
    for keep_running in [false, true] {
        let (_temp, ctx, mut app, recorder) = app_with(keep_running);

        app.perform_toolbar_action(&ctx, ToolbarAction::Quit);

        assert!(
            app.shutdown_progress.is_some(),
            "Quit ends the panels with the setting {keep_running}"
        );
        assert!(!app.keep_running.hidden, "Quit hides nothing with the setting {keep_running}");
        assert!(recorder.lines().is_empty(), "Quit says nothing about a hide");
    }
}
