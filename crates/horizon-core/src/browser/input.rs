//! Core input model for browser panels.
//!
//! The UI layer translates egui events into these platform-neutral values;
//! the driver thread translates them into CDP `Input.*` calls. CDP uses
//! Windows virtual-key codes and modifier bitmasks on every platform, so
//! the mapping table lives here in core.

use serde_json::{Value, json};

/// CDP modifier bitmask (matches `protocol::Input.Modifier`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
/// are delivered via [`BrowserInput::InsertText`] or the `char` key event
/// `text`, so letters/digits only appear as fallbacks for special layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

fn vk_for_char(c: char) -> u32 {
    match c {
        'a'..='z' | 'A'..='Z' => (c.to_ascii_uppercase() as u32) - ('A' as u32) + 0x41,
        '0'..='9' => (c as u32) - ('0' as u32) + 0x30,
        c => c as u32,
    }
}

/// One input event to deliver to the page. Coordinates are in CSS pixels
/// of the emulated viewport.
#[derive(Clone, Debug, PartialEq)]
pub enum BrowserInput {
    MouseMove { x: f64, y: f64, buttons: u32 },
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
    Wheel { x: f64, y: f64, delta_x: f64, delta_y: f64, modifiers: BrowserModifiers },
    /// Key press. `text` (for `char`-style delivery) is derived from
    /// `key`/`text` at the call site.
    KeyDown {
        key: BrowserKey,
        text: Option<String>,
        modifiers: BrowserModifiers,
    },
    KeyUp { key: BrowserKey, modifiers: BrowserModifiers },
    /// Paste-style raw text insertion (IME path; also used for pasting).
    InsertText { text: String },
}

impl BrowserInput {
    /// Map to a CDP `(method, params)` pair.
    #[must_use]
    pub fn cdp(self) -> (&'static str, Value) {
        match self {
            Self::MouseMove { x, y, buttons } => (
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mouseMoved",
                    "x": x,
                    "y": y,
                    "buttons": buttons,
                }),
            ),
            Self::MousePress { x, y, button, click_count, buttons, modifiers } => (
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mousePressed",
                    "x": x,
                    "y": y,
                    "button": button.cdp_name(),
                    "clickCount": click_count,
                    "buttons": buttons,
                    "modifiers": modifiers.cdp_bits(),
                }),
            ),
            Self::MouseRelease { x, y, button, click_count, buttons, modifiers } => (
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mouseReleased",
                    "x": x,
                    "y": y,
                    "button": button.cdp_name(),
                    "clickCount": click_count,
                    "buttons": buttons,
                    "modifiers": modifiers.cdp_bits(),
                }),
            ),
            Self::Wheel { x, y, delta_x, delta_y, modifiers } => (
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mouseWheel",
                    "x": x,
                    "y": y,
                    "deltaX": delta_x,
                    "deltaY": delta_y,
                    "modifiers": modifiers.cdp_bits(),
                }),
            ),
            Self::KeyDown { key, text, modifiers } => {
                if let Some(text) = text {
                    (
                        "Input.dispatchKeyEvent",
                        json!({
                            "type": "char",
                            "text": text,
                            "modifiers": modifiers.cdp_bits(),
                        }),
                    )
                } else {
                    (
                        "Input.dispatchKeyEvent",
                        json!({
                            "type": "rawKeyDown",
                            "windowsVirtualKeyCode": key.vk_code(),
                            "code": key.code_name(),
                            "modifiers": modifiers.cdp_bits(),
                        }),
                    )
                }
            }
            Self::KeyUp { key, modifiers } => (
                "Input.dispatchKeyEvent",
                json!({
                    "type": "keyUp",
                    "windowsVirtualKeyCode": key.vk_code(),
                    "code": key.code_name(),
                    "modifiers": modifiers.cdp_bits(),
                }),
            ),
            Self::InsertText { text } => (
                "Input.insertText",
                json!({ "text": text }),
            ),
        }
    }

    /// Whether this event changes page state (used for activity tracking).
    #[must_use]
    pub fn is_activity(&self) -> bool {
        matches!(
            self,
            Self::MousePress { .. } | Self::Wheel { .. } | Self::KeyDown { .. } | Self::InsertText { .. }
        )
    }
}

/// CSS `code` identifier for CDP key events (best-effort; Chrome accepts
/// missing `code` on rawKeyDown/keyUp in most paths).
impl BrowserKey {
    #[must_use]
    pub fn code_name(self) -> &'static str {
        match self {
            Self::ArrowUp => "ArrowUp",
            Self::ArrowDown => "ArrowDown",
            Self::ArrowLeft => "ArrowLeft",
            Self::ArrowRight => "ArrowRight",
            Self::Enter => "Enter",
            Self::Tab => "Tab",
            Self::Escape => "Escape",
            Self::Space => "Space",
            Self::Backspace => "Backspace",
            Self::Delete => "Delete",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "PageUp",
            Self::PageDown => "PageDown",
            Self::Insert => "Insert",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F6 => "F6",
            Self::F7 => "F7",
            Self::F8 => "F8",
            Self::F9 => "F9",
            Self::F10 => "F10",
            Self::F11 => "F11",
            Self::F12 => "F12",
            Self::F13 => "F13",
            Self::F14 => "F14",
            Self::F15 => "F15",
            Self::Char(c) => char_code_name(c),
        }
    }
}

/// Statically allocated code names for printable characters. The set is the
/// US-layout keyboard; anything else falls back to the generic name.
fn char_code_name(c: char) -> &'static str {
    match c {
        'a'..='z' | 'A'..='Z' => match c.to_ascii_uppercase() as u8 - b'A' {
            0 => "KeyA",
            1 => "KeyB",
            2 => "KeyC",
            3 => "KeyD",
            4 => "KeyE",
            5 => "KeyF",
            6 => "KeyG",
            7 => "KeyH",
            8 => "KeyI",
            9 => "KeyJ",
            10 => "KeyK",
            11 => "KeyL",
            12 => "KeyM",
            13 => "KeyN",
            14 => "KeyO",
            15 => "KeyP",
            16 => "KeyQ",
            17 => "KeyR",
            18 => "KeyS",
            19 => "KeyT",
            20 => "KeyU",
            21 => "KeyV",
            22 => "KeyW",
            23 => "KeyX",
            24 => "KeyY",
            25 => "KeyZ",
            _ => "Unidentified",
        },
        '0'..='9' => match c {
            '0' => "Digit0",
            '1' => "Digit1",
            '2' => "Digit2",
            '3' => "Digit3",
            '4' => "Digit4",
            '5' => "Digit5",
            '6' => "Digit6",
            '7' => "Digit7",
            '8' => "Digit8",
            _ => "Digit9",
        },
        _ => "Unidentified",
    }
}

#[cfg(test)]
mod tests {
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
    }

    #[test]
    fn char_key_uses_char_event() {
        let (method, params) = BrowserInput::KeyDown {
            key: BrowserKey::Char('h'),
            text: Some("h".to_string()),
            modifiers: BrowserModifiers::none(),
        }
        .cdp();
        assert_eq!(method, "Input.dispatchKeyEvent");
        assert_eq!(params["type"], "char");
        assert_eq!(params["text"], "h");
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
