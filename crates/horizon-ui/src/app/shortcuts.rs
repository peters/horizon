use egui::{Event, InputState, Key, Modifiers};
use horizon_core::{ShortcutBinding, ShortcutKey, ShortcutModifiers};

const CAPTURED_CLIPBOARD_EVENT_ID: &str = "speech_captured_clipboard_event";

pub(crate) fn shortcut_pressed(input: &InputState, binding: ShortcutBinding) -> bool {
    shortcut_pressed_in_events(&input.events, binding)
}

pub(crate) fn shortcut_pressed_in_events(events: &[Event], binding: ShortcutBinding) -> bool {
    events.iter().any(|event| shortcut_event_matches(event, binding))
}

/// Whether the binder is *actively capturing* (raw flag only), excluding the
/// captured-key-release-pending grace period. The terminal filter uses this
/// narrow form so its broad key/text swallow does not consume the captured
/// chord's release before the pending-key branch clears it.
pub(crate) fn hotkey_binder_capturing(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp(egui::Id::new("speech_hotkey_capturing")))
        .unwrap_or(false)
}

pub(in crate::app) fn mark_captured_clipboard_event(ctx: &egui::Context) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(CAPTURED_CLIPBOARD_EVENT_ID), true));
}

/// Consume the one-frame marker set when the hotkey binder saw an egui
/// clipboard pseudo-event. Settings renders before terminal input filtering,
/// so the raw capture flag has already been cleared by the time this runs.
pub(in crate::app) fn take_captured_clipboard_event(ctx: &egui::Context) -> bool {
    ctx.data_mut(|data| {
        let id = egui::Id::new(CAPTURED_CLIPBOARD_EVENT_ID);
        let captured = data.get_temp(id).unwrap_or(false);
        data.remove_temp::<bool>(id);
        captured
    })
}

pub(crate) fn is_clipboard_pseudo_event(event: &Event) -> bool {
    matches!(event, Event::Copy | Event::Cut | Event::Paste(_))
}

/// How long a just-captured chord may suppress input while waiting for its
/// key release. Generous for a real hold, short enough that a lost release
/// cannot wedge shortcuts or typing.
const PENDING_CAPTURE_TIMEOUT: f64 = 3.0;

/// Return the pending captured chord after expiring stale state. Global
/// shortcut dispatch and terminal filtering must share this cleanup path.
pub(in crate::app) fn pending_hotkey_capture(ctx: &egui::Context) -> Option<super::settings::PendingCapture> {
    let pending_id = egui::Id::new("speech_captured_key");
    let pending = ctx
        .data(|data| data.get_temp::<Option<super::settings::PendingCapture>>(pending_id))
        .flatten();
    let expired =
        pending.is_some_and(|capture| ctx.input(|input| input.time) - capture.armed_at > PENDING_CAPTURE_TIMEOUT);
    if expired {
        ctx.data_mut(|data| data.insert_temp(pending_id, None::<super::settings::PendingCapture>));
        None
    } else {
        pending
    }
}

pub(crate) fn hotkey_capture_active(ctx: &egui::Context) -> bool {
    // True while the binder is capturing, and while a just-captured chord's
    // key is still held (its repeats/release must not fire the shortcut it
    // was bound to).
    hotkey_binder_capturing(ctx) || pending_hotkey_capture(ctx).is_some()
}

/// Scan a frame's events for a press (initial, non-repeat) and a release of
/// `binding`. The release matches on the key alone because modifiers are
/// often released before the main key in push-to-talk usage.
pub(crate) fn press_and_release_in_events(events: &[Event], binding: ShortcutBinding) -> (bool, bool) {
    let mut pressed = false;
    let mut released = false;
    for event in events {
        if let Event::Key {
            key,
            physical_key,
            pressed: is_pressed,
            repeat,
            ..
        } = event
        {
            if *is_pressed && !repeat && shortcut_event_matches(event, binding) {
                pressed = true;
            }
            if !is_pressed && key_matches(*key, *physical_key, binding.key) {
                released = true;
            }
        }
    }
    (pressed, released)
}

/// Whether an event is a key event (press, repeat, or release) for the
/// binding's key, regardless of modifiers. Used to keep a push-to-talk
/// chord out of the terminal input stream.
pub(crate) fn event_uses_shortcut_key(event: &Event, binding: ShortcutBinding) -> bool {
    matches!(
        event,
        Event::Key { key, physical_key, .. } if key_matches(*key, *physical_key, binding.key)
    )
}

pub(crate) fn shortcut_event_matches(event: &Event, binding: ShortcutBinding) -> bool {
    let modifiers = egui_modifiers(binding.modifiers);
    matches!(
        event,
        Event::Key {
            key,
            physical_key,
            pressed: true,
            modifiers: event_modifiers,
            ..
        } if event_modifiers.matches_logically(modifiers)
            && key_matches(*key, *physical_key, binding.key)
    )
}

fn egui_modifiers(modifiers: ShortcutModifiers) -> Modifiers {
    Modifiers {
        alt: modifiers.alt(),
        ctrl: modifiers.ctrl(),
        shift: modifiers.shift(),
        mac_cmd: modifiers.mac_cmd(),
        command: modifiers.command(),
    }
}

fn key_matches(logical_key: Key, physical_key: Option<Key>, shortcut_key: ShortcutKey) -> bool {
    match shortcut_key {
        ShortcutKey::ArrowDown => logical_key == Key::ArrowDown,
        ShortcutKey::ArrowLeft => logical_key == Key::ArrowLeft,
        ShortcutKey::ArrowRight => logical_key == Key::ArrowRight,
        ShortcutKey::ArrowUp => logical_key == Key::ArrowUp,
        ShortcutKey::Escape => logical_key == Key::Escape,
        ShortcutKey::Enter => logical_key == Key::Enter,
        ShortcutKey::Tab => logical_key == Key::Tab,
        ShortcutKey::Comma => logical_key == Key::Comma,
        ShortcutKey::Minus => logical_key == Key::Minus,
        ShortcutKey::Plus => logical_key == Key::Plus || logical_key == Key::Equals,
        ShortcutKey::Digit(digit) => {
            digit_key(digit).is_some_and(|key| logical_key == key || physical_key == Some(key))
        }
        ShortcutKey::Letter(letter) => letter_key(letter).is_some_and(|key| logical_key == key),
        ShortcutKey::Function(function) => function_key(function).is_some_and(|key| logical_key == key),
    }
}

fn digit_key(digit: u8) -> Option<Key> {
    Some(match digit {
        0 => Key::Num0,
        1 => Key::Num1,
        2 => Key::Num2,
        3 => Key::Num3,
        4 => Key::Num4,
        5 => Key::Num5,
        6 => Key::Num6,
        7 => Key::Num7,
        8 => Key::Num8,
        9 => Key::Num9,
        _ => return None,
    })
}

fn letter_key(letter: char) -> Option<Key> {
    Some(match letter {
        'A' => Key::A,
        'B' => Key::B,
        'C' => Key::C,
        'D' => Key::D,
        'E' => Key::E,
        'F' => Key::F,
        'G' => Key::G,
        'H' => Key::H,
        'I' => Key::I,
        'J' => Key::J,
        'K' => Key::K,
        'L' => Key::L,
        'M' => Key::M,
        'N' => Key::N,
        'O' => Key::O,
        'P' => Key::P,
        'Q' => Key::Q,
        'R' => Key::R,
        'S' => Key::S,
        'T' => Key::T,
        'U' => Key::U,
        'V' => Key::V,
        'W' => Key::W,
        'X' => Key::X,
        'Y' => Key::Y,
        'Z' => Key::Z,
        _ => return None,
    })
}

fn function_key(function: u8) -> Option<Key> {
    Some(match function {
        1 => Key::F1,
        2 => Key::F2,
        3 => Key::F3,
        4 => Key::F4,
        5 => Key::F5,
        6 => Key::F6,
        7 => Key::F7,
        8 => Key::F8,
        9 => Key::F9,
        10 => Key::F10,
        11 => Key::F11,
        12 => Key::F12,
        13 => Key::F13,
        14 => Key::F14,
        15 => Key::F15,
        16 => Key::F16,
        17 => Key::F17,
        18 => Key::F18,
        19 => Key::F19,
        20 => Key::F20,
        21 => Key::F21,
        22 => Key::F22,
        23 => Key::F23,
        24 => Key::F24,
        25 => Key::F25,
        26 => Key::F26,
        27 => Key::F27,
        28 => Key::F28,
        29 => Key::F29,
        30 => Key::F30,
        31 => Key::F31,
        32 => Key::F32,
        33 => Key::F33,
        34 => Key::F34,
        35 => Key::F35,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use egui::{Event, Key, Modifiers, RawInput};
    use horizon_core::{ShortcutBinding, ShortcutKey, ShortcutModifiers};

    use super::{
        event_uses_shortcut_key, hotkey_capture_active, mark_captured_clipboard_event, press_and_release_in_events,
        shortcut_pressed, take_captured_clipboard_event,
    };

    #[test]
    fn captured_clipboard_marker_is_consumed_once() {
        let ctx = egui::Context::default();
        mark_captured_clipboard_event(&ctx);

        assert!(take_captured_clipboard_event(&ctx));
        assert!(!take_captured_clipboard_event(&ctx));
    }

    fn key_event(key: Key, pressed: bool, repeat: bool, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat,
            modifiers,
        }
    }

    #[test]
    fn initial_press_detected_but_repeats_ignored() {
        let binding = ShortcutBinding::new(ShortcutModifiers::NONE, ShortcutKey::Function(9));
        let initial = [key_event(Key::F9, true, false, Modifiers::NONE)];
        assert_eq!(press_and_release_in_events(&initial, binding), (true, false));

        let repeat = [key_event(Key::F9, true, true, Modifiers::NONE)];
        assert_eq!(press_and_release_in_events(&repeat, binding), (false, false));
    }

    #[test]
    fn release_matches_on_key_alone_even_with_modifiers_dropped() {
        let binding = ShortcutBinding::new(
            ShortcutModifiers::CTRL.plus(ShortcutModifiers::SHIFT),
            ShortcutKey::Letter('K'),
        );
        // Modifiers already released before the main key: still a release.
        let events = [key_event(Key::K, false, false, Modifiers::NONE)];
        assert_eq!(press_and_release_in_events(&events, binding), (false, true));
    }

    #[test]
    fn press_requires_full_chord() {
        let binding = ShortcutBinding::new(ShortcutModifiers::CTRL, ShortcutKey::Letter('K'));
        let bare = [key_event(Key::K, true, false, Modifiers::NONE)];
        assert_eq!(press_and_release_in_events(&bare, binding), (false, false));
    }

    #[test]
    fn event_uses_shortcut_key_ignores_modifiers_but_not_key() {
        let binding = ShortcutBinding::new(ShortcutModifiers::CTRL, ShortcutKey::Letter('K'));
        assert!(event_uses_shortcut_key(
            &key_event(Key::K, true, false, Modifiers::SHIFT),
            binding
        ));
        assert!(!event_uses_shortcut_key(
            &key_event(Key::J, true, false, Modifiers::CTRL),
            binding
        ));
    }

    #[test]
    fn pending_captured_key_keeps_hotkey_capture_active() {
        let ctx = egui::Context::default();
        assert!(!hotkey_capture_active(&ctx));

        ctx.data_mut(|data| {
            data.insert_temp(
                egui::Id::new("speech_captured_key"),
                Some(crate::app::settings::PendingCapture {
                    key: Key::F11,
                    physical_key: Some(Key::F11),
                    shifted: false,
                    clipboard: None,
                    armed_at: 1.0,
                }),
            );
        });

        assert!(hotkey_capture_active(&ctx));
    }

    #[test]
    fn expired_pending_capture_does_not_block_global_shortcuts() {
        let ctx = egui::Context::default();
        ctx.begin_pass(RawInput {
            time: Some(5.0),
            ..RawInput::default()
        });
        let pending_id = egui::Id::new("speech_captured_key");
        ctx.data_mut(|data| {
            data.insert_temp(
                pending_id,
                Some(crate::app::settings::PendingCapture {
                    key: Key::F11,
                    physical_key: Some(Key::F11),
                    shifted: false,
                    clipboard: None,
                    armed_at: 1.0,
                }),
            );
        });

        assert!(!hotkey_capture_active(&ctx));
        let pending = ctx.data(|data| {
            data.get_temp::<Option<crate::app::settings::PendingCapture>>(pending_id)
                .flatten()
        });
        assert!(pending.is_none());
        let _ = ctx.end_pass();
    }

    #[test]
    fn plus_shortcuts_accept_equals_keypress() {
        let binding = ShortcutBinding::new(ShortcutModifiers::PRIMARY, ShortcutKey::Plus);
        let mut raw = RawInput {
            modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
            ..RawInput::default()
        };
        raw.events.push(Event::Key {
            key: Key::Equals,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: raw.modifiers,
        });

        let input = egui::InputState::default().begin_pass(raw, false, 1.0, egui::InputOptions::default());

        assert!(shortcut_pressed(&input, binding));
    }

    #[test]
    fn primary_shortcuts_use_command_semantics() {
        let binding = ShortcutBinding::new(ShortcutModifiers::PRIMARY, ShortcutKey::Letter('K'));
        let mut raw = RawInput {
            modifiers: Modifiers::COMMAND,
            ..RawInput::default()
        };
        raw.events.push(Event::Key {
            key: Key::K,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: raw.modifiers,
        });

        let input = egui::InputState::default().begin_pass(raw, false, 1.0, egui::InputOptions::default());

        assert!(shortcut_pressed(&input, binding));
    }

    #[test]
    fn digit_shortcuts_match_physical_digit_keys_when_layout_changes_logical_key() {
        let binding = ShortcutBinding::new(ShortcutModifiers::PRIMARY, ShortcutKey::Digit(0));
        let mut raw = RawInput {
            modifiers: Modifiers::COMMAND,
            ..RawInput::default()
        };
        raw.events.push(Event::Key {
            key: Key::Quote,
            physical_key: Some(Key::Num0),
            pressed: true,
            repeat: false,
            modifiers: raw.modifiers,
        });

        let input = egui::InputState::default().begin_pass(raw, false, 1.0, egui::InputOptions::default());

        assert!(shortcut_pressed(&input, binding));
    }
}
