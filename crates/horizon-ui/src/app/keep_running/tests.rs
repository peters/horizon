//! The close of the main window with each of the three choices, the ask that
//! counts what runs, the notice of the first hide, and `Quit Horizon`.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Context, Event, Key, Modifiers, RawInput, ViewportCommand, ViewportId};
use horizon_core::notify::{Notice, Sink};
use horizon_core::{Config, PanelKind, PanelOptions, RuntimeState, StartupDecision, WindowCloseSetting};

use crate::app::HorizonApp;
use crate::app::keep_running::CloseAnswer;
use crate::app::notify::Notifications;
use crate::app::root_chrome::ToolbarAction;
use crate::app::test_support::{
    raw_input, run_app_frame_with_input, test_app_with_config_and_startup, wait_for_session_saves,
};
use crate::theme;

mod no_lease;

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

/// The options of a panel of this computer that runs a program of its own. The
/// program ends by itself after a few seconds, so a test that fails leaves no
/// process behind.
fn running(mut options: PanelOptions) -> PanelOptions {
    options.launch_wrapper = vec!["/bin/sh".to_owned(), "-c".to_owned(), "sleep 10".to_owned()];
    options
}

/// The options of a terminal of a workspace that is placed on a machine.
fn on_a_machine(session: &str) -> PanelOptions {
    running(PanelOptions {
        kind: PanelKind::Ssh,
        machine: Some("build-1".to_owned()),
        machine_session: Some(session.to_owned()),
        ..PanelOptions::default()
    })
}

/// The input of a frame in which the desktop asks the main window to close.
fn close_request() -> RawInput {
    let mut input = raw_input([1400.0, 900.0], None);
    input
        .viewports
        .entry(ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    input
}

/// One frame of the window with `input` as its input.
fn frame_with(ctx: &Context, app: &mut HorizonApp, input: RawInput) -> egui::FullOutput {
    run_app_frame_with_input(ctx, app, input)
}

/// Two frames of the window before the case draws, so the layout of a new window
/// is in the memory of the context: the first frame of a window draws no text.
fn warm_up(ctx: &Context, app: &mut HorizonApp) {
    for _ in 0..2 {
        let _ = frame_with(ctx, app, raw_input([1400.0, 900.0], None));
    }
}

/// One frame of the window in which the desktop asks it to close.
fn close_frame(ctx: &Context, app: &mut HorizonApp) -> egui::FullOutput {
    frame_with(ctx, app, close_request())
}

/// The ask that waits now, drawn: the frame that takes the request gives the ask
/// its place, and the frame after it draws the ask there.
fn ask_frame(ctx: &Context, app: &mut HorizonApp) -> egui::FullOutput {
    for _ in 0..2 {
        let _ = frame_with(ctx, app, raw_input([1400.0, 900.0], None));
    }
    frame_with(ctx, app, raw_input([1400.0, 900.0], None))
}

/// One frame of the window with `key` pressed and released.
fn press_key(ctx: &Context, app: &mut HorizonApp, key: Key) -> egui::FullOutput {
    let mut input = raw_input([1400.0, 900.0], None);
    input.events = [true, false]
        .map(|pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        })
        .to_vec();
    frame_with(ctx, app, input)
}

/// The commands that the frame of `input` sent to the main window.
fn commands(ctx: &Context, app: &mut HorizonApp, input: RawInput) -> Vec<ViewportCommand> {
    frame_with(ctx, app, input)
        .viewport_output
        .get(&ViewportId::ROOT)
        .map(|output| output.commands.clone())
        .unwrap_or_default()
}

/// The drawn texts of the frame.
fn texts(output: &egui::FullOutput) -> Vec<&str> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect()
}

/// Every shape of the frame, at any depth of a group of shapes.
fn shapes_of(output: &egui::FullOutput) -> Vec<&egui::Shape> {
    fn walk<'a>(shape: &'a egui::Shape, found: &mut Vec<&'a egui::Shape>) {
        found.push(shape);
        if let egui::Shape::Vec(inner) = shape {
            for shape in inner {
                walk(shape, found);
            }
        }
    }
    let mut found = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut found);
    }
    found
}

/// The fill of the action of the ask under `heading` that drew the text `label`:
/// the smallest filled rectangle of the frame that stands under that text and
/// around it. The toolbar of the window says some of the words of the ask too.
fn fill_of(output: &egui::FullOutput, label: &str, heading: &str) -> egui::Color32 {
    let centre = button(output, label, heading);
    let shapes = shapes_of(output);
    let text = shapes
        .iter()
        .filter_map(|shape| match shape {
            egui::Shape::Text(drawn) if drawn.galley.text() == label => {
                let rect = egui::Rect::from_min_size(drawn.pos, drawn.galley.size());
                Some((rect, rect.center().distance(centre)))
            }
            _ => None,
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or_else(
            || panic!("the ask under {heading} draws {label}: {:?}", texts(output)),
            |(rect, _)| rect,
        );
    shapes
        .iter()
        .filter_map(|shape| match shape {
            egui::Shape::Rect(rect) if rect.fill.a() != 0 && rect.rect.contains_rect(text) => Some(rect),
            _ => None,
        })
        .min_by(|a, b| a.rect.area().total_cmp(&b.rect.area()))
        .map_or_else(|| panic!("a fill stands under the text {label}"), |rect| rect.fill)
}

/// The frame of the ask once its area stopped fading in: a fill that is still
/// translucent belongs to a frame of that fade, and the area of a question draws
/// nothing before a frame of its own found its size.
fn settled_ask(ctx: &Context, app: &mut HorizonApp, heading: &str) -> egui::FullOutput {
    let mut output = ask_frame(ctx, app);
    for _ in 0..40 {
        if fill_of(&output, "Cancel", heading).a() == u8::MAX {
            return output;
        }
        output = frame_with(ctx, app, raw_input([1400.0, 900.0], None));
    }
    panic!("the ask under {heading} never reaches the colour of its actions");
}

/// The drawn texts of the frame with their centre.
fn texts_with_centre(output: &egui::FullOutput) -> Vec<(&str, egui::Pos2)> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some((text.galley.text(), text.pos + text.galley.size() * 0.5)),
            _ => None,
        })
        .collect()
}

/// The centre of the button `label` of the ask under its `heading`: the one
/// nearest to the heading, because the toolbar of the window says the word too.
fn button(output: &egui::FullOutput, label: &str, heading: &str) -> egui::Pos2 {
    let drawn = texts_with_centre(output);
    let anchor = drawn.iter().find(|(text, _)| *text == heading).map_or_else(
        || panic!("the ask draws {heading}: {:?}", texts(output)),
        |(_, centre)| *centre,
    );
    drawn
        .iter()
        .filter(|(text, _)| *text == label)
        .map(|(_, centre)| *centre)
        .min_by(|a, b| a.distance(anchor).total_cmp(&b.distance(anchor)))
        .unwrap_or_else(|| panic!("the ask offers {label}: {:?}", texts(output)))
}

/// One press of `pos`: the frame that takes it down and the frame that lets it up.
fn press(ctx: &Context, app: &mut HorizonApp, pos: egui::Pos2) -> egui::FullOutput {
    let mut output = egui::FullOutput::default();
    // The pointer moves to the button first, as a real pointer does.
    let mut hover = raw_input([1400.0, 900.0], None);
    hover.events = vec![Event::PointerMoved(pos)];
    let _ = frame_with(ctx, app, hover);
    for pressed in [true, false] {
        let mut input = raw_input([1400.0, 900.0], None);
        input.events = vec![Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }];
        output = frame_with(ctx, app, input);
    }
    output
}

/// The application of one test, with a sink that records instead of showing, and
/// a session whose board the run saves.
fn app_with(setting: WindowCloseSetting) -> (tempfile::TempDir, Context, HorizonApp, Recorder) {
    let mut config = Config::default();
    config.features.when_window_closes = setting;
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

/// With the choice `quit`, a close ends Horizon as it did before the two rounds,
/// and it asks nothing while programs run.
#[test]
fn the_choice_quit_ends_horizon_and_asks_nothing() {
    let (_temp, ctx, mut app, recorder) = app_with(WindowCloseSetting::Quit);
    let workspace = app.board.create_workspace("Quit");
    let _ = app
        .create_panel_with_options(running(PanelOptions::default()), workspace)
        .expect("a shell panel");

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
    assert!(app.keep_running.ask.is_none(), "no ask comes before the end");
    assert!(!app.keep_running.hidden, "no window is hidden");
    assert!(recorder.lines().is_empty(), "nothing is said about a hide");
}

/// With the choice `keep_running`, a close hides the window and ends nothing: the
/// panels run, and the board of the session is on disk at the hide.
#[test]
fn the_choice_keep_running_hides_the_window_and_keeps_horizon_running() {
    let (temp, ctx, mut app, _recorder) = app_with(WindowCloseSetting::KeepRunning);
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
    assert!(app.keep_running.ask.is_none(), "quiet never asks");
    let saved = saved_board(&temp);
    assert!(
        saved.workspaces.iter().any(|saved| saved.name == "Kept"),
        "the hide saved the board as a close does"
    );
}

/// The notice of the first hide speaks once for the run, not for each hide.
#[test]
fn the_notice_of_a_hidden_window_speaks_once_for_the_run() {
    let (_temp, ctx, mut app, recorder) = app_with(WindowCloseSetting::KeepRunning);

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
    let (_temp, ctx, mut app, _recorder) = app_with(WindowCloseSetting::KeepRunning);
    let _ = commands(&ctx, &mut app, close_request());
    assert!(app.keep_running.hidden);

    app.show_main_window(&ctx);
    let sent = commands(&ctx, &mut app, RawInput::default());

    assert!(sent.contains(&ViewportCommand::Visible(true)), "{sent:?}");
    assert!(sent.contains(&ViewportCommand::Focus), "{sent:?}");
    assert!(!app.keep_running.hidden, "the window is shown again");
}

/// A hidden window draws no frame, so the request of a second start reaches the
/// pass of a window that draws nothing: that pass shows and raises the window.
#[test]
fn the_pass_of_a_hidden_window_shows_it_for_a_second_start() {
    let (_temp, ctx, mut app, _recorder) = app_with(WindowCloseSetting::KeepRunning);
    let root = app.session_store.home().root().to_path_buf();
    let _ = commands(&ctx, &mut app, close_request());
    assert!(app.keep_running.hidden, "the window is hidden");

    let asking_root = root.clone();
    let caller = std::thread::spawn(move || horizon_core::folder_requests::ask_window(&asking_root));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !caller.is_finished() {
        app.folder_requests
            .waiting
            .store(true, std::sync::atomic::Ordering::Release);
        crate::app::undrawn::tests::run_logic_pass(&ctx, &mut app);
        assert!(std::time::Instant::now() < deadline, "the pass showed the window");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let answer = caller.join().expect("the caller").expect("a caller of the channel");

    assert_eq!(answer, Some(horizon_core::folder_requests::Answer::Window));
    assert!(!app.keep_running.hidden, "the pass showed the window");
    app.board.shutdown_terminal_panels();
}

/// With the choice `ask` and no program, a close ends Horizon at once and shows no
/// ask.
#[test]
fn the_choice_ask_ends_at_once_when_no_program_runs() {
    let (_temp, ctx, mut app, _recorder) = app_with(WindowCloseSetting::Ask);

    let sent = commands(&ctx, &mut app, close_request());

    assert!(sent.contains(&ViewportCommand::CancelClose), "{sent:?}");
    assert!(app.shutdown_progress.is_some(), "the exit of the panels runs");
    assert!(app.keep_running.ask.is_none(), "nothing runs, so nothing is asked");
}

/// The ask counts the programs of this computer and the panels of a machine, and
/// it speaks of both.
#[test]
fn the_ask_counts_the_programs_here_and_the_panels_of_a_machine() {
    let (_temp, ctx, mut app, _recorder) = app_with(WindowCloseSetting::Ask);
    let workspace = app.board.create_workspace("Ask");
    let _ = app
        .create_panel_with_options(running(PanelOptions::default()), workspace)
        .expect("a shell panel");
    let _ = app
        .create_panel_with_options(on_a_machine("hz-22222222-1"), workspace)
        .expect("a panel of a machine");
    let _ = app
        .create_panel_with_options(
            PanelOptions {
                kind: PanelKind::Editor,
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("an editor panel");
    warm_up(&ctx, &mut app);
    let _ = close_frame(&ctx, &mut app);

    let output = ask_frame(&ctx, &mut app);
    let drawn = texts(&output);

    assert!(drawn.contains(&"Close Horizon?"), "the heading: {drawn:?}");
    assert!(
        drawn.contains(&"1 program is running."),
        "only the local terminal counts: {drawn:?}"
    );
    assert!(
        drawn.contains(&"1 panel runs on a machine and keeps running there."),
        "the panels of a machine have their own line: {drawn:?}"
    );
    assert!(
        drawn.contains(&"Keep running in the background"),
        "the first action: {drawn:?}"
    );
    assert!(drawn.contains(&"End them and quit"), "the second action: {drawn:?}");
    assert!(drawn.contains(&"Cancel"), "the third action: {drawn:?}");
    assert!(drawn.contains(&"Remember this choice"), "the box: {drawn:?}");
    let running = app.keep_running.ask.as_ref().map(|ask| ask.running);
    assert_eq!(
        running.map(|running| (running.here, running.on_machines)),
        Some((1, 1)),
        "the ask counted when the close came"
    );
    assert!(
        output
            .viewport_output
            .get(&ViewportId::ROOT)
            .is_some_and(|output| !output.commands.contains(&ViewportCommand::Visible(false))),
        "the window stays while the ask shows"
    );

    // A second close request while the ask shows changes nothing.
    let sent = commands(&ctx, &mut app, close_request());
    assert!(
        sent.contains(&ViewportCommand::CancelClose),
        "the ask holds its window: {sent:?}"
    );
    assert!(app.keep_running.ask.is_some(), "the ask still waits for the person");
    assert!(app.shutdown_progress.is_none(), "a second close ends nothing");
    assert!(!app.keep_running.hidden, "a second close hides nothing");
    app.board.shutdown_terminal_panels();
}

/// Enter takes the first action: Horizon keeps running behind its hidden window.
#[test]
fn enter_keeps_running_in_the_background() {
    let (_temp, ctx, mut app, _recorder) = app_with(WindowCloseSetting::Ask);
    let workspace = app.board.create_workspace("Ask");
    let _ = app
        .create_panel_with_options(running(PanelOptions::default()), workspace)
        .expect("a shell panel");
    warm_up(&ctx, &mut app);
    let _ = close_frame(&ctx, &mut app);

    let output = press_key(&ctx, &mut app, Key::Enter);
    let sent = output
        .viewport_output
        .get(&ViewportId::ROOT)
        .map(|output| output.commands.clone())
        .unwrap_or_default();

    assert!(sent.contains(&ViewportCommand::Visible(false)), "{sent:?}");
    assert!(app.keep_running.hidden, "the window is hidden");
    assert!(app.keep_running.ask.is_none(), "the ask is answered");
    assert!(app.shutdown_progress.is_none(), "Enter ends no program");
    app.board.shutdown_terminal_panels();
}

/// `End them and quit` ends the programs and Horizon.
#[test]
fn end_them_and_quit_ends_horizon() {
    let (_temp, ctx, mut app, _recorder) = app_with(WindowCloseSetting::Ask);
    let workspace = app.board.create_workspace("Ask");
    let _ = app
        .create_panel_with_options(running(PanelOptions::default()), workspace)
        .expect("a shell panel");
    warm_up(&ctx, &mut app);
    let _ = close_frame(&ctx, &mut app);
    let output = ask_frame(&ctx, &mut app);

    let _ = press(&ctx, &mut app, button(&output, "End them and quit", "Close Horizon?"));

    assert!(app.shutdown_progress.is_some(), "the programs and Horizon end");
    assert!(!app.keep_running.hidden, "the window is not hidden");
    assert!(app.keep_running.ask.is_none(), "the ask is answered");
}

/// `Cancel` and the escape key keep everything where it is.
#[test]
fn cancel_and_escape_keep_everything() {
    for escape in [false, true] {
        let (_temp, ctx, mut app, _recorder) = app_with(WindowCloseSetting::Ask);
        let workspace = app.board.create_workspace("Ask");
        let _ = app
            .create_panel_with_options(running(PanelOptions::default()), workspace)
            .expect("a shell panel");
        warm_up(&ctx, &mut app);
        let _ = close_frame(&ctx, &mut app);
        assert!(app.keep_running.ask.is_some(), "the ask shows");
        let output = ask_frame(&ctx, &mut app);

        if escape {
            let _ = press_key(&ctx, &mut app, Key::Escape);
        } else {
            let _ = press(&ctx, &mut app, button(&output, "Cancel", "Close Horizon?"));
        }

        assert!(app.keep_running.ask.is_none(), "the ask is answered");
        assert!(app.shutdown_progress.is_none(), "Cancel ends no program");
        assert!(!app.keep_running.hidden, "Cancel hides nothing");
        app.board.shutdown_terminal_panels();
    }
}

/// The box `Remember this choice` writes the choice of the person, and the next
/// close takes it without a question.
#[test]
fn remember_writes_the_choice_and_the_next_close_does_not_ask() {
    for (answer, expected) in [
        (CloseAnswer::KeepRunning, WindowCloseSetting::KeepRunning),
        (CloseAnswer::Quit, WindowCloseSetting::Quit),
    ] {
        let (_temp, ctx, mut app, _recorder) = app_with(WindowCloseSetting::Ask);
        let workspace = app.board.create_workspace("Ask");
        let _ = app
            .create_panel_with_options(running(PanelOptions::default()), workspace)
            .expect("a shell panel");
        warm_up(&ctx, &mut app);
        let _ = close_frame(&ctx, &mut app);
        let ask = app.keep_running.ask.as_mut().expect("the ask shows");
        ask.remember = true;

        app.answer_close_ask(&ctx, answer);

        assert_eq!(
            app.template_config.features.when_window_closes, expected,
            "the box wrote the choice of the person"
        );
        assert!(app.keep_running.ask.is_none(), "the ask is answered");

        // The next close takes the remembered choice and asks nothing, which needs
        // a window that the answer of a `quit` did not end.
        app.shutdown_progress = None;
        app.keep_running.hidden = false;
        let _ = close_frame(&ctx, &mut app);
        assert!(app.keep_running.ask.is_none(), "the remembered choice asks nothing");
        match expected {
            WindowCloseSetting::Quit => assert!(app.shutdown_progress.is_some(), "quit ends Horizon"),
            _ => assert!(app.keep_running.hidden, "keep running hides the window"),
        }
        app.board.shutdown_terminal_panels();
    }
}

/// `Quit Horizon` in the menu asks for the programs that it ends, with the lines
/// of what lives on, and `Quit` ends Horizon.
#[test]
fn quit_horizon_asks_for_the_programs_and_ends_them() {
    let (_temp, ctx, mut app, _recorder) = app_with(WindowCloseSetting::KeepRunning);
    app.template_config.features.keep_agents_through_restart = true;
    let workspace = app.board.create_workspace("Quit");
    let _ = app
        .create_panel_with_options(running(PanelOptions::default()), workspace)
        .expect("a shell panel");
    let _ = app
        .create_panel_with_options(
            running(PanelOptions {
                kind: PanelKind::Codex,
                ..PanelOptions::default()
            }),
            workspace,
        )
        .expect("an agent panel");
    let _ = app
        .create_panel_with_options(on_a_machine("hz-33333333-1"), workspace)
        .expect("a panel of a machine");
    warm_up(&ctx, &mut app);

    app.perform_toolbar_action(&ctx, ToolbarAction::Quit);
    let output = ask_frame(&ctx, &mut app);
    let drawn = texts(&output);

    assert!(
        drawn.contains(&"End 2 programs and quit?"),
        "the question counts the programs of this computer: {drawn:?}"
    );
    assert!(
        drawn.contains(&"Agent programs that are kept through a restart come back at the next start."),
        "the agent programs come back: {drawn:?}"
    );
    assert!(
        drawn.contains(&"1 panel runs on a machine and keeps running there."),
        "the panel of the machine keeps running there: {drawn:?}"
    );
    assert!(app.shutdown_progress.is_none(), "the question ends nothing yet");

    let _ = press(&ctx, &mut app, button(&output, "Quit", "End 2 programs and quit?"));

    assert!(app.shutdown_progress.is_some(), "Quit ends Horizon");
}

/// `Quit Horizon` ends Horizon at once when no program runs.
#[test]
fn quit_horizon_ends_at_once_when_nothing_runs() {
    for setting in WindowCloseSetting::VALUES {
        let (_temp, ctx, mut app, _recorder) = app_with(setting);

        app.perform_toolbar_action(&ctx, ToolbarAction::Quit);

        assert!(
            app.shutdown_progress.is_some(),
            "Quit ends Horizon with the choice {setting:?}"
        );
        assert!(app.keep_running.ask.is_none(), "nothing runs, so nothing is asked");
        assert!(!app.keep_running.hidden, "Quit hides nothing");
    }
}

/// A workspace toolbar stands in the middle of the canvas, where the ask draws
/// its buttons: the press on a button that a toolbar covers belongs to the ask.
#[test]
fn the_ask_of_a_close_takes_a_press_under_a_workspace_toolbar() {
    let (_temp, ctx, mut app, _recorder) = app_with(WindowCloseSetting::Ask);
    let workspace = app.board.create_workspace("Ask");
    let _ = app
        .create_panel_with_options(running(PanelOptions::default()), workspace)
        .expect("a shell panel");
    warm_up(&ctx, &mut app);
    let _ = close_frame(&ctx, &mut app);
    let output = ask_frame(&ctx, &mut app);
    let button = button(&output, "End them and quit", "Close Horizon?");

    crate::app::test_support::toolbar_under(&ctx, &mut app, button);
    let _ = press(&ctx, &mut app, button);

    assert!(
        app.shutdown_progress.is_some(),
        "the press of End them and quit arrives where a workspace toolbar stands"
    );
    assert!(app.keep_running.ask.is_none(), "the ask is answered");
}

/// The action that Enter takes in the ask of a close is the main action of the
/// question, which the other questions draw with the accent: the fill of `Save`.
/// The action that ends the programs keeps the fill of `Delete`, and `Cancel`
/// keeps the plain one.
#[test]
fn the_first_action_of_the_close_ask_is_drawn_as_the_main_action() {
    let (_temp, ctx, mut app, _recorder) = app_with(WindowCloseSetting::Ask);
    let workspace = app.board.create_workspace("Ask");
    let _ = app
        .create_panel_with_options(running(PanelOptions::default()), workspace)
        .expect("a shell panel");
    warm_up(&ctx, &mut app);
    let _ = close_frame(&ctx, &mut app);
    let output = settled_ask(&ctx, &mut app, "Close Horizon?");

    let keep = fill_of(&output, "Keep running in the background", "Close Horizon?");
    let quit = fill_of(&output, "End them and quit", "Close Horizon?");
    let cancel = fill_of(&output, "Cancel", "Close Horizon?");
    assert_eq!(cancel, theme::PANEL_BG_ALT(), "Cancel keeps the plain fill");
    assert_eq!(
        keep,
        theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.28),
        "the action that Enter takes is drawn as Save is"
    );
    assert_eq!(
        quit,
        theme::blend(theme::PANEL_BG_ALT(), theme::PALETTE_RED(), 0.22),
        "the action that ends the programs is drawn as Delete is"
    );
    app.board.shutdown_terminal_panels();
}

/// `Quit Horizon` asks with two actions: the red one ends the programs, and
/// Enter takes neither of them.
#[test]
fn the_action_that_ends_the_programs_of_quit_horizon_is_red_and_enter_takes_it_not() {
    let (_temp, ctx, mut app, _recorder) = app_with(WindowCloseSetting::KeepRunning);
    let workspace = app.board.create_workspace("Quit");
    let _ = app
        .create_panel_with_options(running(PanelOptions::default()), workspace)
        .expect("a shell panel");
    warm_up(&ctx, &mut app);
    app.perform_toolbar_action(&ctx, ToolbarAction::Quit);
    let heading = "End 1 program and quit?";
    let _ = settled_ask(&ctx, &mut app, heading);

    // Enter answers nothing there: the ask waits for a press on one of its two
    // actions, and no program ends.
    let _ = press_key(&ctx, &mut app, Key::Enter);
    assert!(app.keep_running.ask.is_some(), "Enter leaves the ask of Quit Horizon");
    assert!(app.shutdown_progress.is_none(), "Enter ends no program there");

    let output = ask_frame(&ctx, &mut app);
    let quit = fill_of(&output, "Quit", heading);
    assert_eq!(
        fill_of(&output, "Cancel", heading),
        theme::PANEL_BG_ALT(),
        "Cancel keeps the plain fill"
    );
    assert_eq!(
        quit,
        theme::blend(theme::PANEL_BG_ALT(), theme::PALETTE_RED(), 0.22),
        "the action that ends the programs is drawn as Delete is"
    );
    app.board.shutdown_terminal_panels();
}
