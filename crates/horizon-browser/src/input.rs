//! Core input model for browser panels.
//!
//! The UI layer translates egui events into these platform-neutral values;
//! the driver thread translates them into CDP `Input.*` calls. CDP uses
//! Windows virtual-key codes and modifier bitmasks on every platform, so
//! the mapping table lives here in core.

mod cdp;

/// CDP modifier bitmask (matches `protocol::Input.Modifier`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // the four CDP modifier bits
pub struct BrowserModifiers {
    pub alt: bool,
    pub ctrl: bool,
    /// Command (macOS) / Windows key.
    pub meta: bool,
    pub shift: bool,
}

impl BrowserModifiers {
    #[must_use]
    pub const fn cdp_bits(self) -> u32 {
        let mut bits = 0u32;
        if self.alt {
            bits |= 1;
        }
        if self.ctrl {
            bits |= 2;
        }
        if self.meta {
            bits |= 4;
        }
        if self.shift {
            bits |= 8;
        }
        bits
    }

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserButton {
    Left,
    Middle,
    Right,
}

impl BrowserButton {
    #[must_use]
    pub fn cdp_name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Middle => "middle",
            Self::Right => "right",
        }
    }
}

/// Keyboard keys with a distinct CDP representation. Printable characters
/// and unmodified Enter ride on a `keyDown` event carrying `text` (Chrome then
/// synthesizes the full keydown/keypress/input DOM sequence); other
/// non-printable keys use `rawKeyDown` with a virtual-key code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// Printable character (e.g. `a`, `7`, `=`).
    Char(char),
}

/// Chromium editing command attached to a synthetic key-down.
///
/// Clipboard pseudo-events need the explicit command because modifier bits
/// alone do not invoke the platform editing action in every headless Chrome
/// build (notably on macOS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserEditCommand {
    Copy,
    Cut,
    SelectAll,
}

impl BrowserEditCommand {
    const fn cdp_name(self) -> &'static str {
        match self {
            Self::Copy => "Copy",
            Self::Cut => "Cut",
            Self::SelectAll => "SelectAll",
        }
    }
}

impl BrowserKey {
    /// Windows virtual-key code, as CDP expects on all platforms.
    #[must_use]
    pub fn vk_code(self) -> u32 {
        match self {
            Self::ArrowUp => 0x26,
            Self::ArrowDown => 0x28,
            Self::ArrowRight => 0x27,
            Self::ArrowLeft => 0x25,
            Self::Enter => 0x0D,
            Self::Tab => 0x09,
            Self::Escape => 0x1B,
            Self::Space => 0x20,
            Self::Backspace => 0x08,
            Self::Delete => 0x2E,
            Self::Home => 0x24,
            Self::End => 0x23,
            Self::PageUp => 0x21,
            Self::PageDown => 0x22,
            Self::Insert => 0x2D,
            Self::F1 => 0x70,
            Self::F2 => 0x71,
            Self::F3 => 0x72,
            Self::F4 => 0x73,
            Self::F5 => 0x74,
            Self::F6 => 0x75,
            Self::F7 => 0x76,
            Self::F8 => 0x77,
            Self::F9 => 0x78,
            Self::F10 => 0x79,
            Self::F11 => 0x7A,
            Self::F12 => 0x7B,
            Self::F13 => 0x7C,
            Self::F14 => 0x7D,
            Self::F15 => 0x7E,
            Self::Char(c) => vk_for_char(c),
        }
    }

    #[must_use]
    pub fn printable_char(self) -> Option<char> {
        match self {
            Self::Char(c) => Some(c),
            _ => None,
        }
    }

    #[must_use]
    pub fn printable_char_with_shift(self, shift: bool) -> Option<char> {
        self.printable_char().map(|c| if shift { shifted_char(c) } else { c })
    }
}

fn vk_for_char(c: char) -> u32 {
    match c {
        'a'..='z' | 'A'..='Z' => (c.to_ascii_uppercase() as u32) - ('A' as u32) + 0x41,
        '0'..='9' => (c as u32) - ('0' as u32) + 0x30,
        ';' => 0xBA,
        '=' => 0xBB,
        ',' => 0xBC,
        '-' => 0xBD,
        '.' => 0xBE,
        '/' => 0xBF,
        '`' => 0xC0,
        '[' => 0xDB,
        '\\' => 0xDC,
        ']' => 0xDD,
        '\'' => 0xDE,
        c => c as u32,
    }
}

/// One input event to deliver to the page. Coordinates are in CSS pixels
/// of the emulated viewport.
#[derive(Clone, Debug, PartialEq)]
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
    /// Key press. `text` (for printable keys) makes Chrome run the normal
    /// keydown→keypress→input pipeline instead of a bare text insertion.
    /// `repeat` marks auto-repeat of a held key.
    KeyDown {
        /// Physical key position used for DOM `code` and Windows VK. The
        /// logical `key` and committed `text` still drive DOM `key`/input.
        physical_key: Option<BrowserKey>,
        key: BrowserKey,
        text: Option<String>,
        modifiers: BrowserModifiers,
        repeat: bool,
        edit_command: Option<BrowserEditCommand>,
    },
    KeyUp {
        /// Physical key position retained from the matching key-down.
        physical_key: Option<BrowserKey>,
        key: BrowserKey,
        /// Effective DOM key captured on key-down for layout-correct key-up.
        text: Option<String>,
        modifiers: BrowserModifiers,
    },
    /// Paste-style raw text insertion (IME path; also used for pasting).
    InsertText { text: String },
}

impl BrowserInput {
    /// Whether this event changes page state (used for activity tracking).
    #[must_use]
    pub fn is_activity(&self) -> bool {
        matches!(
            self,
            Self::MousePress { .. } | Self::Wheel { .. } | Self::KeyDown { .. } | Self::InsertText { .. }
        )
    }

    /// Whether this key-down asks Chrome to copy the current page selection.
    /// The driver snapshots that selection before dispatch so the UI can
    /// bridge headless Chrome's private clipboard to the host clipboard.
    #[must_use]
    pub const fn copies_selection(&self) -> bool {
        matches!(
            self,
            Self::KeyDown {
                edit_command: Some(BrowserEditCommand::Copy | BrowserEditCommand::Cut),
                ..
            }
        )
    }
}

/// Shift-adjusted character for the US-layout physical key map.
#[must_use]
pub const fn shifted_char(c: char) -> char {
    match c {
        'a'..='z' => c.to_ascii_uppercase(),
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn modifier_bits_match_cdp() {
        assert_eq!(BrowserModifiers::none().cdp_bits(), 0);
        let mods = BrowserModifiers {
            alt: true,
            ctrl: true,
            meta: false,
            shift: true,
        };
        assert_eq!(mods.cdp_bits(), 1 | 2 | 8);
    }

    #[test]
    fn vk_codes() {
        assert_eq!(BrowserKey::Enter.vk_code(), 0x0D);
        assert_eq!(BrowserKey::F5.vk_code(), 0x74);
        assert_eq!(BrowserKey::ArrowLeft.vk_code(), 0x25);
        assert_eq!(BrowserKey::Char('a').vk_code(), 0x41);
        assert_eq!(BrowserKey::Char('7').vk_code(), 0x37);
    }

    #[test]
    fn mouse_params_shape() {
        let (method, params) = BrowserInput::MousePress {
            x: 10.0,
            y: 20.0,
            button: BrowserButton::Left,
            click_count: 1,
            buttons: 1,
            modifiers: BrowserModifiers::none(),
        }
        .cdp();
        assert_eq!(method, "Input.dispatchMouseEvent");
        assert_eq!(params["type"], "mousePressed");
        assert_eq!(params["button"], "left");
        assert_eq!(params["x"], 10.0);

        let (_, moved) = BrowserInput::MouseMove {
            x: 30.0,
            y: 40.0,
            buttons: 1,
            modifiers: BrowserModifiers {
                shift: true,
                ..BrowserModifiers::none()
            },
        }
        .cdp();
        assert_eq!(moved["type"], "mouseMoved");
        assert_eq!(moved["modifiers"], 8);
    }

    #[test]
    fn char_key_sends_keydown_with_text() {
        let (method, params) = BrowserInput::KeyDown {
            physical_key: None,
            key: BrowserKey::Char('h'),
            text: Some("h".to_string()),
            modifiers: BrowserModifiers::none(),
            repeat: false,
            edit_command: None,
        }
        .cdp();
        assert_eq!(method, "Input.dispatchKeyEvent");
        assert_eq!(params["type"], "keyDown");
        assert_eq!(params["text"], "h");
        assert_eq!(params["key"], "h");
        assert_eq!(params["windowsVirtualKeyCode"], 0x48);
        assert_eq!(params["autoRepeat"], false);
    }

    #[test]
    fn non_printable_key_sends_raw_keydown() {
        let (_, params) = BrowserInput::KeyDown {
            physical_key: None,
            key: BrowserKey::Backspace,
            text: None,
            modifiers: BrowserModifiers::none(),
            repeat: true,
            edit_command: None,
        }
        .cdp();
        assert_eq!(params["type"], "rawKeyDown");
        assert!(params.get("text").is_none());
        assert_eq!(params["autoRepeat"], true);
    }

    #[test]
    fn unmodified_enter_sends_text_for_chromiums_keypress_pipeline() {
        let (_, params) = BrowserInput::KeyDown {
            physical_key: None,
            key: BrowserKey::Enter,
            text: None,
            modifiers: BrowserModifiers::none(),
            repeat: false,
            edit_command: None,
        }
        .cdp();

        assert_eq!(params["type"], "keyDown");
        assert_eq!(params["text"], "\r");
        assert_eq!(params["key"], "Enter");
    }

    #[test]
    fn shortcut_enter_remains_non_textual() {
        for modifiers in [
            BrowserModifiers {
                ctrl: true,
                ..BrowserModifiers::none()
            },
            BrowserModifiers {
                alt: true,
                ..BrowserModifiers::none()
            },
            BrowserModifiers {
                meta: true,
                ..BrowserModifiers::none()
            },
        ] {
            let (_, params) = BrowserInput::KeyDown {
                physical_key: None,
                key: BrowserKey::Enter,
                text: None,
                modifiers,
                repeat: false,
                edit_command: None,
            }
            .cdp();

            assert_eq!(params["type"], "rawKeyDown");
            assert!(params.get("text").is_none());
        }
    }

    #[test]
    fn shifted_keys_report_effective_dom_key_and_physical_code() {
        let modifiers = BrowserModifiers {
            shift: true,
            ..BrowserModifiers::none()
        };
        let (_, params) = BrowserInput::KeyDown {
            physical_key: None,
            key: BrowserKey::Char('a'),
            text: Some("A".to_string()),
            modifiers,
            repeat: false,
            edit_command: None,
        }
        .cdp();
        assert_eq!(params["key"], "A");
        assert_eq!(params["code"], "KeyA");
        assert_eq!(params["windowsVirtualKeyCode"], 0x41);

        let (_, shortcut) = BrowserInput::KeyDown {
            physical_key: None,
            key: BrowserKey::Char('1'),
            text: None,
            modifiers: BrowserModifiers {
                ctrl: true,
                ..modifiers
            },
            repeat: false,
            edit_command: None,
        }
        .cdp();
        assert_eq!(shortcut["key"], "!");
        assert_eq!(shortcut["code"], "Digit1");
    }

    #[test]
    fn physical_key_drives_code_and_virtual_key_without_changing_layout_text() {
        let (_, key_down) = BrowserInput::KeyDown {
            physical_key: Some(BrowserKey::Char('q')),
            key: BrowserKey::Char('a'),
            text: Some("a".to_string()),
            modifiers: BrowserModifiers::none(),
            repeat: false,
            edit_command: None,
        }
        .cdp();
        let (_, key_up) = BrowserInput::KeyUp {
            physical_key: Some(BrowserKey::Char('q')),
            key: BrowserKey::Char('a'),
            text: Some("a".to_string()),
            modifiers: BrowserModifiers::none(),
        }
        .cdp();

        for params in [&key_down, &key_up] {
            assert_eq!(params["key"], "a");
            assert_eq!(params["code"], "KeyQ");
            assert_eq!(params["windowsVirtualKeyCode"], 0x51);
        }
    }

    #[test]
    fn editing_keydowns_carry_chromium_edit_commands() {
        let (_, params) = BrowserInput::KeyDown {
            physical_key: None,
            key: BrowserKey::Char('x'),
            text: None,
            modifiers: BrowserModifiers {
                meta: true,
                ..BrowserModifiers::none()
            },
            repeat: false,
            edit_command: Some(BrowserEditCommand::Cut),
        }
        .cdp();

        assert_eq!(params["type"], "rawKeyDown");
        assert_eq!(params["commands"], json!(["Cut"]));

        let (_, select_all) = BrowserInput::KeyDown {
            physical_key: None,
            key: BrowserKey::Char('a'),
            text: None,
            modifiers: BrowserModifiers {
                meta: true,
                ..BrowserModifiers::none()
            },
            repeat: false,
            edit_command: Some(BrowserEditCommand::SelectAll),
        }
        .cdp();
        assert_eq!(select_all["commands"], json!(["SelectAll"]));
    }

    #[test]
    fn only_copying_edit_commands_request_a_selection_snapshot() {
        let cut = BrowserInput::KeyDown {
            physical_key: None,
            key: BrowserKey::Char('x'),
            text: None,
            modifiers: BrowserModifiers::none(),
            repeat: false,
            edit_command: Some(BrowserEditCommand::Cut),
        };
        let select_all = BrowserInput::KeyDown {
            physical_key: None,
            key: BrowserKey::Char('a'),
            text: None,
            modifiers: BrowserModifiers::none(),
            repeat: false,
            edit_command: Some(BrowserEditCommand::SelectAll),
        };

        assert!(cut.copies_selection());
        assert!(!select_all.copies_selection());
    }

    #[test]
    fn punctuation_uses_oem_virtual_keys_and_css_codes() {
        let cases = [
            (';', "Semicolon", 0xBA),
            ('=', "Equal", 0xBB),
            (',', "Comma", 0xBC),
            ('-', "Minus", 0xBD),
            ('.', "Period", 0xBE),
            ('/', "Slash", 0xBF),
            ('`', "Backquote", 0xC0),
            ('[', "BracketLeft", 0xDB),
            ('\\', "Backslash", 0xDC),
            (']', "BracketRight", 0xDD),
            ('\'', "Quote", 0xDE),
        ];
        for (character, code, virtual_key) in cases {
            let key = BrowserKey::Char(character);
            assert_eq!(key.code_name(), code);
            assert_eq!(key.vk_code(), virtual_key);
            assert_eq!(key.dom_key_name(), character.to_string());
        }
    }

    #[test]
    fn wheel_params_shape() {
        let (method, params) = BrowserInput::Wheel {
            x: 5.0,
            y: 6.0,
            delta_x: 0.0,
            delta_y: 120.0,
            modifiers: BrowserModifiers::none(),
        }
        .cdp();
        assert_eq!(method, "Input.dispatchMouseEvent");
        assert_eq!(params["type"], "mouseWheel");
        assert_eq!(params["deltaY"], 120.0);
    }

    #[test]
    fn insert_text_shape() {
        let (method, params) = BrowserInput::InsertText {
            text: "hello".to_string(),
        }
        .cdp();
        assert_eq!(method, "Input.insertText");
        assert_eq!(params["text"], "hello");
    }
}
