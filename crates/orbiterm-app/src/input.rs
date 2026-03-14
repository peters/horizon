use egui::{Key, Modifiers};

/// Convert an egui key event to the terminal byte sequence.
pub fn key_to_bytes(key: Key, modifiers: Modifiers) -> Option<Vec<u8>> {
    if modifiers.ctrl {
        let ctrl_char = match key {
            Key::A => Some(1),
            Key::B => Some(2),
            Key::C => Some(3),
            Key::D => Some(4),
            Key::E => Some(5),
            Key::F => Some(6),
            Key::G => Some(7),
            Key::H => Some(8),
            Key::I => Some(9),
            Key::J => Some(10),
            Key::K => Some(11),
            Key::L => Some(12),
            Key::M => Some(13),
            Key::N => Some(14),
            Key::O => Some(15),
            Key::P => Some(16),
            Key::Q => Some(17),
            Key::R => Some(18),
            Key::S => Some(19),
            Key::T => Some(20),
            Key::U => Some(21),
            Key::V => Some(22),
            Key::W => Some(23),
            Key::X => Some(24),
            Key::Y => Some(25),
            Key::Z => Some(26),
            _ => None,
        };
        return ctrl_char.map(|c| vec![c]);
    }

    match key {
        Key::Enter => Some(b"\r".to_vec()),
        Key::Backspace => Some(b"\x7f".to_vec()),
        Key::Tab => Some(b"\t".to_vec()),
        Key::Escape => Some(b"\x1b".to_vec()),
        Key::ArrowUp => Some(b"\x1b[A".to_vec()),
        Key::ArrowDown => Some(b"\x1b[B".to_vec()),
        Key::ArrowRight => Some(b"\x1b[C".to_vec()),
        Key::ArrowLeft => Some(b"\x1b[D".to_vec()),
        Key::Home => Some(b"\x1b[H".to_vec()),
        Key::End => Some(b"\x1b[F".to_vec()),
        Key::PageUp => Some(b"\x1b[5~".to_vec()),
        Key::PageDown => Some(b"\x1b[6~".to_vec()),
        Key::Delete => Some(b"\x1b[3~".to_vec()),
        Key::Insert => Some(b"\x1b[2~".to_vec()),
        Key::F1 => Some(b"\x1bOP".to_vec()),
        Key::F2 => Some(b"\x1bOQ".to_vec()),
        Key::F3 => Some(b"\x1bOR".to_vec()),
        Key::F4 => Some(b"\x1bOS".to_vec()),
        Key::F5 => Some(b"\x1b[15~".to_vec()),
        Key::F6 => Some(b"\x1b[17~".to_vec()),
        Key::F7 => Some(b"\x1b[18~".to_vec()),
        Key::F8 => Some(b"\x1b[19~".to_vec()),
        Key::F9 => Some(b"\x1b[20~".to_vec()),
        Key::F10 => Some(b"\x1b[21~".to_vec()),
        Key::F11 => Some(b"\x1b[23~".to_vec()),
        Key::F12 => Some(b"\x1b[24~".to_vec()),
        _ => None,
    }
}
