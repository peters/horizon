use std::time::{Duration, Instant};

#[cfg(feature = "speech")]
use crate::test_egui::DiscardTextures;
use egui::Context;
use horizon_core::PanelId;

#[cfg(feature = "speech")]
use super::handle_profile_hotkeys;
use super::{HoldHotkeyTransition, SPEECH_RELEASE_OWNERSHIP_TIMEOUT, SpeechActivity, hold_hotkey_transition};
use crate::app::HeldSpeechBinding;
use crate::app::test_support::test_app;

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
            engaged = handle_profile_hotkeys(ui, &mut speech, Some(target), true, engaged, &mut events);
        })
        .discard_textures();
    assert_eq!(engaged, None);
    assert_eq!(speech.recording_target(), None);

    // F2 engages the second profile and starts capture into the target.
    let _ = ctx
        .run_ui(frame(vec![press(Key::F2)]), |ui| {
            engaged = handle_profile_hotkeys(ui, &mut speech, Some(target), true, engaged, &mut events);
        })
        .discard_textures();
    assert_eq!(engaged, Some(1));
    assert_eq!(speech.recording_target(), Some(target));
    assert!(channels.capture_start_requested());

    // Releasing the engaged key stops the hold (recording ends, the
    // engine moves on to awaiting the captured PCM).
    let _ = ctx
        .run_ui(frame(vec![release(Key::F2)]), |ui| {
            engaged = handle_profile_hotkeys(ui, &mut speech, Some(target), true, engaged, &mut events);
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
    let focused = PanelId(7);
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
    let focused = PanelId(7);
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
fn hold_hotkey_drops_stale_ownership_after_recording_ends() {
    let transition = hold_hotkey_transition(1, false, true, Some(1), SpeechActivity::Busy, Some(PanelId(7)));
    assert_eq!(transition.engaged_profile, None);
    assert!(!transition.stop);
}
