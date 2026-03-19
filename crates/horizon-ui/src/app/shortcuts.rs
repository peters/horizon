use egui::{InputState, Key, Modifiers};
use horizon_core::{ShortcutBinding, ShortcutKey, ShortcutModifiers};

pub(crate) fn shortcut_pressed(input: &InputState, binding: ShortcutBinding) -> bool {
    input.modifiers.matches_logically(egui_modifiers(binding.modifiers)) && key_pressed(input, binding.key)
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

fn key_pressed(input: &InputState, key: ShortcutKey) -> bool {
    match key {
        ShortcutKey::ArrowDown => input.key_pressed(Key::ArrowDown),
        ShortcutKey::ArrowLeft => input.key_pressed(Key::ArrowLeft),
        ShortcutKey::ArrowRight => input.key_pressed(Key::ArrowRight),
        ShortcutKey::ArrowUp => input.key_pressed(Key::ArrowUp),
        ShortcutKey::Escape => input.key_pressed(Key::Escape),
        ShortcutKey::Enter => input.key_pressed(Key::Enter),
        ShortcutKey::Tab => input.key_pressed(Key::Tab),
        ShortcutKey::Comma => input.key_pressed(Key::Comma),
        ShortcutKey::Minus => input.key_pressed(Key::Minus),
        ShortcutKey::Plus => input.key_pressed(Key::Plus) || input.key_pressed(Key::Equals),
        ShortcutKey::Digit(0) => input.key_pressed(Key::Num0),
        ShortcutKey::Digit(1) => input.key_pressed(Key::Num1),
        ShortcutKey::Digit(2) => input.key_pressed(Key::Num2),
        ShortcutKey::Digit(3) => input.key_pressed(Key::Num3),
        ShortcutKey::Digit(4) => input.key_pressed(Key::Num4),
        ShortcutKey::Digit(5) => input.key_pressed(Key::Num5),
        ShortcutKey::Digit(6) => input.key_pressed(Key::Num6),
        ShortcutKey::Digit(7) => input.key_pressed(Key::Num7),
        ShortcutKey::Digit(8) => input.key_pressed(Key::Num8),
        ShortcutKey::Digit(9) => input.key_pressed(Key::Num9),
        ShortcutKey::Letter('A') => input.key_pressed(Key::A),
        ShortcutKey::Letter('B') => input.key_pressed(Key::B),
        ShortcutKey::Letter('C') => input.key_pressed(Key::C),
        ShortcutKey::Letter('D') => input.key_pressed(Key::D),
        ShortcutKey::Letter('E') => input.key_pressed(Key::E),
        ShortcutKey::Letter('F') => input.key_pressed(Key::F),
        ShortcutKey::Letter('G') => input.key_pressed(Key::G),
        ShortcutKey::Letter('H') => input.key_pressed(Key::H),
        ShortcutKey::Letter('I') => input.key_pressed(Key::I),
        ShortcutKey::Letter('J') => input.key_pressed(Key::J),
        ShortcutKey::Letter('K') => input.key_pressed(Key::K),
        ShortcutKey::Letter('L') => input.key_pressed(Key::L),
        ShortcutKey::Letter('M') => input.key_pressed(Key::M),
        ShortcutKey::Letter('N') => input.key_pressed(Key::N),
        ShortcutKey::Letter('O') => input.key_pressed(Key::O),
        ShortcutKey::Letter('P') => input.key_pressed(Key::P),
        ShortcutKey::Letter('Q') => input.key_pressed(Key::Q),
        ShortcutKey::Letter('R') => input.key_pressed(Key::R),
        ShortcutKey::Letter('S') => input.key_pressed(Key::S),
        ShortcutKey::Letter('T') => input.key_pressed(Key::T),
        ShortcutKey::Letter('U') => input.key_pressed(Key::U),
        ShortcutKey::Letter('V') => input.key_pressed(Key::V),
        ShortcutKey::Letter('W') => input.key_pressed(Key::W),
        ShortcutKey::Letter('X') => input.key_pressed(Key::X),
        ShortcutKey::Letter('Y') => input.key_pressed(Key::Y),
        ShortcutKey::Letter('Z') => input.key_pressed(Key::Z),
        ShortcutKey::Function(1) => input.key_pressed(Key::F1),
        ShortcutKey::Function(2) => input.key_pressed(Key::F2),
        ShortcutKey::Function(3) => input.key_pressed(Key::F3),
        ShortcutKey::Function(4) => input.key_pressed(Key::F4),
        ShortcutKey::Function(5) => input.key_pressed(Key::F5),
        ShortcutKey::Function(6) => input.key_pressed(Key::F6),
        ShortcutKey::Function(7) => input.key_pressed(Key::F7),
        ShortcutKey::Function(8) => input.key_pressed(Key::F8),
        ShortcutKey::Function(9) => input.key_pressed(Key::F9),
        ShortcutKey::Function(10) => input.key_pressed(Key::F10),
        ShortcutKey::Function(11) => input.key_pressed(Key::F11),
        ShortcutKey::Function(12) => input.key_pressed(Key::F12),
        ShortcutKey::Function(13) => input.key_pressed(Key::F13),
        ShortcutKey::Function(14) => input.key_pressed(Key::F14),
        ShortcutKey::Function(15) => input.key_pressed(Key::F15),
        ShortcutKey::Function(16) => input.key_pressed(Key::F16),
        ShortcutKey::Function(17) => input.key_pressed(Key::F17),
        ShortcutKey::Function(18) => input.key_pressed(Key::F18),
        ShortcutKey::Function(19) => input.key_pressed(Key::F19),
        ShortcutKey::Function(20) => input.key_pressed(Key::F20),
        ShortcutKey::Function(21) => input.key_pressed(Key::F21),
        ShortcutKey::Function(22) => input.key_pressed(Key::F22),
        ShortcutKey::Function(23) => input.key_pressed(Key::F23),
        ShortcutKey::Function(24) => input.key_pressed(Key::F24),
        ShortcutKey::Function(25) => input.key_pressed(Key::F25),
        ShortcutKey::Function(26) => input.key_pressed(Key::F26),
        ShortcutKey::Function(27) => input.key_pressed(Key::F27),
        ShortcutKey::Function(28) => input.key_pressed(Key::F28),
        ShortcutKey::Function(29) => input.key_pressed(Key::F29),
        ShortcutKey::Function(30) => input.key_pressed(Key::F30),
        ShortcutKey::Function(31) => input.key_pressed(Key::F31),
        ShortcutKey::Function(32) => input.key_pressed(Key::F32),
        ShortcutKey::Function(33) => input.key_pressed(Key::F33),
        ShortcutKey::Function(34) => input.key_pressed(Key::F34),
        ShortcutKey::Function(35) => input.key_pressed(Key::F35),
        ShortcutKey::Digit(_) | ShortcutKey::Letter(_) | ShortcutKey::Function(_) => false,
    }
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
}
