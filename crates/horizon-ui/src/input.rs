use std::borrow::Cow;
use std::fmt::Write;

use alacritty_terminal::term::TermMode;
use egui::{Key, Modifiers, MouseWheelUnit, PointerButton, Vec2};

pub struct KeyTranslation {
    pub bytes: Vec<u8>,
    pub suppress_text: Option<String>,
}

#[derive(Clone, Copy)]
pub struct GridPoint {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Default)]
pub struct PointerButtons {
    pub primary: bool,
    pub middle: bool,
    pub secondary: bool,
}

pub enum WheelAction {
    Pty(Vec<u8>),
    Scrollback(i32),
}

pub fn translate_key_event(
    key: Key,
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
        let bytes = build_sequence(key, modifiers, mode, pressed, repeat, text.as_deref())?;
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

pub fn mouse_button_report(
    button: PointerButton,
    pressed: bool,
    modifiers: Modifiers,
    mode: TermMode,
    point: GridPoint,
) -> Option<Vec<u8>> {
    if modifiers.shift || !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }

    let code = match button {
        PointerButton::Primary => 0,
        PointerButton::Middle => 1,
        PointerButton::Secondary => 2,
        PointerButton::Extra1 | PointerButton::Extra2 => return None,
    };

    Some(mouse_report(code, pressed, modifiers, mode, point))
}

pub fn mouse_motion_report(
    buttons: PointerButtons,
    modifiers: Modifiers,
    mode: TermMode,
    point: GridPoint,
) -> Option<Vec<u8>> {
    if modifiers.shift || !mode.intersects(TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG) {
        return None;
    }

    let code = if buttons.primary {
        Some(32)
    } else if buttons.middle {
        Some(33)
    } else if buttons.secondary {
        Some(34)
    } else if mode.contains(TermMode::MOUSE_MOTION) {
        Some(35)
    } else {
        None
    }?;

    Some(mouse_report(code, true, modifiers, mode, point))
}

pub fn wheel_action(
    delta: Vec2,
    unit: MouseWheelUnit,
    cell_size: Vec2,
    modifiers: Modifiers,
    mode: TermMode,
    point: GridPoint,
) -> Option<WheelAction> {
    let vertical = discrete_scroll_steps(delta.y, unit, cell_size.y);
    let horizontal = discrete_scroll_steps(delta.x, unit, cell_size.x);

    if vertical == 0 && horizontal == 0 {
        return None;
    }

    if !modifiers.shift && mode.intersects(TermMode::MOUSE_MODE) {
        let mut bytes = Vec::new();
        append_wheel_reports(&mut bytes, vertical, horizontal, modifiers, mode, point);
        return Some(WheelAction::Pty(bytes));
    }

    if !modifiers.shift && mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
        let mut bytes = Vec::new();
        append_alternate_scroll(&mut bytes, vertical, horizontal);
        return Some(WheelAction::Pty(bytes));
    }

    if vertical != 0 {
        return Some(WheelAction::Scrollback(vertical));
    }

    None
}

fn append_wheel_reports(
    bytes: &mut Vec<u8>,
    vertical: i32,
    horizontal: i32,
    modifiers: Modifiers,
    mode: TermMode,
    point: GridPoint,
) {
    let vertical_code = if vertical > 0 { 64 } else { 65 };
    for _ in 0..vertical.unsigned_abs() {
        bytes.extend(mouse_report(vertical_code, true, modifiers, mode, point));
    }

    let horizontal_code = if horizontal > 0 { 66 } else { 67 };
    for _ in 0..horizontal.unsigned_abs() {
        bytes.extend(mouse_report(horizontal_code, true, modifiers, mode, point));
    }
}

fn append_alternate_scroll(bytes: &mut Vec<u8>, vertical: i32, horizontal: i32) {
    let vertical_code = if vertical > 0 { b'A' } else { b'B' };
    for _ in 0..vertical.unsigned_abs() {
        bytes.extend_from_slice(b"\x1bO");
        bytes.push(vertical_code);
    }

    let horizontal_code = if horizontal > 0 { b'D' } else { b'C' };
    for _ in 0..horizontal.unsigned_abs() {
        bytes.extend_from_slice(b"\x1bO");
        bytes.push(horizontal_code);
    }
}

fn mouse_report(button: u8, pressed: bool, modifiers: Modifiers, mode: TermMode, point: GridPoint) -> Vec<u8> {
    let mut code = button;
    if modifiers.shift {
        code += 4;
    }
    if modifiers.alt {
        code += 8;
    }
    if control_modifier(modifiers) {
        code += 16;
    }

    if mode.contains(TermMode::SGR_MOUSE) {
        let suffix = if pressed { 'M' } else { 'm' };
        return format!("\x1b[<{};{};{}{}", code, point.column + 1, point.line + 1, suffix).into_bytes();
    }

    let button = if pressed { code } else { 3 + code };
    normal_mouse_report(point, button, mode.contains(TermMode::UTF8_MOUSE))
}

fn normal_mouse_report(point: GridPoint, button: u8, utf8: bool) -> Vec<u8> {
    let max_point = if utf8 { 2015 } else { 223 };
    if point.line >= max_point || point.column >= max_point {
        return Vec::new();
    }

    let mut bytes = vec![b'\x1b', b'[', b'M', 32 + button];
    append_mouse_position(&mut bytes, point.column, utf8);
    append_mouse_position(&mut bytes, point.line, utf8);
    bytes
}

fn append_mouse_position(bytes: &mut Vec<u8>, position: usize, utf8: bool) {
    if utf8 && position >= 95 {
        let encoded = 32 + 1 + position;
        let first = 0xC0 + encoded / 64;
        let second = 0x80 + (encoded & 63);
        bytes.push(u8::try_from(first).unwrap_or(u8::MAX));
        bytes.push(u8::try_from(second).unwrap_or(u8::MAX));
        return;
    }

    bytes.push(32 + 1 + u8::try_from(position).unwrap_or(u8::MAX));
}

fn discrete_scroll_steps(delta: f32, unit: MouseWheelUnit, cell_extent: f32) -> i32 {
    if !delta.is_finite() {
        return 0;
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    match unit {
        MouseWheelUnit::Line | MouseWheelUnit::Page => {
            delta.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32
        }
        MouseWheelUnit::Point => {
            if !cell_extent.is_finite() || cell_extent <= 0.0 {
                return 0;
            }
            (delta / cell_extent).round().clamp(i32::MIN as f32, i32::MAX as f32) as i32
        }
    }
}

fn should_suppress_text_for_key(key: Key, modifiers: Modifiers, mode: TermMode) -> bool {
    printable_text(key, modifiers).is_some() && (modifiers.alt || mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL))
}

fn control_modifier(modifiers: Modifiers) -> bool {
    modifiers.ctrl || modifiers.command
}

fn control_character(key: Key) -> Option<u8> {
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
            Key::ArrowUp => Some(Cow::Borrowed(&b"\x1bOA"[..])),
            Key::ArrowDown => Some(Cow::Borrowed(&b"\x1bOB"[..])),
            Key::ArrowRight => Some(Cow::Borrowed(&b"\x1bOC"[..])),
            Key::ArrowLeft => Some(Cow::Borrowed(&b"\x1bOD"[..])),
            Key::Home => Some(Cow::Borrowed(&b"\x1bOH"[..])),
            Key::End => Some(Cow::Borrowed(&b"\x1bOF"[..])),
            _ => None,
        };
        if let Some(sequence) = app_cursor {
            return Some(sequence.into_owned());
        }
    }

    if key == Key::Tab && modifiers.shift && !control_modifier(modifiers) && !modifiers.alt {
        return Some(b"\x1b[Z".to_vec());
    }

    if matches!(key, Key::Enter | Key::Tab | Key::Backspace | Key::Escape) && !any_modifiers(modifiers) {
        return Some(match key {
            Key::Enter => b"\r".to_vec(),
            Key::Tab => b"\t".to_vec(),
            Key::Backspace => b"\x7f".to_vec(),
            Key::Escape => b"\x1b".to_vec(),
            _ => unreachable!(),
        });
    }

    build_sequence(key, modifiers, TermMode::NONE, true, false, None)
}

fn any_modifiers(modifiers: Modifiers) -> bool {
    modifiers.alt || modifiers.shift || control_modifier(modifiers)
}

fn printable_text(key: Key, modifiers: Modifiers) -> Option<String> {
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

fn build_sequence(
    key: Key,
    modifiers: Modifiers,
    mode: TermMode,
    pressed: bool,
    repeat: bool,
    text: Option<&str>,
) -> Option<Vec<u8>> {
    let kitty_sequence = mode.intersects(
        TermMode::REPORT_ALL_KEYS_AS_ESC
            | TermMode::DISAMBIGUATE_ESC_CODES
            | TermMode::REPORT_EVENT_TYPES
            | TermMode::REPORT_ALTERNATE_KEYS
            | TermMode::REPORT_ASSOCIATED_TEXT,
    );
    let kitty_encode_all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let kitty_event_type = mode.contains(TermMode::REPORT_EVENT_TYPES) && (repeat || !pressed);
    let mut sequence_modifiers = SequenceModifiers::from(modifiers);
    let associated_text = text.filter(|text| {
        pressed && mode.contains(TermMode::REPORT_ASSOCIATED_TEXT) && !text.is_empty() && !is_control_character(text)
    });

    let builder = SequenceBuilder {
        mode,
        kitty_sequence,
        kitty_encode_all,
        kitty_event_type,
        modifiers: sequence_modifiers,
    };

    let sequence_base = builder
        .try_build_named_kitty(key)
        .or_else(|| builder.try_build_named_normal(key, associated_text.is_some()))
        .or_else(|| builder.try_build_control_char_or_modifier(key, pressed, &mut sequence_modifiers))
        .or_else(|| builder.try_build_textual(key, text, associated_text));

    let SequenceBase { payload, terminator } = sequence_base?;
    let mut payload = format!("\x1b[{payload}");

    if kitty_event_type || !sequence_modifiers.is_empty() || associated_text.is_some() {
        let _ = write!(payload, ";{}", sequence_modifiers.encode_esc_sequence());
    }

    if kitty_event_type {
        payload.push(':');
        payload.push(match (pressed, repeat) {
            (_, true) => '2',
            (true, false) => '1',
            (false, false) => '3',
        });
    }

    if let Some(text) = associated_text {
        let mut codepoints = text.chars().map(u32::from);
        if let Some(codepoint) = codepoints.next() {
            let _ = write!(payload, ";{codepoint}");
        }
        for codepoint in codepoints {
            let _ = write!(payload, ":{codepoint}");
        }
    }

    payload.push(terminator.encode_esc_sequence());
    Some(payload.into_bytes())
}

struct SequenceBuilder {
    mode: TermMode,
    kitty_sequence: bool,
    kitty_encode_all: bool,
    kitty_event_type: bool,
    modifiers: SequenceModifiers,
}

impl SequenceBuilder {
    fn try_build_textual(&self, key: Key, text: Option<&str>, associated_text: Option<&str>) -> Option<SequenceBase> {
        let (true, Some(text)) = (self.kitty_sequence, text) else {
            return None;
        };

        if text.chars().count() == 1 {
            let ch = text.chars().next()?;
            let unshifted = if self.modifiers.contains(SequenceModifiers::SHIFT) && key.alpha_key() {
                ch.to_ascii_lowercase()
            } else {
                ch
            };
            let alternate = u32::from(ch);
            let unicode = u32::from(unshifted);
            let payload = if self.mode.contains(TermMode::REPORT_ALTERNATE_KEYS) && alternate != unicode {
                format!("{unicode}:{alternate}")
            } else {
                unicode.to_string()
            };
            return Some(SequenceBase::new(payload.into(), SequenceTerminator::Kitty));
        }

        if self.kitty_encode_all && associated_text.is_some() {
            return Some(SequenceBase::new("0".into(), SequenceTerminator::Kitty));
        }

        None
    }

    fn try_build_named_kitty(&self, key: Key) -> Option<SequenceBase> {
        if !self.kitty_sequence {
            return None;
        }

        let (base, terminator) = match key {
            Key::F3 => ("13", SequenceTerminator::Normal('~')),
            Key::F13 => ("57376", SequenceTerminator::Kitty),
            Key::F14 => ("57377", SequenceTerminator::Kitty),
            Key::F15 => ("57378", SequenceTerminator::Kitty),
            Key::F16 => ("57379", SequenceTerminator::Kitty),
            Key::F17 => ("57380", SequenceTerminator::Kitty),
            Key::F18 => ("57381", SequenceTerminator::Kitty),
            Key::F19 => ("57382", SequenceTerminator::Kitty),
            Key::F20 => ("57383", SequenceTerminator::Kitty),
            Key::F21 => ("57384", SequenceTerminator::Kitty),
            Key::F22 => ("57385", SequenceTerminator::Kitty),
            Key::F23 => ("57386", SequenceTerminator::Kitty),
            Key::F24 => ("57387", SequenceTerminator::Kitty),
            Key::F25 => ("57388", SequenceTerminator::Kitty),
            Key::F26 => ("57389", SequenceTerminator::Kitty),
            Key::F27 => ("57390", SequenceTerminator::Kitty),
            Key::F28 => ("57391", SequenceTerminator::Kitty),
            Key::F29 => ("57392", SequenceTerminator::Kitty),
            Key::F30 => ("57393", SequenceTerminator::Kitty),
            Key::F31 => ("57394", SequenceTerminator::Kitty),
            Key::F32 => ("57395", SequenceTerminator::Kitty),
            Key::F33 => ("57396", SequenceTerminator::Kitty),
            Key::F34 => ("57397", SequenceTerminator::Kitty),
            Key::F35 => ("57398", SequenceTerminator::Kitty),
            _ => return None,
        };

        Some(SequenceBase::new(base.into(), terminator))
    }

    fn try_build_named_normal(&self, key: Key, has_associated_text: bool) -> Option<SequenceBase> {
        let one_based = if self.modifiers.is_empty() && !self.kitty_event_type && !has_associated_text {
            ""
        } else {
            "1"
        };

        let (base, terminator) = match key {
            Key::PageUp => ("5", SequenceTerminator::Normal('~')),
            Key::PageDown => ("6", SequenceTerminator::Normal('~')),
            Key::Insert => ("2", SequenceTerminator::Normal('~')),
            Key::Delete => ("3", SequenceTerminator::Normal('~')),
            Key::Home => (one_based, SequenceTerminator::Normal('H')),
            Key::End => (one_based, SequenceTerminator::Normal('F')),
            Key::ArrowLeft => (one_based, SequenceTerminator::Normal('D')),
            Key::ArrowRight => (one_based, SequenceTerminator::Normal('C')),
            Key::ArrowUp => (one_based, SequenceTerminator::Normal('A')),
            Key::ArrowDown => (one_based, SequenceTerminator::Normal('B')),
            Key::F1 => (one_based, SequenceTerminator::Normal('P')),
            Key::F2 => (one_based, SequenceTerminator::Normal('Q')),
            Key::F3 => (one_based, SequenceTerminator::Normal('R')),
            Key::F4 => (one_based, SequenceTerminator::Normal('S')),
            Key::F5 => ("15", SequenceTerminator::Normal('~')),
            Key::F6 => ("17", SequenceTerminator::Normal('~')),
            Key::F7 => ("18", SequenceTerminator::Normal('~')),
            Key::F8 => ("19", SequenceTerminator::Normal('~')),
            Key::F9 => ("20", SequenceTerminator::Normal('~')),
            Key::F10 => ("21", SequenceTerminator::Normal('~')),
            Key::F11 => ("23", SequenceTerminator::Normal('~')),
            Key::F12 => ("24", SequenceTerminator::Normal('~')),
            Key::F13 => ("25", SequenceTerminator::Normal('~')),
            Key::F14 => ("26", SequenceTerminator::Normal('~')),
            Key::F15 => ("28", SequenceTerminator::Normal('~')),
            Key::F16 => ("29", SequenceTerminator::Normal('~')),
            Key::F17 => ("31", SequenceTerminator::Normal('~')),
            Key::F18 => ("32", SequenceTerminator::Normal('~')),
            Key::F19 => ("33", SequenceTerminator::Normal('~')),
            Key::F20 => ("34", SequenceTerminator::Normal('~')),
            _ => return None,
        };

        Some(SequenceBase::new(base.into(), terminator))
    }

    fn try_build_control_char_or_modifier(
        &self,
        key: Key,
        _pressed: bool,
        _modifiers: &mut SequenceModifiers,
    ) -> Option<SequenceBase> {
        if !self.kitty_encode_all && !self.kitty_sequence {
            return None;
        }

        let mut base = match key {
            Key::Tab => "9",
            Key::Enter => "13",
            Key::Escape => "27",
            Key::Space => "32",
            Key::Backspace => "127",
            _ => "",
        };

        if !self.kitty_encode_all && base.is_empty() {
            return None;
        }

        if base.is_empty() {
            base = match key {
                Key::Tab | Key::Enter | Key::Escape | Key::Space | Key::Backspace => base,
                _ => return None,
            };
        }

        Some(SequenceBase::new(base.into(), SequenceTerminator::Kitty))
    }
}

struct SequenceBase {
    payload: Cow<'static, str>,
    terminator: SequenceTerminator,
}

impl SequenceBase {
    fn new(payload: Cow<'static, str>, terminator: SequenceTerminator) -> Self {
        Self { payload, terminator }
    }
}

#[derive(Clone, Copy)]
enum SequenceTerminator {
    Normal(char),
    Kitty,
}

impl SequenceTerminator {
    fn encode_esc_sequence(self) -> char {
        match self {
            Self::Normal(character) => character,
            Self::Kitty => 'u',
        }
    }
}

#[derive(Clone, Copy)]
struct SequenceModifiers(u8);

impl SequenceModifiers {
    const SHIFT: Self = Self(0b0001);
    const ALT: Self = Self(0b0010);
    const CONTROL: Self = Self(0b0100);

    const fn empty() -> Self {
        Self(0)
    }

    const fn bits(self) -> u8 {
        self.0
    }

    const fn is_empty(self) -> bool {
        self.bits() == 0
    }

    fn set(&mut self, flag: Self, enabled: bool) {
        if enabled {
            self.0 |= flag.bits();
        } else {
            self.0 &= !flag.bits();
        }
    }

    const fn contains(self, flag: Self) -> bool {
        self.bits() & flag.bits() == flag.bits()
    }

    fn encode_esc_sequence(self) -> u8 {
        self.bits() + 1
    }
}

impl From<Modifiers> for SequenceModifiers {
    fn from(modifiers: Modifiers) -> Self {
        let mut result = Self::empty();
        result.set(Self::SHIFT, modifiers.shift);
        result.set(Self::ALT, modifiers.alt);
        result.set(Self::CONTROL, control_modifier(modifiers));
        result
    }
}

fn is_control_character(text: &str) -> bool {
    let Some(codepoint) = text.bytes().next() else {
        return false;
    };
    text.len() == 1 && (codepoint < 0x20 || (0x7f..=0x9f).contains(&codepoint))
}

trait KeyExt {
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

#[cfg(test)]
mod tests {
    use super::{
        GridPoint, PointerButtons, WheelAction, mouse_button_report, paste_bytes, translate_key_event, wheel_action,
    };
    use alacritty_terminal::term::TermMode;
    use egui::{Key, Modifiers, MouseWheelUnit, PointerButton, Vec2};

    #[test]
    fn app_cursor_mode_uses_ss3_sequences() {
        let translation =
            translate_key_event(Key::ArrowUp, true, false, Modifiers::NONE, TermMode::APP_CURSOR).expect("up");

        assert_eq!(translation.bytes, b"\x1bOA");
    }

    #[test]
    fn ctrl_letter_maps_to_control_code() {
        let translation = translate_key_event(Key::C, true, false, Modifiers::CTRL, TermMode::NONE).expect("ctrl-c");

        assert_eq!(translation.bytes, vec![3]);
    }

    #[test]
    fn kitty_escape_uses_csi_u_sequence() {
        let translation = translate_key_event(
            Key::Escape,
            true,
            false,
            Modifiers::NONE,
            TermMode::DISAMBIGUATE_ESC_CODES,
        )
        .expect("kitty escape");

        assert_eq!(translation.bytes, b"\x1b[27u");
    }

    #[test]
    fn bracketed_paste_filters_escape_and_ctrl_c() {
        let bytes = paste_bytes("hi\x1bthere\x03", TermMode::BRACKETED_PASTE, true);

        assert_eq!(bytes, b"\x1b[200~hithere\x1b[201~");
    }

    #[test]
    fn sgr_mouse_reports_button_release() {
        let bytes = mouse_button_report(
            PointerButton::Primary,
            false,
            Modifiers::NONE,
            TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE,
            GridPoint { line: 3, column: 8 },
        )
        .expect("mouse release");

        assert_eq!(bytes, b"\x1b[<0;9;4m");
    }

    #[test]
    fn wheel_uses_mouse_reporting_when_enabled() {
        let action = wheel_action(
            Vec2::new(0.0, 12.0),
            MouseWheelUnit::Point,
            Vec2::new(8.0, 12.0),
            Modifiers::NONE,
            TermMode::MOUSE_REPORT_CLICK,
            GridPoint { line: 1, column: 1 },
        )
        .expect("wheel action");

        match action {
            WheelAction::Pty(bytes) => assert_eq!(bytes, b"\x1b[M`\"\""),
            WheelAction::Scrollback(_) => panic!("expected PTY wheel reporting"),
        }
    }

    #[test]
    fn wheel_falls_back_to_scrollback_without_mouse_mode() {
        let action = wheel_action(
            Vec2::new(0.0, 32.0),
            MouseWheelUnit::Point,
            Vec2::new(8.0, 16.0),
            Modifiers::NONE,
            TermMode::NONE,
            GridPoint { line: 0, column: 0 },
        )
        .expect("scrollback");

        match action {
            WheelAction::Scrollback(lines) => assert_eq!(lines, 2),
            WheelAction::Pty(_) => panic!("expected scrollback"),
        }
    }

    #[test]
    fn mouse_motion_uses_drag_report_codes() {
        let bytes = super::mouse_motion_report(
            PointerButtons {
                primary: true,
                middle: false,
                secondary: false,
            },
            Modifiers::NONE,
            TermMode::MOUSE_DRAG,
            GridPoint { line: 0, column: 0 },
        )
        .expect("drag motion");

        assert_eq!(bytes, b"\x1b[M@!!");
    }
}
