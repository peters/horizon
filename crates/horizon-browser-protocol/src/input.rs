//! Backend-neutral input values shared by browser hosts and clients.

/// Keyboard and pointer modifiers active for an input event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)] // one bool per independently active modifier
pub struct BrowserModifiers {
    pub alt: bool,
    pub ctrl: bool,
    /// Command (macOS) / Windows key.
    pub meta: bool,
    pub shift: bool,
}

impl BrowserModifiers {
    #[must_use]
    pub const fn none() -> Self {
        Self {
            alt: false,
            ctrl: false,
            meta: false,
            shift: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserButton {
    Left,
    Middle,
    Right,
}

/// Keyboard keys with a distinct cross-backend representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum BrowserKey {
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Enter,
    Tab,
    Escape,
    Space,
    Backspace,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    /// Printable character (for example `a`, `7`, or `=`).
    Char(char),
}

impl BrowserKey {
    #[must_use]
    pub fn printable_char(self) -> Option<char> {
        match self {
            Self::Char(character) => Some(character),
            _ => None,
        }
    }

    /// Apply the shared US-layout shift mapping used by browser hosts.
    #[must_use]
    pub fn printable_char_with_shift(self, shift: bool) -> Option<char> {
        self.printable_char()
            .map(|character| if shift { shifted_char(character) } else { character })
    }
}

/// Editing operation attached to a synthetic key-down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserEditCommand {
    Copy,
    Cut,
    SelectAll,
}

/// One input event to deliver to the page. Coordinates are in CSS pixels of
/// the emulated viewport.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserInput {
    MouseMove {
        x: f64,
        y: f64,
        buttons: u32,
        modifiers: BrowserModifiers,
    },
    MousePress {
        x: f64,
        y: f64,
        button: BrowserButton,
        click_count: u32,
        buttons: u32,
        modifiers: BrowserModifiers,
    },
    MouseRelease {
        x: f64,
        y: f64,
        button: BrowserButton,
        click_count: u32,
        buttons: u32,
        modifiers: BrowserModifiers,
    },
    Wheel {
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
        modifiers: BrowserModifiers,
    },
    KeyDown {
        /// Physical key position retained independently from the logical key.
        physical_key: Option<BrowserKey>,
        key: BrowserKey,
        /// Text committed by the key, when any.
        text: Option<String>,
        modifiers: BrowserModifiers,
        repeat: bool,
        edit_command: Option<BrowserEditCommand>,
    },
    KeyUp {
        /// Physical key position retained from the matching key-down.
        physical_key: Option<BrowserKey>,
        key: BrowserKey,
        /// Effective key text captured on key-down for layout-correct key-up.
        text: Option<String>,
        modifiers: BrowserModifiers,
    },
    /// Paste-style raw text insertion, also used for IME input.
    InsertText { text: String },
}

/// Shift-adjusted character for the shared US-layout physical key map.
#[must_use]
pub const fn shifted_char(character: char) -> char {
    match character {
        'a'..='z' => character.to_ascii_uppercase(),
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        '`' => '~',
        other => other,
    }
}
