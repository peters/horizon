use std::borrow::Cow;
use std::fmt::Write;

use alacritty_terminal::term::TermMode;
use egui::{Key, Modifiers};

use super::keyboard::{KeyExt, base_key_text, control_modifier, is_control_character};

#[derive(Clone, Copy)]
pub(super) struct SequenceRequest<'a> {
    pub(super) key: Key,
    pub(super) physical_key: Option<Key>,
    pub(super) key_without_modifiers_text: Option<&'a str>,
    pub(super) modifiers: Modifiers,
    pub(super) mode: TermMode,
    pub(super) pressed: bool,
    pub(super) repeat: bool,
    pub(super) text: Option<&'a str>,
}

pub(super) fn build_sequence(request: SequenceRequest<'_>) -> Option<Vec<u8>> {
    let SequenceRequest {
        key,
        physical_key,
        key_without_modifiers_text,
        modifiers,
        mode,
        pressed,
        repeat,
        text,
    } = request;
    let kitty_mode = KittySequenceMode::from(mode);
    // Alternate-key and associated-text flags refine CSI-u payloads, but
    // they should not force ordinary printable text off the legacy UTF-8 path.
    let kitty_event_type = mode.contains(TermMode::REPORT_EVENT_TYPES) && (repeat || !pressed);
    let sequence_modifiers = SequenceModifiers::from(modifiers);
    let associated_text = text.filter(|text| {
        pressed && mode.contains(TermMode::REPORT_ASSOCIATED_TEXT) && !text.is_empty() && !is_control_character(text)
    });

    let builder = SequenceBuilder {
        mode,
        kitty_mode,
        kitty_event_type,
        modifiers: sequence_modifiers,
    };

    let sequence_base = builder
        .try_build_named_kitty(key)
        .or_else(|| builder.try_build_named_normal(key, associated_text.is_some()))
        .or_else(|| builder.try_build_control_char_or_modifier(key))
        .or_else(|| builder.try_build_textual(key, physical_key, key_without_modifiers_text, text, associated_text));

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
    kitty_mode: KittySequenceMode,
    kitty_event_type: bool,
    modifiers: SequenceModifiers,
}

#[derive(Clone, Copy)]
enum KittySequenceMode {
    Disabled,
    Basic,
    EncodeAll,
}

impl KittySequenceMode {
    const fn enabled(self) -> bool {
        !matches!(self, Self::Disabled)
    }

    const fn encodes_all(self) -> bool {
        matches!(self, Self::EncodeAll)
    }
}

impl From<TermMode> for KittySequenceMode {
    fn from(mode: TermMode) -> Self {
        if mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC) {
            Self::EncodeAll
        } else if mode.intersects(
            TermMode::DISAMBIGUATE_ESC_CODES
                | TermMode::REPORT_EVENT_TYPES
                | TermMode::REPORT_ALTERNATE_KEYS
                | TermMode::REPORT_ASSOCIATED_TEXT,
        ) {
            Self::Basic
        } else {
            Self::Disabled
        }
    }
}

impl SequenceBuilder {
    fn try_build_textual(
        &self,
        key: Key,
        physical_key: Option<Key>,
        key_without_modifiers_text: Option<&str>,
        text: Option<&str>,
        associated_text: Option<&str>,
    ) -> Option<SequenceBase> {
        let (true, Some(text)) = (self.kitty_mode.encodes_all(), text) else {
            return None;
        };

        if text.chars().count() == 1 {
            let ch = text.chars().next()?;
            let unshifted = key_without_modifiers_text
                .and_then(single_char)
                .or_else(|| base_key_text(key, physical_key).as_deref().and_then(single_char))
                .unwrap_or_else(|| {
                    if self.modifiers.contains(SequenceModifiers::SHIFT) && key.alpha_key() {
                        ch.to_ascii_lowercase()
                    } else {
                        ch
                    }
                });
            let alternate = u32::from(ch);
            let unicode = u32::from(unshifted);
            let payload = if self.mode.contains(TermMode::REPORT_ALTERNATE_KEYS) {
                let base_layout = base_key_text(key, physical_key)
                    .as_deref()
                    .and_then(single_char)
                    .map(u32::from)
                    .filter(|base| *base != unicode && *base != alternate);
                format_textual_payload(unicode, (alternate != unicode).then_some(alternate), base_layout)
            } else {
                unicode.to_string()
            };
            return Some(SequenceBase::new(payload.into(), SequenceTerminator::Kitty));
        }

        if self.kitty_mode.encodes_all() && associated_text.is_some() {
            return Some(SequenceBase::new("0".into(), SequenceTerminator::Kitty));
        }

        None
    }

    fn try_build_named_kitty(&self, key: Key) -> Option<SequenceBase> {
        if !self.kitty_mode.enabled() {
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

        // In kitty disambiguate mode, Home (CSI H) and End (CSI F) clash
        // with cursor-movement commands (CUP / CPL). Always include the
        // explicit key number "1" so kitty-aware programs can distinguish
        // them from cursor-movement sequences.
        let one_based_or_kitty = if one_based.is_empty() && self.kitty_mode.enabled() {
            "1"
        } else {
            one_based
        };

        let (base, terminator) = match key {
            Key::PageUp => ("5", SequenceTerminator::Normal('~')),
            Key::PageDown => ("6", SequenceTerminator::Normal('~')),
            Key::Insert => ("2", SequenceTerminator::Normal('~')),
            Key::Delete => ("3", SequenceTerminator::Normal('~')),
            Key::Home => (one_based_or_kitty, SequenceTerminator::Normal('H')),
            Key::End => (one_based_or_kitty, SequenceTerminator::Normal('F')),
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

    fn try_build_control_char_or_modifier(&self, key: Key) -> Option<SequenceBase> {
        if !self.kitty_mode.enabled() {
            return None;
        }

        let base = match key {
            Key::Tab => "9",
            Key::Enter => "13",
            Key::Escape => "27",
            Key::Backspace => "127",
            // Space is printable text, so only force it onto CSI-u when the
            // app explicitly asked for every key via REPORT_ALL_KEYS_AS_ESC.
            Key::Space if self.kitty_mode.encodes_all() => "32",
            _ => return None,
        };

        Some(SequenceBase::new(base.into(), SequenceTerminator::Kitty))
    }
}

fn format_textual_payload(unicode: u32, alternate: Option<u32>, base_layout: Option<u32>) -> String {
    match (alternate, base_layout) {
        (None, None) => unicode.to_string(),
        (Some(alternate), None) => format!("{unicode}:{alternate}"),
        (None, Some(base_layout)) => format!("{unicode}::{base_layout}"),
        (Some(alternate), Some(base_layout)) => format!("{unicode}:{alternate}:{base_layout}"),
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

fn single_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let ch = chars.next()?;
    chars.next().is_none().then_some(ch)
}
