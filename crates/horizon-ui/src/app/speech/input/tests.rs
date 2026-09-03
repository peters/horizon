use std::time::{Duration, Instant};

#[cfg(feature = "speech")]
use crate::test_egui::DiscardTextures;
use egui::Context;
use horizon_core::browser::BrowserPanelState;
use horizon_core::{Panel, PanelContent, PanelId, PanelKind, PanelOptions, WorkspaceId};

use super::{
    HoldHotkeyTransition, SPEECH_RELEASE_OWNERSHIP_TIMEOUT, SpeechActivity, TranscriptInjection,
    hold_hotkey_transition, inject_transcript, should_release_desktop_target, terminal_matches_focused_viewport,
};
#[cfg(feature = "speech")]
use super::{apply_global_hotkey_events, handle_profile_hotkeys, start_speech};
use crate::app::HeldSpeechBinding;
use crate::app::speech::SpeechSink;
use crate::app::test_support::test_app;

#[cfg(feature = "speech")]
struct PrepareHookGuard;

#[cfg(feature = "speech")]
impl PrepareHookGuard {
    fn install(hook: fn() -> Result<(), horizon_cursor::InjectError>) -> Self {
        crate::app::speech::desktop::set_test_prepare_hook(Some(hook));
        Self
    }
}

#[cfg(feature = "speech")]
impl Drop for PrepareHookGuard {
    fn drop(&mut self) {
        crate::app::speech::desktop::set_test_prepare_hook(None);
    }
}

/// Root focus may move directly into a detached Horizon viewport. Keep
/// ownership across a transient all-unfocused pass, but bound it in case
/// the destination viewport never receives the key-up.
#[test]
fn focus_loss_ownership_survives_handoff_and_expires_without_key_up() {
    let ctx = Context::default();
    let now = Instant::now();
    let (_temp, mut app) = test_app();
    let chord = horizon_core::ShortcutBinding::new(
        horizon_core::ShortcutModifiers::CTRL,
        horizon_core::ShortcutKey::Letter('K'),
    );
    // Pressed with no terminal focused: the filter holds the chord, but
    // the engine never engaged a profile.
    app.speech_held_bindings.push(HeldSpeechBinding::new(chord));
    app.speech_engaged_profile = None;
    app.speech_escape_release_pending = true;

    app.stop_hold_on_focus_loss(&ctx, now);

    assert_eq!(app.speech_held_bindings.len(), 1);
    assert_eq!(app.speech_held_bindings[0].binding, chord);
    assert!(app.speech_escape_release_pending);
    assert_eq!(
        app.speech_held_bindings[0].release_deadline,
        Some(now + SPEECH_RELEASE_OWNERSHIP_TIMEOUT)
    );
    assert_eq!(
        app.speech_escape_release_deadline,
        Some(now + SPEECH_RELEASE_OWNERSHIP_TIMEOUT)
    );

    app.any_viewport_focused = false;
    app.cancel_unattended_recording();

    assert_eq!(app.speech_held_bindings.len(), 1);
    assert!(app.speech_escape_release_pending);

    let later = now + Duration::from_secs(1);
    let newer_chord = horizon_core::ShortcutBinding::new(
        horizon_core::ShortcutModifiers::ALT,
        horizon_core::ShortcutKey::Letter('L'),
    );
    app.speech_held_bindings.push(HeldSpeechBinding::new(newer_chord));
    // A newly consumed Escape is a separate ownership generation.
    app.speech_escape_release_deadline = None;
    app.arm_speech_release_ownership(&ctx, later);

    assert_eq!(
        app.speech_held_bindings[0].release_deadline,
        Some(now + SPEECH_RELEASE_OWNERSHIP_TIMEOUT)
    );
    assert_eq!(
        app.speech_held_bindings[1].release_deadline,
        Some(later + SPEECH_RELEASE_OWNERSHIP_TIMEOUT)
    );
    assert_eq!(
        app.speech_escape_release_deadline,
        Some(later + SPEECH_RELEASE_OWNERSHIP_TIMEOUT)
    );

    app.expire_speech_release_ownership(now + SPEECH_RELEASE_OWNERSHIP_TIMEOUT);

    assert_eq!(app.speech_held_bindings.len(), 1);
    assert_eq!(app.speech_held_bindings[0].binding, newer_chord);
    assert!(app.speech_escape_release_pending);

    app.expire_speech_release_ownership(later + SPEECH_RELEASE_OWNERSHIP_TIMEOUT);

    assert!(app.speech_held_bindings.is_empty());
    assert!(!app.speech_escape_release_pending);
    assert!(app.speech_escape_release_deadline.is_none());
}

/// End-to-end press path for profile push-to-talk: a synthetic F-key
/// egui event must engage the matching profile, start capture, and stop
/// on release. Guards the full parse → match → engine chain that no
/// smoke lane could exercise live (the mac runner has no input device).
#[cfg(feature = "speech")]
#[test]
fn f_key_events_drive_profile_hold_dictation_end_to_end() {
    use egui::{Event, Key, Modifiers, RawInput};

    let press = |key| Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    let release = |key| Event::Key {
        key,
        physical_key: Some(key),
        pressed: false,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    let frame = |events| RawInput {
        events,
        ..RawInput::default()
    };

    let ctx = Context::default();
    let (mut speech, channels) = crate::app::speech::SpeechSystem::with_test_bindings(&["F1", "F2", "F3"]);
    let target = PanelId(7);
    let mut engaged = None;
    let mut events = Vec::new();

    // A key with no profile binding must not engage anything.
    let _ = ctx
        .run_ui(frame(vec![press(Key::K)]), |ui| {
            engaged = handle_profile_hotkeys(
                ui,
                &mut speech,
                Some(SpeechSink::Panel(target)),
                true,
                engaged,
                &mut events,
                &[],
            );
        })
        .discard_textures();
    assert_eq!(engaged, None);
    assert_eq!(speech.recording_target(), None);

    // F2 engages the second profile and starts capture into the target.
    let _ = ctx
        .run_ui(frame(vec![press(Key::F2)]), |ui| {
            engaged = handle_profile_hotkeys(
                ui,
                &mut speech,
                Some(SpeechSink::Panel(target)),
                true,
                engaged,
                &mut events,
                &[],
            );
        })
        .discard_textures();
    assert_eq!(engaged, Some(1));
    assert_eq!(speech.recording_target(), Some(target));
    assert!(channels.capture_start_requested());

    // Releasing the engaged key stops the hold (recording ends, the
    // engine moves on to awaiting the captured PCM).
    let _ = ctx
        .run_ui(frame(vec![release(Key::F2)]), |ui| {
            engaged = handle_profile_hotkeys(
                ui,
                &mut speech,
                Some(SpeechSink::Panel(target)),
                true,
                engaged,
                &mut events,
                &[],
            );
        })
        .discard_textures();
    assert_eq!(engaged, None);
    assert_eq!(speech.recording_target(), None);
    assert!(speech.is_active());
    // Unbound keys, a clean start, and a clean stop must not produce
    // ignored-press notices.
    assert!(events.is_empty());
}

#[test]
fn hold_hotkey_claims_only_an_idle_session_with_a_focused_terminal() {
    let focused = SpeechSink::Panel(PanelId(7));
    let starts = HoldHotkeyTransition {
        start_target: Some(focused),
        stop: false,
        engaged_profile: Some(1),
    };
    assert_eq!(
        hold_hotkey_transition(1, true, false, None, SpeechActivity::Idle, Some(focused)),
        starts
    );

    let ignored = HoldHotkeyTransition {
        start_target: None,
        stop: false,
        engaged_profile: None,
    };
    assert_eq!(
        hold_hotkey_transition(1, true, false, None, SpeechActivity::Recording, Some(focused)),
        ignored
    );
    assert_eq!(
        hold_hotkey_transition(1, true, false, None, SpeechActivity::Idle, None),
        ignored
    );
}

#[test]
fn hold_hotkey_same_batch_tap_stops_only_its_own_recording() {
    let focused = SpeechSink::Panel(PanelId(7));
    assert_eq!(
        hold_hotkey_transition(1, true, true, None, SpeechActivity::Idle, Some(focused)),
        HoldHotkeyTransition {
            start_target: Some(focused),
            stop: true,
            engaged_profile: None,
        }
    );

    let mic_button_press = hold_hotkey_transition(1, true, false, None, SpeechActivity::Recording, Some(focused));
    assert_eq!(mic_button_press.engaged_profile, None);
    assert!(!hold_hotkey_transition(1, false, true, None, SpeechActivity::Recording, Some(focused)).stop);
    assert!(hold_hotkey_transition(1, false, true, Some(1), SpeechActivity::Recording, Some(focused)).stop);
    assert!(!hold_hotkey_transition(2, false, true, Some(1), SpeechActivity::Recording, Some(focused)).stop);
}

#[test]
fn hold_hotkey_idle_without_a_terminal_can_start_desktop_dictation() {
    assert_eq!(
        hold_hotkey_transition(0, true, false, None, SpeechActivity::Idle, Some(SpeechSink::Desktop)),
        HoldHotkeyTransition {
            start_target: Some(SpeechSink::Desktop),
            stop: false,
            engaged_profile: Some(0),
        }
    );
}

#[test]
fn hold_hotkey_drops_stale_ownership_after_recording_ends() {
    let transition = hold_hotkey_transition(
        1,
        false,
        true,
        Some(1),
        SpeechActivity::Busy,
        Some(SpeechSink::Panel(PanelId(7))),
    );
    assert_eq!(transition.engaged_profile, None);
    assert!(!transition.stop);
}

#[cfg(feature = "speech")]
#[test]
fn global_hold_defers_same_drain_release_until_the_next_batch() {
    let _prepare = PrepareHookGuard::install(|| Ok(()));
    let (mut speech, channels) = crate::app::speech::SpeechSystem::with_test_bindings(&["F1"]);
    let sink = Some(SpeechSink::Desktop);
    let mut notices = Vec::new();
    let (engaged, deferred, disconnected) = apply_global_hotkey_events(
        &mut speech,
        [
            horizon_cursor::HotkeyEvent::Pressed(0),
            horizon_cursor::HotkeyEvent::Released(0),
        ],
        sink,
        true,
        None,
        &mut notices,
    );
    assert_eq!(engaged, Some(0));
    assert!(!disconnected);
    assert_eq!(speech.recording_sink(), Some(SpeechSink::Desktop));
    assert!(channels.capture_start_requested());
    assert_eq!(deferred, vec![horizon_cursor::HotkeyEvent::Released(0)]);
    assert!(notices.is_empty());

    let (engaged, deferred, disconnected) =
        apply_global_hotkey_events(&mut speech, deferred, sink, true, engaged, &mut notices);
    assert_eq!(engaged, None);
    assert!(!disconnected);
    assert!(deferred.is_empty());
    assert_eq!(speech.recording_sink(), None);
}

#[cfg(feature = "speech")]
#[test]
fn global_listener_disconnect_cancels_an_active_hold() {
    let _prepare = PrepareHookGuard::install(|| Ok(()));
    let (mut speech, channels) = crate::app::speech::SpeechSystem::with_test_bindings(&["F1"]);
    let sink = Some(SpeechSink::Desktop);
    let mut notices = Vec::new();
    let (engaged, _, disconnected) = apply_global_hotkey_events(
        &mut speech,
        [horizon_cursor::HotkeyEvent::Pressed(0)],
        sink,
        true,
        None,
        &mut notices,
    );
    assert_eq!(engaged, Some(0));
    assert!(!disconnected);
    assert_eq!(speech.recording_sink(), Some(SpeechSink::Desktop));
    assert!(channels.capture_start_requested());

    let (engaged, deferred, disconnected) = apply_global_hotkey_events(
        &mut speech,
        [horizon_cursor::HotkeyEvent::Disconnected],
        sink,
        true,
        engaged,
        &mut notices,
    );
    assert_eq!(engaged, None);
    assert!(disconnected);
    assert!(deferred.is_empty());
    assert_eq!(speech.recording_sink(), None);
}

#[cfg(feature = "speech")]
#[test]
fn unsafe_desktop_target_refuses_before_microphone_capture() {
    fn reject_target() -> Result<(), horizon_cursor::InjectError> {
        Err(horizon_cursor::InjectError::Target("focused field changed"))
    }

    let (mut speech, channels) = crate::app::speech::SpeechSystem::with_test_bindings(&["F1"]);
    let mut events = Vec::new();
    let _prepare = PrepareHookGuard::install(reject_target);
    let started = start_speech(&mut speech, SpeechSink::Desktop, 0, &mut events);

    assert!(!started);
    assert_eq!(speech.recording_sink(), None);
    assert!(!channels.capture_start_requested());
    assert!(matches!(
        events.as_slice(),
        [super::SpeechEvent::Error(message)]
            if message.contains("focused field changed") && message.contains("clipboard was not used")
    ));
}

#[test]
fn queued_desktop_insertion_keeps_the_captured_target_owned() {
    assert!(!should_release_desktop_target(false, false, true));
    assert!(!should_release_desktop_target(true, false, false));
    assert!(!should_release_desktop_target(false, true, false));
    assert!(should_release_desktop_target(false, false, false));
}

#[test]
fn terminal_matches_the_viewport_that_has_os_focus() {
    let root = WorkspaceId(1);
    let detached = WorkspaceId(2);
    assert!(terminal_matches_focused_viewport(root, false, true, None));
    assert!(!terminal_matches_focused_viewport(detached, true, true, None));
    assert!(terminal_matches_focused_viewport(detached, true, false, Some(detached)));
    assert!(!terminal_matches_focused_viewport(root, false, false, Some(detached)));
}

#[test]
fn dictation_inserts_into_editor_panels_at_the_caret() {
    let mut panel = Panel::spawn(
        PanelId(3),
        WorkspaceId(1),
        PanelOptions {
            kind: PanelKind::Editor,
            ..PanelOptions::default()
        },
    )
    .expect("scratch editor");
    {
        let editor = panel.editor_mut().expect("editor content");
        editor.text = "ab cd".to_owned();
        editor.set_caret(2);
    }
    assert_eq!(inject_transcript(&mut panel, "hello"), TranscriptInjection::Editor);
    let editor = panel.editor().expect("editor content");
    assert_eq!(editor.text, "abhello  cd");
    assert_eq!(editor.caret, 8);
}

#[test]
fn dictation_dispatches_insert_text_to_browser_panels() {
    let mut panel = Panel::from_content(
        PanelId(9),
        WorkspaceId(1),
        PanelKind::Browser,
        PanelContent::Browser(Box::new(BrowserPanelState::inert())),
    );
    assert_eq!(
        inject_transcript(&mut panel, "hello"),
        TranscriptInjection::BrowserInsertText("hello ".to_owned())
    );
}

#[test]
fn dictation_ignores_panels_without_a_text_surface() {
    let mut panel = Panel::spawn(
        PanelId(4),
        WorkspaceId(1),
        PanelOptions {
            kind: PanelKind::GitChanges,
            ..PanelOptions::default()
        },
    )
    .expect("git changes panel");
    assert_eq!(inject_transcript(&mut panel, "hello"), TranscriptInjection::Ignored);
}
