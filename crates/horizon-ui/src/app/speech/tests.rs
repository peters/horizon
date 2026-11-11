//! The hotkey, the routing of a take and the state mark, in one window.

use std::time::{Duration, Instant};

use egui::{Context, Event, Key, Modifiers};
use horizon_core::{Config, PanelId, StartupDecision, TranscriberKind};

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
            app.handle_speech_key(ctx);
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
    let mut speech = SpeechUi::new(std::path::PathBuf::from("/home/.horizon"));
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
    let mut speech = SpeechUi::new(std::path::PathBuf::from("/home/.horizon"));
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
    let mut speech = SpeechUi::new(std::path::PathBuf::from("/home/.horizon"));
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
    let mut speech = SpeechUi::new(std::path::PathBuf::from("/home/.horizon"));
    speech.phase = super::Phase::Listening;
    speech.target = Some(SETTINGS_TRY);

    let routed = speech.apply(horizon_core::speech::SpeechEvent {
        target: SETTINGS_TRY,
        kind: horizon_core::speech::SpeechEventKind::Text("hei".to_owned()),
    });
    assert_eq!(routed, None, "no panel receives the text of the Try button");
    assert_eq!(speech.try_answer(), Some(&Ok("hei".to_owned())));

    speech.phase = super::Phase::Listening;
    let routed = speech.apply(horizon_core::speech::SpeechEvent {
        target: SETTINGS_TRY,
        kind: horizon_core::speech::SpeechEventKind::Failed("no answer".to_owned()),
    });
    assert_eq!(routed, None);
    assert_eq!(speech.try_answer(), Some(&Err("no answer".to_owned())));
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
