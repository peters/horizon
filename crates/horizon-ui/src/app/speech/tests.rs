//! The hotkey, the routing of a take and the state mark, in one window.

use std::time::{Duration, Instant};

use egui::{Context, Event, Key, Modifiers};
use horizon_core::{Config, PanelId, PanelOptions, StartupDecision, TranscriberKind, WorkspaceId};

use super::{NO_TERMINAL, SETTINGS_TRY, SpeechUi, clock_text};
use crate::app::test_support::{test_app, test_app_with_startup};
use crate::test_egui::DiscardTextures;

fn key(key: Key, pressed: bool) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

/// One frame with `events`, in which the window handles its speech hotkey.
fn frame_with_keys(ctx: &Context, app: &mut crate::app::HorizonApp, events: Vec<Event>) -> Vec<Event> {
    frame_with_keys_in(ctx, app, super::SpeechWindow::Root, events)
}

/// One frame with `events`, in which `window` handles its speech hotkey.
fn frame_with_keys_in(
    ctx: &Context,
    app: &mut crate::app::HorizonApp,
    window: super::SpeechWindow,
    events: Vec<Event>,
) -> Vec<Event> {
    let mut input = egui::RawInput {
        events,
        time: Some(0.0),
        focused: true,
        ..Default::default()
    };
    input.events.insert(0, Event::WindowFocused(true));
    input.viewports.entry(egui::ViewportId::ROOT).or_default().focused = Some(true);
    let mut remaining = Vec::new();
    let _ = ctx
        .run_ui(input, |_ui| {
            app.handle_speech_key(ctx, window);
            remaining = ctx.input(|input| input.events.clone());
        })
        .discard_textures();
    remaining
}

/// A settings document where speech input is on, with a recorder of this computer
/// and one command.
fn speech_on() -> Config {
    let mut config = Config::default();
    config.speech.recorder = "rec --rate 16000 {file}".to_owned();
    config.speech.transcriber.kind = TranscriberKind::Command;
    config.speech.transcriber.command = "transcriber {file}".to_owned();
    config
}

#[test]
fn a_hotkey_of_the_start_works_before_the_first_save() {
    // The window of a test is built from its settings, as the start of a person is,
    // so a hotkey works before any save of the window.
    let (_temp, ctx, mut app) = crate::app::test_support::test_app_with_config_and_startup(
        &speech_on(),
        StartupDecision::Ephemeral {
            runtime_state: Box::new(horizon_core::RuntimeState::default()),
        },
    );
    let remaining = frame_with_keys(&ctx, &mut app, vec![key(Key::F9, true)]);
    assert!(app.speech.input.is_none(), "no take runs without a terminal");
    let message = app.speech.mark(Instant::now()).expect("the sentence of the key");
    assert_eq!(message, NO_TERMINAL);
    assert!(
        !remaining
            .iter()
            .any(|event| matches!(event, Event::Key { key: Key::F9, .. }))
    );
}

#[test]
fn a_hotkey_with_no_terminal_starts_no_recording_and_says_so() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    app.apply_runtime_config(&speech_on());
    assert!(app.board.focused.is_none(), "a fresh test window has no panel");

    let remaining = frame_with_keys(&ctx, &mut app, vec![key(Key::F9, true)]);

    assert!(
        app.speech.input.is_none(),
        "no worker thread starts a take that has no panel"
    );
    assert_eq!(app.speech.target(), None);
    let message = app.speech.mark(Instant::now()).expect("the sentence of the key");
    assert_eq!(message, NO_TERMINAL);
    assert!(
        !remaining
            .iter()
            .any(|event| matches!(event, Event::Key { key: Key::F9, .. })),
        "the key of a take never reaches a program of a panel"
    );
}

#[test]
fn a_tap_whose_release_is_in_the_same_frame_starts_no_long_take() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    app.apply_runtime_config(&speech_on());

    // The press and the release of one frame: the take of that frame ends at once,
    // so a tap never records until its bound.
    let remaining = frame_with_keys(&ctx, &mut app, vec![key(Key::F9, true), key(Key::F9, false)]);

    let message = app.speech.mark(Instant::now()).expect("the sentence of the key");
    assert_eq!(message, NO_TERMINAL, "the tap of a key with no terminal starts nothing");
    assert!(app.speech.input.is_none());
    assert!(
        !remaining
            .iter()
            .any(|event| matches!(event, Event::Key { key: Key::F9, .. }))
    );
}

#[test]
fn a_hotkey_while_the_settings_name_no_transcriber_says_that_speech_input_is_off() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    app.apply_runtime_config(&Config::default());

    let remaining = frame_with_keys(&ctx, &mut app, vec![key(Key::F9, true)]);

    assert!(app.speech.input.is_none());
    let message = app.speech.mark(Instant::now()).expect("the sentence of the key");
    assert_eq!(message, super::NOT_SET);
    assert!(
        !remaining
            .iter()
            .any(|event| matches!(event, Event::Key { key: Key::F9, .. }))
    );
}

#[test]
fn the_pill_holds_no_line_of_a_take_and_its_sentence_of_an_error() {
    // The icon of the title bar states that a take records or is transcribed, so
    // the pill says nothing while it does.
    let mut speech = SpeechUi::new();
    let started = Instant::now();
    speech.target = Some(PanelId(3));
    speech.phase = super::Phase::Listening;
    speech.source = Some(super::TakeSource::Hotkey);
    speech.started = Some(started);

    assert_eq!(
        speech.mark(started + Duration::from_secs(3)),
        None,
        "a recording take draws no line of the pill"
    );
    assert_eq!(speech.mic_state(PanelId(3)), super::MicState::Recording);
    assert_eq!(speech.take_started(), Some(started));

    speech.phase = super::Phase::Writing;
    speech.started = None;
    assert_eq!(speech.mark(started), None, "a transcribed take draws no line either");
    assert_eq!(speech.mic_state(PanelId(3)), super::MicState::Busy);

    // The sentence of a failed take is the line that the pill still draws.
    speech.phase = super::Phase::Idle;
    speech.fail("pw-record is not installed on this computer");
    assert_eq!(
        speech.mark(started),
        Some("pw-record is not installed on this computer".to_owned())
    );
}

#[test]
fn the_sentence_of_a_failed_take_shows_for_six_seconds_and_then_goes() {
    let mut speech = SpeechUi::new();
    let now = Instant::now();
    speech.fail("pw-record is not installed on this computer");
    let message = speech.mark(now).expect("the sentence of the failure");
    assert_eq!(message, "pw-record is not installed on this computer");
    assert!(speech.mark(now + Duration::from_secs(5)).is_some());
    assert_eq!(speech.mark(now + Duration::from_secs(7)), None);
}

#[test]
fn a_text_of_a_take_goes_to_the_panel_of_the_take_and_no_other() {
    let mut speech = SpeechUi::new();
    let target = PanelId(11);
    speech.phase = super::Phase::Listening;
    speech.target = Some(target);
    let event = |kind| horizon_core::speech::SpeechEvent { target, kind };

    let routed = speech.apply(event(horizon_core::speech::SpeechEventKind::Text("hei".to_owned())));
    assert_eq!(routed, Some((target, "hei".to_owned())));
    assert_eq!(speech.target(), Some(target), "the panel keeps the mark of its take");
    assert!(speech.try_answer().is_none());

    // A take that ends without a text inserts nothing.
    speech.phase = super::Phase::Listening;
    assert_eq!(
        speech.apply(event(horizon_core::speech::SpeechEventKind::Dropped)),
        None
    );
    assert!(speech.try_answer().is_none());
}

#[test]
fn the_text_of_the_try_button_stays_in_the_settings_tab() {
    let mut speech = SpeechUi::new();
    speech.phase = super::Phase::Listening;
    speech.target = Some(SETTINGS_TRY);

    let routed = speech.apply(horizon_core::speech::SpeechEvent {
        target: SETTINGS_TRY,
        kind: horizon_core::speech::SpeechEventKind::Text("hei".to_owned()),
    });
    assert_eq!(routed, None, "no panel receives the text of the Try button");
    assert_eq!(speech.try_answer(), Some(&Ok("hei".to_owned())));

    // The sentence of a failure is the sentence of the take, and it names the host
    // of the endpoint of the settings.
    let sentence = horizon_core::speech::service::ServiceError::Unreachable {
        host: "127.0.0.1:40913".to_owned(),
    }
    .to_string();
    assert!(
        sentence.starts_with("Could not connect to 127.0.0.1:40913: "),
        "{sentence}"
    );
    speech.phase = super::Phase::Listening;
    let routed = speech.apply(horizon_core::speech::SpeechEvent {
        target: SETTINGS_TRY,
        kind: horizon_core::speech::SpeechEventKind::Failed(sentence.clone()),
    });
    assert_eq!(routed, None);
    assert_eq!(speech.try_answer(), Some(&Err(sentence)));
}

#[test]
fn the_text_of_the_mark_is_the_minutes_and_seconds_of_a_take() {
    assert_eq!(clock_text(Duration::ZERO), "0:00");
    assert_eq!(clock_text(Duration::from_millis(999)), "0:00");
    assert_eq!(clock_text(Duration::from_secs(9)), "0:09");
    assert_eq!(clock_text(Duration::from_secs(62)), "1:02");
    assert_eq!(clock_text(Duration::from_secs(3600)), "60:00");
}

#[test]
fn the_mark_stays_inside_the_panel_of_its_take() {
    use super::pill;
    let panel = egui::Rect::from_min_size(egui::Pos2::new(40.0, 60.0), egui::vec2(500.0, 400.0));
    let rect = pill::mark_rect(panel, egui::vec2(120.0, 14.0));
    assert!(panel.contains_rect(rect), "the mark is inside its panel: {rect:?}");
    assert!(rect.min.x > panel.min.x, "the mark keeps off the left edge");
    assert!(rect.max.y < panel.max.y, "the mark keeps off the bottom edge");
    assert!(
        rect.max.y > panel.center().y,
        "the mark sits at the bottom of its panel"
    );

    // A wider text widens the pill and keeps it inside the panel it was given.
    let wide = pill::mark_rect(panel, egui::vec2(400.0, 14.0));
    assert!(wide.width() > rect.width());
    assert!(panel.contains_rect(wide));
}

#[test]
fn a_window_of_a_session_with_no_panel_of_a_take_draws_no_mark() {
    let (_temp, _ctx, app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(horizon_core::RuntimeState::default()),
    });
    assert_eq!(app.speech.target(), None);
    assert!(app.speech.mark(Instant::now()).is_none());
}

/// A settings document where speech input is on, with a recorder that cannot start
/// on any computer: a test starts no program and opens no microphone.
fn speech_on_with_a_recorder_that_cannot_start() -> Config {
    let mut config = speech_on();
    config.speech.recorder = "/nonexistent/horizon-test-recorder {file}".to_owned();
    config
}

/// The board of two workspaces: one in the main window, one detached, each with one
/// terminal panel, and the focus on the panel of the detached workspace.
fn app_with_a_detached_workspace() -> (tempfile::TempDir, Context, crate::app::HorizonApp, WorkspaceId, PanelId) {
    let (temp, mut app) = test_app();
    app.apply_runtime_config(&speech_on_with_a_recorder_that_cannot_start());
    let main = app.board.create_workspace("main window");
    let side = app.board.create_workspace("side window");
    let options = || PanelOptions {
        kind: horizon_core::PanelKind::Shell,
        rows: 6,
        cols: 40,
        ..PanelOptions::default()
    };
    app.board
        .create_panel(options(), main)
        .expect("a panel of the main window");
    let side_panel = app
        .board
        .create_panel(options(), side)
        .expect("a panel of the side window");
    app.detach_workspace(side);
    assert!(app.workspace_is_detached(side), "the side workspace is detached");
    app.board.focus(side_panel);
    (temp, Context::default(), app, side, side_panel)
}

#[test]
fn the_hotkey_of_the_main_window_never_takes_a_panel_of_a_detached_workspace() {
    // The focus is on a panel that the main window does not draw. A take for it
    // would record with nothing on screen in this window and insert the text into a
    // panel that the person cannot see.
    let (_temp, ctx, mut app, _side, _side_panel) = app_with_a_detached_workspace();

    let remaining = frame_with_keys(&ctx, &mut app, vec![key(Key::F9, true)]);

    assert_eq!(app.speech.target(), None, "the main window has no panel of a take");
    assert!(app.speech.input.is_none(), "no take runs for a panel of another window");
    let message = app.speech.mark(Instant::now()).expect("the sentence of the key");
    assert_eq!(message, NO_TERMINAL);
    assert!(
        !remaining
            .iter()
            .any(|event| matches!(event, Event::Key { key: Key::F9, .. })),
        "the key of a take never reaches a program of a panel"
    );
}

#[test]
fn the_hotkey_of_a_detached_window_takes_the_panel_that_has_the_focus_there() {
    // The same board, and the hotkey of the window that draws the workspace: the
    // take belongs to the panel of that window.
    let (_temp, ctx, mut app, side, side_panel) = app_with_a_detached_workspace();

    let remaining = frame_with_keys_in(
        &ctx,
        &mut app,
        super::SpeechWindow::Detached(side),
        vec![key(Key::F9, true)],
    );

    assert_eq!(
        app.speech.target(),
        Some(side_panel),
        "the take belongs to the panel of the detached window"
    );
    assert!(
        app.speech.is_listening(),
        "the press of the detached window started one take"
    );
    assert!(
        !remaining
            .iter()
            .any(|event| matches!(event, Event::Key { key: Key::F9, .. })),
        "the key of a take never reaches a program of a panel"
    );
    app.speech.finish();
}

#[test]
fn a_take_that_a_slow_link_did_not_send_offers_one_retry() {
    let mut speech = SpeechUi::new();
    let target = PanelId(11);
    speech.phase = super::Phase::Listening;
    speech.target = Some(target);
    let sentence = horizon_core::speech::service::ServiceError::UploadTooSlow {
        host: "speech.example".to_owned(),
        seconds: 40,
    }
    .to_string();
    let routed = speech.apply(horizon_core::speech::SpeechEvent {
        target,
        kind: horizon_core::speech::SpeechEventKind::Kept(sentence.clone()),
    });
    assert_eq!(routed, None, "the kept take inserts nothing yet");
    let kept = speech.kept_take().expect("the take waits for one retry");
    assert!(
        kept.starts_with("The take is kept and can be sent again."),
        "the tab says that the take stays: {kept}"
    );
    let shortcut = kept
        .split(" can be sent again. ")
        .nth(1)
        .expect("the sentence names the shortcut");
    assert!(
        shortcut.contains("Shift+R") && (shortcut.contains("Ctrl+") || shortcut.contains("Cmd+")),
        "the sentence names the shortcut of a retry: {shortcut}"
    );
    let (message, until) = speech.error.clone().expect("a sentence of the mark");
    assert!(message.contains("The take is kept"), "{message}");
    let now = std::time::Instant::now();
    assert!(
        until > now + Duration::from_secs(20),
        "the offer of a retry stays longer than another failure"
    );

    // A take that the service answered with an error keeps nothing.
    speech.phase = super::Phase::Listening;
    speech.apply(horizon_core::speech::SpeechEvent {
        target,
        kind: horizon_core::speech::SpeechEventKind::Failed("The take held no text".to_owned()),
    });
    assert_eq!(speech.kept_take(), None, "nothing waits for a retry");
}

/// One press of a key that the person already holds: a repeat of that key, which
/// the window system sends again and again until the key is released.
fn repeat_key(key: Key) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: true,
        modifiers: Modifiers::NONE,
    }
}

/// One frame with `events`, in which the window handles its speech hotkey and
/// applies the answers of the worker of the take, as a frame of the window does.
fn take_frame(ctx: &Context, app: &mut crate::app::HorizonApp, events: Vec<Event>) {
    let mut input = egui::RawInput {
        events,
        time: Some(0.0),
        focused: true,
        ..Default::default()
    };
    input.events.insert(0, Event::WindowFocused(true));
    input.viewports.entry(egui::ViewportId::ROOT).or_default().focused = Some(true);
    let _ = ctx
        .run_ui(input, |_ui| {
            app.handle_speech_key(ctx, super::SpeechWindow::Root);
            app.poll_speech(ctx);
        })
        .discard_textures();
}

/// How many times a stand-in recorder of a test started.
fn starts_of(file: &std::path::Path) -> usize {
    std::fs::read_to_string(file).map_or(0, |text| text.lines().count())
}

/// The sentence of the mark, once the window has applied it, and at most five
/// seconds of frames.
fn wait_for_sentence(ctx: &Context, app: &mut crate::app::HorizonApp) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        take_frame(ctx, app, Vec::new());
        if let Some(text) = app.speech.mark(Instant::now()) {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "the window shows no sentence of the take, and its mark is {:?}",
            app.speech.mark(Instant::now())
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The window of one terminal panel, with the settings of speech input on.
fn window_with_a_terminal(config: &Config) -> (tempfile::TempDir, Context, crate::app::HorizonApp, PanelId) {
    let (temp, mut app) = test_app();
    app.apply_runtime_config(config);
    let workspace = app.board.create_workspace("main window");
    let panel = app
        .board
        .create_panel(
            PanelOptions {
                kind: horizon_core::PanelKind::Shell,
                rows: 6,
                cols: 40,
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("a panel of the window");
    app.board.focus(panel);
    (temp, Context::default(), app, panel)
}

#[test]
fn a_held_hotkey_starts_one_take_and_shows_a_recorder_that_dies_at_once() {
    // The recorder of this test cannot record: it answers one line of an error and
    // ends, which is the recorder of a computer whose sound server is away.
    let scripts = tempfile::tempdir().expect("a folder of the stand-ins");
    let starts = scripts.path().join("starts.txt");
    let program = horizon_core::stand_in_shell_in(
        scripts.path(),
        "recorder",
        &format!(
            "printf 'start\\n' >> '{}'\nprintf 'the sound server is away\\n' >&2\nexit 1\n",
            starts.display()
        ),
    )
    .expect("a recorder of a test");
    let mut config = speech_on();
    config.speech.recorder = format!("{program} {{file}}");
    let (_temp, ctx, mut app, panel) = window_with_a_terminal(&config);

    take_frame(&ctx, &mut app, vec![key(Key::F9, true)]);
    assert_eq!(app.speech.target(), Some(panel), "the take belongs to the panel");
    assert!(app.speech.is_listening(), "the press starts one take");

    // The key stays down and the recorder ends by itself: the person reads why.
    let sentence = wait_for_sentence(&ctx, &mut app);
    assert!(sentence.contains("ended early"), "{sentence}");
    assert!(sentence.contains("the sound server is away"), "{sentence}");

    // Every auto-repeat of the held key is still the same press, so no second take
    // starts a second recorder behind the mark.
    for _ in 0..6 {
        take_frame(&ctx, &mut app, vec![repeat_key(Key::F9)]);
        std::thread::sleep(Duration::from_millis(20));
    }
    take_frame(&ctx, &mut app, Vec::new());
    let text = app.speech.mark(Instant::now()).expect("the mark of the take");
    assert!(!text.is_empty(), "the mark keeps the failure: {text}");
    assert_eq!(starts_of(&starts), 1, "one held key starts one recorder");

    // The release of a key whose take already ended adds nothing.
    take_frame(&ctx, &mut app, vec![key(Key::F9, false)]);
    take_frame(&ctx, &mut app, Vec::new());
    assert_eq!(starts_of(&starts), 1, "the release starts no recorder");
    assert!(app.speech.mark(Instant::now()).is_some(), "the mark keeps the failure");
}

#[test]
fn the_icon_of_the_panel_records_while_the_recorder_lives() {
    let scripts = tempfile::tempdir().expect("a folder of the stand-ins");
    let program = horizon_core::stand_in_shell_in(
        scripts.path(),
        "recorder",
        "trap 'exit 0' INT TERM\nwhile :; do sleep 0.02; done",
    )
    .expect("a recorder of a test");
    let mut config = speech_on();
    config.speech.recorder = format!("{program} {{file}}");
    let (_temp, ctx, mut app, panel) = window_with_a_terminal(&config);

    take_frame(&ctx, &mut app, vec![key(Key::F9, true)]);
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(1250) {
        take_frame(&ctx, &mut app, Vec::new());
        assert_eq!(
            app.speech.mic_state(panel),
            super::MicState::Recording,
            "the icon of the panel is the recording of its take"
        );
        assert_eq!(
            app.speech.mic_state(PanelId(999)),
            super::MicState::Idle,
            "the icon of another panel is idle"
        );
        assert_eq!(
            app.speech.mark(Instant::now()),
            None,
            "the pill counts nothing while the recorder lives"
        );
        std::thread::sleep(Duration::from_millis(40));
    }
    let take_started = app.speech.take_started().expect("the take of the panel records");
    assert_eq!(
        super::clock_text(take_started.elapsed()),
        "0:01",
        "the clock of the icon moves while the recorder lives"
    );

    // The release ends the take, and the recorder of the test ends with it.
    take_frame(&ctx, &mut app, vec![key(Key::F9, false)]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.speech.is_listening() {
        take_frame(&ctx, &mut app, Vec::new());
        assert!(Instant::now() < deadline, "the take of the test never ended");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A window of one terminal panel with a stand-in recorder that records until its
/// take ends, and the file that counts the recorders that started.
fn window_with_a_staying_recorder() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    Context,
    crate::app::HorizonApp,
    PanelId,
    std::path::PathBuf,
) {
    let scripts = tempfile::tempdir().expect("a folder of the stand-ins");
    let starts = scripts.path().join("starts.txt");
    let program = horizon_core::stand_in_shell_in(
        scripts.path(),
        "recorder",
        &format!(
            "printf 'start\\n' >> '{}'\ntrap 'exit 0' INT TERM\nwhile :; do sleep 0.02; done",
            starts.display()
        ),
    )
    .expect("a recorder of a test");
    let mut config = speech_on();
    config.speech.recorder = format!("{program} {{file}}");
    let (temp, ctx, app, panel) = window_with_a_terminal(&config);
    (scripts, temp, ctx, app, panel, starts)
}

/// Wait until `done` holds, with frames of the window.
fn wait_until(ctx: &Context, app: &mut crate::app::HorizonApp, done: impl Fn(&crate::app::HorizonApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(app) {
        take_frame(ctx, app, Vec::new());
        assert!(
            Instant::now() < deadline,
            "the window never reached the state of the test"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_panel_shows_no_icon_while_speech_input_is_off() {
    let (_temp, mut app) = test_app();
    let mut config = speech_on();
    config.speech.transcriber.command = String::new();
    assert!(!config.speech.is_on(), "a settings file with no transcriber");
    app.apply_runtime_config(&config);
    let workspace = app.board.create_workspace("main window");
    let panel = app
        .board
        .create_panel(
            PanelOptions {
                kind: horizon_core::PanelKind::Shell,
                rows: 6,
                cols: 40,
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("a panel of the window");

    assert!(
        app.panel_mic(panel, super::SpeechWindow::Root, true).is_none(),
        "no panel wears the icon while speech input is off"
    );

    app.apply_runtime_config(&speech_on());
    let mic = app
        .panel_mic(panel, super::SpeechWindow::Root, true)
        .expect("the icon of a terminal while speech input is on");
    assert_eq!(mic.state, super::MicState::Idle);
    assert!(mic.enabled, "an idle icon of a window that waits can be pressed");
}

#[test]
fn only_a_panel_that_a_take_can_enter_wears_the_icon() {
    let (_temp, mut app) = test_app();
    app.apply_runtime_config(&speech_on_with_a_recorder_that_cannot_start());
    let main = app.board.create_workspace("main window");
    let side = app.board.create_workspace("side window");
    let panel = |kind| PanelOptions {
        kind,
        rows: 6,
        cols: 40,
        ..PanelOptions::default()
    };
    let terminal = app
        .board
        .create_panel(panel(horizon_core::PanelKind::Shell), main)
        .expect("a terminal of the main window");
    let editor = app
        .board
        .create_panel(panel(horizon_core::PanelKind::Editor), main)
        .expect("an editor of the main window");
    let detached = app
        .board
        .create_panel(panel(horizon_core::PanelKind::Shell), side)
        .expect("a terminal of the side window");
    app.detach_workspace(side);

    assert!(app.panel_mic(terminal, super::SpeechWindow::Root, true).is_some());
    assert!(
        app.panel_mic(editor, super::SpeechWindow::Root, true).is_none(),
        "a paste enters no editor panel, so the hotkey never takes it"
    );
    assert!(
        app.panel_mic(detached, super::SpeechWindow::Root, true).is_none(),
        "a panel that another window draws is no target of this window"
    );
    assert!(
        app.panel_mic(detached, super::SpeechWindow::Detached(side), true)
            .is_some(),
        "the window that draws the panel shows its icon"
    );
}

#[test]
fn a_click_on_the_icon_starts_and_stops_the_take_of_that_panel() {
    let (_temp, ctx, mut app, panel) = window_with_a_terminal(&speech_on_with_a_recorder_that_cannot_start());
    assert_eq!(app.speech.target(), None, "no take runs at the start");

    // A press on the icon of a panel starts the take of that panel, even though the
    // hotkey of the window would take no panel while the focus is elsewhere.
    app.click_speech_mic(&ctx, panel, super::SpeechWindow::Root);

    assert_eq!(
        app.speech.target(),
        Some(panel),
        "the take belongs to the panel of the icon"
    );
    assert!(app.speech.is_listening(), "the press starts one take");
    assert_eq!(app.speech.mic_state(panel), super::MicState::Recording);

    // The second press on the red icon ends the take, and the worker answers its end.
    app.click_speech_mic(&ctx, panel, super::SpeechWindow::Root);
    wait_until(&ctx, &mut app, |app| !app.speech.is_listening());
    assert_ne!(
        app.speech.mic_state(panel),
        super::MicState::Recording,
        "the icon of the panel is no recording after the second press"
    );
}

#[test]
fn a_take_that_the_hotkey_started_ends_with_a_click_on_its_icon() {
    let (_scripts, _temp, ctx, mut app, panel, starts) = window_with_a_staying_recorder();

    take_frame(&ctx, &mut app, vec![key(Key::F9, true)]);
    assert!(app.speech.is_listening(), "the key starts one take");
    wait_until(&ctx, &mut app, |_| starts_of(&starts) == 1);

    app.click_speech_mic(&ctx, panel, super::SpeechWindow::Root);
    wait_until(&ctx, &mut app, |app| !app.speech.is_listening());
    assert_eq!(starts_of(&starts), 1, "the press on the icon starts no recorder");
}

#[test]
fn a_take_that_the_icon_started_does_not_end_when_a_key_comes_up() {
    let (_scripts, _temp, ctx, mut app, panel, starts) = window_with_a_staying_recorder();

    app.click_speech_mic(&ctx, panel, super::SpeechWindow::Root);
    wait_until(&ctx, &mut app, |_| starts_of(&starts) == 1);

    // The release, and then the press, of the hotkey belong to no take of this key.
    take_frame(&ctx, &mut app, vec![key(Key::F9, false)]);
    assert!(
        app.speech.is_listening(),
        "the release of the hotkey ends only the take that the hotkey started"
    );
    take_frame(&ctx, &mut app, vec![key(Key::F9, true)]);
    assert_eq!(starts_of(&starts), 1, "the press starts no second recorder");
    assert_eq!(app.speech.target(), Some(panel));

    app.speech.cancel();
}

#[test]
fn the_icon_of_another_panel_is_disabled_while_a_take_runs() {
    let (_scripts, _temp, ctx, mut app, first, starts) = window_with_a_staying_recorder();
    let workspace = app.board.panel(first).expect("the first panel").workspace_id;
    let second = app
        .board
        .create_panel(
            PanelOptions {
                kind: horizon_core::PanelKind::Shell,
                rows: 6,
                cols: 40,
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("a second terminal of the window");

    let before = app
        .panel_mic(second, super::SpeechWindow::Root, true)
        .expect("the icon of the second panel");
    assert!(before.enabled, "an idle icon of an idle window can be pressed");
    assert!(!before.active);

    app.click_speech_mic(&ctx, first, super::SpeechWindow::Root);
    wait_until(&ctx, &mut app, |_| starts_of(&starts) == 1);

    let during = app
        .panel_mic(second, super::SpeechWindow::Root, true)
        .expect("the icon of the second panel");
    assert_eq!(during.state, super::MicState::Idle, "no take records for it");
    assert!(during.active, "the window holds the take of the first panel");
    assert!(!during.enabled, "the icon of the other panel takes no press");

    // A press on it starts nothing, and the take of the first panel keeps running.
    app.click_speech_mic(&ctx, second, super::SpeechWindow::Root);
    assert_eq!(app.speech.target(), Some(first));
    assert!(app.speech.is_listening());
    assert_eq!(starts_of(&starts), 1, "no second recorder starts");

    app.speech.cancel();
}

#[test]
fn the_icon_of_a_take_that_is_transcribed_takes_no_press() {
    let (_temp, ctx, mut app, panel) = window_with_a_terminal(&speech_on_with_a_recorder_that_cannot_start());
    app.speech.phase = super::Phase::Writing;
    app.speech.target = Some(panel);

    let mic = app
        .panel_mic(panel, super::SpeechWindow::Root, true)
        .expect("the icon of the panel");
    assert_eq!(mic.state, super::MicState::Busy);
    assert!(!mic.enabled, "a transcribed take takes no press");

    app.click_speech_mic(&ctx, panel, super::SpeechWindow::Root);
    assert_eq!(
        app.speech.phase,
        super::Phase::Writing,
        "the press changes no transcribed take"
    );
    assert!(app.speech.input.is_none(), "no take started");
}

/// The repaint requests that one pass of `poll_speech` made.
///
/// egui keeps the requests of a pass in the pass after it, so the window of the
/// measurement is the pass of the state and the empty pass that follows it.
fn speech_repaint_requests(ctx: &Context, app: &mut crate::app::HorizonApp) -> usize {
    let _ = ctx
        .run_ui(egui::RawInput::default(), |_ui| app.poll_speech(ctx))
        .discard_textures();
    let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {}).discard_textures();
    ctx.repaint_causes()
        .iter()
        .filter(|cause| cause.file.ends_with("app/speech.rs"))
        .count()
}

#[test]
fn an_idle_icon_requests_no_frame_and_a_recording_one_does() {
    let (_temp, ctx, mut app, panel) = window_with_a_terminal(&speech_on());
    // The first passes of a context settle its fonts and its input, and they ask
    // for the frames of that work.
    for _ in 0..3 {
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {}).discard_textures();
    }

    assert_eq!(
        speech_repaint_requests(&ctx, &mut app),
        0,
        "a window whose every icon is idle asks for no frame of the pulse"
    );

    app.speech.phase = super::Phase::Listening;
    app.speech.source = Some(super::TakeSource::Icon);
    app.speech.target = Some(panel);
    app.speech.started = Some(Instant::now());
    assert!(
        speech_repaint_requests(&ctx, &mut app) > 0,
        "the pulse of an icon that records asks for the next frame"
    );

    app.speech.phase = super::Phase::Writing;
    assert!(
        speech_repaint_requests(&ctx, &mut app) > 0,
        "the pulse of an icon that is transcribed asks for the next frame"
    );

    app.speech.cancel();
    assert_eq!(
        speech_repaint_requests(&ctx, &mut app),
        0,
        "no icon pulses while no take runs"
    );
}
