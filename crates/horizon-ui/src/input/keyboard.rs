use alacritty_terminal::term::TermMode;
use egui::{Key, Modifiers};

use super::sequence::build_sequence;

#[derive(Clone)]
pub struct KeyTranslation {
    pub bytes: Vec<u8>,
    pub suppress_text: Option<String>,
}

pub fn translate_key_event_with_physical(
    key: Key,
    physical_key: Option<Key>,
    pressed: bool,
    repeat: bool,
    modifiers: Modifiers,
    mode: TermMode,
) -> Option<KeyTranslation> {
    if !pressed && !mode.contains(TermMode::REPORT_EVENT_TYPES) {
        return None;
    }

    let text = printable_text(key, modifiers);
    let control = control_modifier(modifiers);
    let kitty = mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL);

    if kitty {
        let bytes = build_sequence(key, physical_key, modifiers, mode, pressed, repeat, text.as_deref())?;
        let suppress_text = pressed
            .then_some(text.as_deref())
            .flatten()
            .filter(|_| should_suppress_text_for_key(key, modifiers, mode))
            .map(ToOwned::to_owned);
        return Some(KeyTranslation { bytes, suppress_text });
    }

    if !pressed {
        return None;
    }

    if let Some(bytes) = named_key_sequence(key, modifiers, mode) {
        return Some(KeyTranslation {
            bytes,
            suppress_text: should_suppress_text_for_key(key, modifiers, mode)
                .then_some(text)
                .flatten(),
        });
    }

    if control && let Some(byte) = control_character(key) {
        let mut bytes = Vec::with_capacity(2);
        if modifiers.alt {
            bytes.push(b'\x1b');
        }
        bytes.push(byte);
        return Some(KeyTranslation {
            bytes,
            suppress_text: None,
        });
    }

    if modifiers.alt
        && let Some(text) = text
    {
        let mut bytes = Vec::with_capacity(text.len() + 1);
        bytes.push(b'\x1b');
        bytes.extend_from_slice(text.as_bytes());
        return Some(KeyTranslation {
            bytes,
            suppress_text: Some(text),
        });
    }

    None
}

pub fn translate_text_event(
    key: Key,
    physical_key: Option<Key>,
    text: &str,
    pressed: bool,
    repeat: bool,
    modifiers: Modifiers,
    mode: TermMode,
) -> Option<KeyTranslation> {
    if !mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL) {
        return None;
    }

    let bytes = build_sequence(key, physical_key, modifiers, mode, pressed, repeat, Some(text))?;
    Some(KeyTranslation {
        bytes,
        suppress_text: None,
    })
}

pub fn paste_bytes(text: &str, mode: TermMode, bracketed: bool) -> Vec<u8> {
    if bracketed && mode.contains(TermMode::BRACKETED_PASTE) {
        let filtered = text.replace(['\x1b', '\x03'], "");
        let mut bytes = Vec::with_capacity(filtered.len() + 12);
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(filtered.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        return bytes;
    }

    if bracketed {
        return text.replace("\r\n", "\r").replace('\n', "\r").into_bytes();
    }

    text.as_bytes().to_vec()
}

pub(super) fn should_suppress_text_for_key(key: Key, modifiers: Modifiers, mode: TermMode) -> bool {
    printable_text(key, modifiers).is_some() && (modifiers.alt || mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL))
}

pub fn should_defer_textual_key(
    key: Key,
    physical_key: Option<Key>,
    pressed: bool,
    modifiers: Modifiers,
    mode: TermMode,
) -> bool {
    if !pressed || !mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL) {
        return false;
    }

    let printable_base = base_key_text(key, physical_key).is_some();
    printable_base && (modifiers.alt || (!control_modifier(modifiers) && printable_text(key, modifiers).is_none()))
}

pub(super) fn control_modifier(modifiers: Modifiers) -> bool {
    modifiers.ctrl || modifiers.command
}

pub(super) fn control_character(key: Key) -> Option<u8> {
    let letter = match key {
        Key::A => Some(0x01),
        Key::B => Some(0x02),
        Key::C => Some(0x03),
        Key::D => Some(0x04),
        Key::E => Some(0x05),
        Key::F => Some(0x06),
        Key::G => Some(0x07),
        Key::H => Some(0x08),
        Key::I => Some(0x09),
        Key::J => Some(0x0a),
        Key::K => Some(0x0b),
        Key::L => Some(0x0c),
        Key::M => Some(0x0d),
        Key::N => Some(0x0e),
        Key::O => Some(0x0f),
        Key::P => Some(0x10),
        Key::Q => Some(0x11),
        Key::R => Some(0x12),
        Key::S => Some(0x13),
        Key::T => Some(0x14),
        Key::U => Some(0x15),
        Key::V => Some(0x16),
        Key::W => Some(0x17),
        Key::X => Some(0x18),
        Key::Y => Some(0x19),
        Key::Z => Some(0x1a),
        _ => None,
    };
    if letter.is_some() {
        return letter;
    }

    match key {
        Key::Space => Some(0x00),
        Key::OpenBracket | Key::OpenCurlyBracket => Some(0x1b),
        Key::Backslash | Key::Pipe => Some(0x1c),
        Key::CloseBracket | Key::CloseCurlyBracket => Some(0x1d),
        Key::Slash | Key::Questionmark => Some(0x1f),
        Key::Enter => Some(b'\r'),
        Key::Tab => Some(b'\t'),
        Key::Backspace => Some(0x7f),
        _ => None,
    }
}

fn named_key_sequence(key: Key, modifiers: Modifiers, mode: TermMode) -> Option<Vec<u8>> {
    if mode.contains(TermMode::APP_CURSOR) && !any_modifiers(modifiers) {
        let app_cursor = match key {
            Key::ArrowUp => Some(&b"\x1bOA"[..]),
            Key::ArrowDown => Some(&b"\x1bOB"[..]),
            Key::ArrowRight => Some(&b"\x1bOC"[..]),
            Key::ArrowLeft => Some(&b"\x1bOD"[..]),
            Key::Home => Some(&b"\x1bOH"[..]),
            Key::End => Some(&b"\x1bOF"[..]),
            _ => None,
        };
        if let Some(sequence) = app_cursor {
            return Some(sequence.to_vec());
        }
    }

    if let Some(sequence) = legacy_c0_sequence(key, modifiers) {
        return Some(sequence);
    }

    build_sequence(key, None, modifiers, TermMode::NONE, true, false, None)
}

fn legacy_c0_sequence(key: Key, modifiers: Modifiers) -> Option<Vec<u8>> {
    let suffix = match key {
        Key::Enter => &b"\r"[..],
        Key::Escape => &b"\x1b"[..],
        Key::Backspace if control_modifier(modifiers) => &b"\x08"[..],
        Key::Backspace => &b"\x7f"[..],
        Key::Tab if modifiers.shift => &b"\x1b[Z"[..],
        Key::Tab => &b"\t"[..],
        _ => return None,
    };

    let mut bytes = Vec::with_capacity(suffix.len() + usize::from(modifiers.alt));
    if modifiers.alt {
        bytes.push(b'\x1b');
    }
    bytes.extend_from_slice(suffix);
    Some(bytes)
}

fn any_modifiers(modifiers: Modifiers) -> bool {
    modifiers.alt || modifiers.shift || control_modifier(modifiers)
}

pub(super) fn base_key_text(key: Key, physical_key: Option<Key>) -> Option<String> {
    printable_text(physical_key.unwrap_or(key), Modifiers::NONE)
}

pub(super) fn printable_text(key: Key, modifiers: Modifiers) -> Option<String> {
    let letter = match key {
        Key::A => Some('a'),
        Key::B => Some('b'),
        Key::C => Some('c'),
        Key::D => Some('d'),
        Key::E => Some('e'),
        Key::F => Some('f'),
        Key::G => Some('g'),
        Key::H => Some('h'),
        Key::I => Some('i'),
        Key::J => Some('j'),
        Key::K => Some('k'),
        Key::L => Some('l'),
        Key::M => Some('m'),
        Key::N => Some('n'),
        Key::O => Some('o'),
        Key::P => Some('p'),
        Key::Q => Some('q'),
        Key::R => Some('r'),
        Key::S => Some('s'),
        Key::T => Some('t'),
        Key::U => Some('u'),
        Key::V => Some('v'),
        Key::W => Some('w'),
        Key::X => Some('x'),
        Key::Y => Some('y'),
        Key::Z => Some('z'),
        _ => None,
    };
    if let Some(letter) = letter {
        let letter = if modifiers.shift {
            letter.to_ascii_uppercase()
        } else {
            letter
        };
        return Some(letter.to_string());
    }

    if modifiers.shift {
        return None;
    }

    let symbol = match key {
        Key::Space => Some(" "),
        Key::Colon => Some(":"),
        Key::Comma => Some(","),
        Key::Backslash => Some("\\"),
        Key::Slash => Some("/"),
        Key::Pipe => Some("|"),
        Key::Questionmark => Some("?"),
        Key::Exclamationmark => Some("!"),
        Key::OpenBracket => Some("["),
        Key::CloseBracket => Some("]"),
        Key::OpenCurlyBracket => Some("{"),
        Key::CloseCurlyBracket => Some("}"),
        Key::Backtick => Some("`"),
        Key::Minus => Some("-"),
        Key::Period => Some("."),
        Key::Plus => Some("+"),
        Key::Equals => Some("="),
        Key::Semicolon => Some(";"),
        Key::Quote => Some("'"),
        Key::Num0 => Some("0"),
        Key::Num1 => Some("1"),
        Key::Num2 => Some("2"),
        Key::Num3 => Some("3"),
        Key::Num4 => Some("4"),
        Key::Num5 => Some("5"),
        Key::Num6 => Some("6"),
        Key::Num7 => Some("7"),
        Key::Num8 => Some("8"),
        Key::Num9 => Some("9"),
        _ => None,
    }?;

    Some(symbol.to_owned())
}

pub(super) fn is_control_character(text: &str) -> bool {
    let Some(codepoint) = text.bytes().next() else {
        return false;
    };
    text.len() == 1 && (codepoint < 0x20 || (0x7f..=0x9f).contains(&codepoint))
}

pub(super) trait KeyExt {
    fn alpha_key(self) -> bool;
}

impl KeyExt for Key {
    fn alpha_key(self) -> bool {
        matches!(
            self,
            Key::A
                | Key::B
                | Key::C
                | Key::D
                | Key::E
                | Key::F
                | Key::G
                | Key::H
                | Key::I
                | Key::J
                | Key::K
                | Key::L
                | Key::M
                | Key::N
                | Key::O
                | Key::P
                | Key::Q
                | Key::R
                | Key::S
                | Key::T
                | Key::U
                | Key::V
                | Key::W
                | Key::X
                | Key::Y
                | Key::Z
        )
    }
}
