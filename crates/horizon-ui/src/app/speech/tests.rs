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
    let (message, _) = app.speech.mark(Instant::now()).expect("the sentence of the key");
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
    let (message, tone) = app.speech.mark(Instant::now()).expect("the sentence of the key");
    assert_eq!(message, NO_TERMINAL);
    assert_eq!(tone, crate::app::speech::MarkTone::Error);
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

    let (message, _) = app.speech.mark(Instant::now()).expect("the sentence of the key");
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
    let (message, _) = app.speech.mark(Instant::now()).expect("the sentence of the key");
    assert_eq!(message, super::NOT_SET);
    assert!(
        !remaining
            .iter()
            .any(|event| matches!(event, Event::Key { key: Key::F9, .. }))
    );
}

#[test]
fn the_mark_counts_the_seconds_of_a_take_and_names_a_transcription() {
    let mut speech = SpeechUi::new();
    let started = Instant::now();
    speech.target = Some(PanelId(3));
    speech.phase = super::Phase::Listening;
    speech.started = Some(started);

    let (text, tone) = speech
        .mark(started + Duration::from_secs(3))
        .expect("the mark of a take");
    assert_eq!(text, "Listening 0:03");
    assert_eq!(tone, crate::app::speech::MarkTone::Listening);

    let (text, tone) = speech
        .mark(started + Duration::from_secs(63))
        .expect("the mark of a long take");
    assert_eq!(text, "Listening 1:03");
    assert_eq!(tone, crate::app::speech::MarkTone::Listening);

    speech.phase = super::Phase::Writing;
    speech.started = None;
    let (text, tone) = speech.mark(started).expect("the mark of a transcription");
    assert_eq!(text, "Writing…");
    assert_eq!(tone, crate::app::speech::MarkTone::Writing);
}

#[test]
fn the_sentence_of_a_failed_take_shows_for_six_seconds_and_then_goes() {
    let mut speech = SpeechUi::new();
    let now = Instant::now();
    speech.fail("pw-record is not installed on this computer");
    let (message, tone) = speech.mark(now).expect("the sentence of the failure");
    assert_eq!(message, "pw-record is not installed on this computer");
    assert_eq!(tone, crate::app::speech::MarkTone::Error);
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
    let (message, _) = app.speech.mark(Instant::now()).expect("the sentence of the key");
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
        if let Some((text, super::MarkTone::Error)) = app.speech.mark(Instant::now()) {
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
    let (text, tone) = app.speech.mark(Instant::now()).expect("the mark of the take");
    assert_eq!(tone, super::MarkTone::Error, "the mark keeps the failure: {text}");
    assert_eq!(starts_of(&starts), 1, "one held key starts one recorder");

    // The release of a key whose take already ended adds nothing.
    take_frame(&ctx, &mut app, vec![key(Key::F9, false)]);
    take_frame(&ctx, &mut app, Vec::new());
    assert_eq!(starts_of(&starts), 1, "the release starts no recorder");
    let (_, tone) = app.speech.mark(Instant::now()).expect("the mark of the take");
    assert_eq!(tone, super::MarkTone::Error);
}

#[test]
fn the_mark_counts_the_seconds_while_the_recorder_lives() {
    let scripts = tempfile::tempdir().expect("a folder of the stand-ins");
    let program = horizon_core::stand_in_shell_in(
        scripts.path(),
        "recorder",
        "trap 'exit 0' INT TERM\nwhile :; do sleep 0.02; done",
    )
    .expect("a recorder of a test");
    let mut config = speech_on();
    config.speech.recorder = format!("{program} {{file}}");
    let (_temp, ctx, mut app, _panel) = window_with_a_terminal(&config);

    take_frame(&ctx, &mut app, vec![key(Key::F9, true)]);
    let started = Instant::now();
    let mut texts: Vec<String> = Vec::new();
    while started.elapsed() < Duration::from_millis(1250) {
        take_frame(&ctx, &mut app, Vec::new());
        if let Some((text, super::MarkTone::Listening)) = app.speech.mark(Instant::now())
            && texts.last() != Some(&text)
        {
            texts.push(text);
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    assert_eq!(texts.first().map(String::as_str), Some("Listening 0:00"), "{texts:?}");
    assert_eq!(
        texts.last().map(String::as_str),
        Some("Listening 0:01"),
        "the counter of the mark moves while the recorder lives: {texts:?}"
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
