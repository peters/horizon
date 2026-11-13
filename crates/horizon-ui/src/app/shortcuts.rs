use egui::{Event, InputState, Key, Modifiers};
use horizon_core::{ShortcutBinding, ShortcutKey, ShortcutModifiers};

pub(crate) fn shortcut_key_may_emit_text(key: horizon_core::ShortcutKey) -> bool {
    matches!(
        key,
        horizon_core::ShortcutKey::Letter(_)
            | horizon_core::ShortcutKey::Digit(_)
            | horizon_core::ShortcutKey::Comma
            | horizon_core::ShortcutKey::Minus
            | horizon_core::ShortcutKey::Plus
    )
}

pub(crate) fn shortcut_pressed(input: &InputState, binding: ShortcutBinding) -> bool {
    shortcut_pressed_in_events(&input.events, binding)
}

/// Whether `events` hold one press of `binding`: the frame in which its key goes
/// down.
///
/// An auto-repeat of a key that is already down is the continuation of a press and
/// not a second one. A held chord therefore runs its action once, and a gesture
/// that starts a long job — a speech take, a new panel — cannot run it again for
/// every repeat that the key sends while the person keeps it down.
pub(crate) fn shortcut_pressed_in_events(events: &[Event], binding: ShortcutBinding) -> bool {
    events.iter().any(|event| shortcut_event_matches(event, binding))
}

/// Takes the events of a shortcut that Horizon handled out of this frame, so a
/// consumer after it, such as the terminal of the panel that the command focused,
/// never sees the chord. The text that a printable key of the chord emits goes
/// with it.
pub(crate) fn consume_shortcut(ctx: &egui::Context, binding: ShortcutBinding) {
    ctx.input_mut(|input| {
        let mut after_chord = false;
        input.events.retain(|event| {
            let chord = event_uses_shortcut_key(event, binding);
            let text_of_chord = after_chord && matches!(event, Event::Text(_));
            after_chord = chord && shortcut_key_may_emit_text(binding.key);
            !chord && !text_of_chord
        });
    });
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

/// Whether `events` hold a release of the key of `binding`, whatever the
/// modifiers are: a person who lets go of a modifier first still ends the gesture
/// that the press started.
pub(crate) fn shortcut_released_in_events(events: &[Event], binding: ShortcutBinding) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            Event::Key {
                key,
                physical_key,
                pressed: false,
                ..
            } if key_matches(*key, *physical_key, binding.key)
        )
    })
}

/// Whether `events` hold a press of `key`, for a key that a gesture reserves.
pub(crate) fn key_pressed_in_events(events: &[Event], key: Key) -> bool {
    events
        .iter()
        .any(|event| matches!(event, Event::Key { key: pressed_key, pressed: true, repeat: false, .. } if *pressed_key == key))
}

/// Whether `event` is the press of `binding`, and not the release of it and not the
/// auto-repeat that a held key sends.
pub(crate) fn shortcut_event_matches(event: &Event, binding: ShortcutBinding) -> bool {
    let modifiers = egui_modifiers(binding.modifiers);
    matches!(
        event,
        Event::Key {
            key,
            physical_key,
            pressed: true,
            repeat: false,
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
        // Shift changes the character of the comma key on some layouts: it is
        // `;` on a Norwegian layout, where egui then knows the logical key as
        // `Semicolon`. The person presses the comma keycap that the menu names,
        // so the position of the key decides. The minus and plus keys cannot
        // follow their position this way: on a Norwegian layout the position
        // that winit reports as `Minus` carries the `+` keycap, and a fallback
        // there would run the zoom out for a person who asked to zoom in.
        ShortcutKey::Comma => logical_key == Key::Comma || physical_key == Some(Key::Comma),
        ShortcutKey::Minus => logical_key == Key::Minus,
        ShortcutKey::Plus => logical_key == Key::Plus || logical_key == Key::Equals,
        ShortcutKey::Digit(digit) => {
            digit_key(digit).is_some_and(|key| logical_key == key || physical_key == Some(key))
        }
        ShortcutKey::Letter(letter) => letter_key(letter).is_some_and(|key| logical_key == key),
        // Match the physical key as well (as digits above do): X11 input-
        // method bridges and remapped layouts can deliver an F-key press
        // whose logical key differs, and a function-key binding should
        // follow the keycap actually pressed.
        ShortcutKey::Function(function) => {
            function_key(function).is_some_and(|key| logical_key == key || physical_key == Some(key))
        }
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

    use super::{event_uses_shortcut_key, shortcut_pressed};

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
    fn a_repeat_of_a_held_chord_is_not_a_second_press() {
        let binding = ShortcutBinding::new(ShortcutModifiers::NONE, ShortcutKey::Function(9));
        // egui reads `repeat` from the keys that are down, so the press and the
        // auto-repeat that the window system sends after it are two passes of one
        // state of the input.
        let mut press = RawInput::default();
        press.events.push(key_event(Key::F9, true, false, Modifiers::NONE));
        let input = egui::InputState::default().begin_pass(press, false, 1.0, egui::InputOptions::default());
        assert!(shortcut_pressed(&input, binding), "the press of a key is one action");

        let mut repeat = RawInput::default();
        repeat.events.push(key_event(Key::F9, true, true, Modifiers::NONE));
        let input = input.begin_pass(repeat, false, 1.0, egui::InputOptions::default());
        assert!(
            !shortcut_pressed(&input, binding),
            "a key that is already down runs its action once"
        );

        let mut release = RawInput::default();
        release.events.push(key_event(Key::F9, false, false, Modifiers::NONE));
        let input = input.begin_pass(release, false, 1.0, egui::InputOptions::default());
        assert!(!shortcut_pressed(&input, binding), "the release is no press");
    }

    #[test]
    fn plus_shortcuts_accept_equals_keypress() {
        let binding = ShortcutBinding::new(ShortcutModifiers::PRIMARY, ShortcutKey::Plus);
        let modifiers = Modifiers::COMMAND | Modifiers::SHIFT;
        let mut raw = RawInput::default();
        raw.events.push(Event::Key {
            key: Key::Equals,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        });

        let input = egui::InputState::default().begin_pass(raw, false, 1.0, egui::InputOptions::default());

        assert!(shortcut_pressed(&input, binding));
    }

    #[test]
    fn primary_shortcuts_use_command_semantics() {
        let binding = ShortcutBinding::new(ShortcutModifiers::PRIMARY, ShortcutKey::Letter('K'));
        let modifiers = Modifiers::COMMAND;
        let mut raw = RawInput::default();
        raw.events.push(Event::Key {
            key: Key::K,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        });

        let input = egui::InputState::default().begin_pass(raw, false, 1.0, egui::InputOptions::default());

        assert!(shortcut_pressed(&input, binding));
    }

    /// The Mac event of Cmd+Shift+comma on a Norwegian layout, taken from a
    /// trace of that system: the layout composes `;` and the position of the
    /// key stays the comma keycap.
    fn shift_comma_event(logical_key: Key, physical_key: Key) -> Event {
        Event::Key {
            key: logical_key,
            physical_key: Some(physical_key),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
        }
    }

    #[test]
    fn settings_opens_from_the_comma_keycap_of_a_layout_that_composes_semicolon() {
        let binding = ShortcutBinding::new(ShortcutModifiers::PRIMARY_SHIFT, ShortcutKey::Comma);
        let mut raw = RawInput::default();
        raw.events.push(shift_comma_event(Key::Semicolon, Key::Comma));

        let input = egui::InputState::default().begin_pass(raw, false, 1.0, egui::InputOptions::default());

        assert!(shortcut_pressed(&input, binding));
    }

    #[test]
    fn a_semicolon_keycap_alone_does_not_open_settings() {
        let binding = ShortcutBinding::new(ShortcutModifiers::PRIMARY_SHIFT, ShortcutKey::Comma);
        let mut raw = RawInput::default();
        // A layout that has both keycaps reports the semicolon keycap as
        // `Semicolon` at its own position.
        raw.events.push(shift_comma_event(Key::Semicolon, Key::Semicolon));

        let input = egui::InputState::default().begin_pass(raw, false, 1.0, egui::InputOptions::default());

        assert!(!shortcut_pressed(&input, binding));
    }

    #[test]
    fn a_plus_keycap_does_not_run_the_zoom_out_of_its_position() {
        // The position that a Norwegian layout reports as `Minus` carries the
        // `+` keycap, so the zoom out must not follow that position.
        let zoom_out = ShortcutBinding::new(ShortcutModifiers::PRIMARY, ShortcutKey::Minus);
        let mut raw = RawInput::default();
        raw.events.push(Event::Key {
            key: Key::Plus,
            physical_key: Some(Key::Minus),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        });

        let input = egui::InputState::default().begin_pass(raw, false, 1.0, egui::InputOptions::default());

        assert!(!shortcut_pressed(&input, zoom_out));
    }

    #[test]
    fn digit_shortcuts_match_physical_digit_keys_when_layout_changes_logical_key() {
        let binding = ShortcutBinding::new(ShortcutModifiers::PRIMARY, ShortcutKey::Digit(0));
        let modifiers = Modifiers::COMMAND;
        let mut raw = RawInput::default();
        raw.events.push(Event::Key {
            key: Key::Quote,
            physical_key: Some(Key::Num0),
            pressed: true,
            repeat: false,
            modifiers,
        });

        let input = egui::InputState::default().begin_pass(raw, false, 1.0, egui::InputOptions::default());

        assert!(shortcut_pressed(&input, binding));
    }
}
