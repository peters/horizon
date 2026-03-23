use egui::{InputState, Key, Modifiers};
use horizon_core::{ShortcutBinding, ShortcutKey, ShortcutModifiers};

pub(crate) fn shortcut_pressed(input: &InputState, binding: ShortcutBinding) -> bool {
    let modifiers = egui_modifiers(binding.modifiers);
    input.events.iter().any(|event| {
        matches!(
            event,
            egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                modifiers: event_modifiers,
                ..
            } if event_modifiers.matches_logically(modifiers)
                && key_matches(*key, *physical_key, binding.key)
        )
    })
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

    use super::shortcut_pressed;

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
