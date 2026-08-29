//! Chromium CDP input serialization.

use serde_json::{Value, json};

use super::{BrowserButton, BrowserEditCommand, BrowserInput, BrowserKey, BrowserModifiers};
use BrowserKey::{
    ArrowDown, ArrowLeft, ArrowRight, ArrowUp, Backspace, Char, Delete, End, Enter, Escape, F1, F2, F3, F4, F5, F6, F7,
    F8, F9, F10, F11, F12, F13, F14, F15, Home, Insert, PageDown, PageUp, Space, Tab,
};

pub(crate) trait BrowserInputCdpExt {
    /// Map to a CDP `(method, params)` pair.
    #[must_use]
    fn cdp(self) -> (&'static str, Value);

    /// Whether this key-down asks Chromium to copy the page selection.
    #[must_use]
    fn copies_selection(&self) -> bool;
}

impl BrowserInputCdpExt for BrowserInput {
    fn cdp(self) -> (&'static str, Value) {
        match self {
            Self::MouseMove {
                x,
                y,
                buttons,
                modifiers,
            } => (
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mouseMoved",
                    "x": x,
                    "y": y,
                    "buttons": buttons,
                    "modifiers": modifier_bits(modifiers),
                }),
            ),
            Self::MousePress {
                x,
                y,
                button,
                click_count,
                buttons,
                modifiers,
            } => (
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mousePressed",
                    "x": x,
                    "y": y,
                    "button": button_name(button),
                    "clickCount": click_count,
                    "buttons": buttons,
                    "modifiers": modifier_bits(modifiers),
                }),
            ),
            Self::MouseRelease {
                x,
                y,
                button,
                click_count,
                buttons,
                modifiers,
            } => (
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mouseReleased",
                    "x": x,
                    "y": y,
                    "button": button_name(button),
                    "clickCount": click_count,
                    "buttons": buttons,
                    "modifiers": modifier_bits(modifiers),
                }),
            ),
            Self::Wheel {
                x,
                y,
                delta_x,
                delta_y,
                modifiers,
            } => (
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mouseWheel",
                    "x": x,
                    "y": y,
                    "deltaX": delta_x,
                    "deltaY": delta_y,
                    "modifiers": modifier_bits(modifiers),
                }),
            ),
            Self::KeyDown {
                physical_key,
                key,
                text,
                modifiers,
                repeat,
                edit_command,
            } => key_down_cdp(physical_key, key, text, modifiers, repeat, edit_command),
            Self::KeyUp {
                physical_key,
                key,
                text,
                modifiers,
            } => key_up_cdp(physical_key, key, text.as_deref(), modifiers),
            Self::InsertText { text } => ("Input.insertText", json!({ "text": text })),
        }
    }

    fn copies_selection(&self) -> bool {
        matches!(
            self,
            Self::KeyDown {
                edit_command: Some(BrowserEditCommand::Copy | BrowserEditCommand::Cut),
                ..
            }
        )
    }
}

const fn modifier_bits(modifiers: BrowserModifiers) -> u32 {
    let mut bits = 0;
    if modifiers.alt {
        bits |= 1;
    }
    if modifiers.ctrl {
        bits |= 2;
    }
    if modifiers.meta {
        bits |= 4;
    }
    if modifiers.shift {
        bits |= 8;
    }
    bits
}

const fn button_name(button: BrowserButton) -> &'static str {
    match button {
        BrowserButton::Left => "left",
        BrowserButton::Middle => "middle",
        BrowserButton::Right => "right",
    }
}

const fn edit_command_name(command: BrowserEditCommand) -> &'static str {
    match command {
        BrowserEditCommand::Copy => "Copy",
        BrowserEditCommand::Cut => "Cut",
        BrowserEditCommand::SelectAll => "SelectAll",
    }
}

fn key_down_cdp(
    physical_key: Option<BrowserKey>,
    key: BrowserKey,
    text: Option<String>,
    modifiers: BrowserModifiers,
    repeat: bool,
    edit_command: Option<BrowserEditCommand>,
) -> (&'static str, Value) {
    let text = text.or_else(|| {
        (key == BrowserKey::Enter && !modifiers.ctrl && !modifiers.alt && !modifiers.meta).then(|| "\r".to_string())
    });
    // Printable keys (`text` present) use `keyDown` so Chrome runs the normal
    // keydown-to-input pipeline; other keys use `rawKeyDown`.
    let ty = if text.is_some() { "keyDown" } else { "rawKeyDown" };
    let dom_key = dispatch_dom_key(key, text.as_deref(), modifiers.shift);
    let hardware_key = physical_key.unwrap_or(key);
    let mut params = json!({
        "type": ty,
        "key": dom_key,
        "code": code_name(hardware_key),
        "windowsVirtualKeyCode": vk_code(hardware_key),
        "autoRepeat": repeat,
        "modifiers": modifier_bits(modifiers),
    });
    if let Some(text) = text {
        params["text"] = json!(text);
    }
    if let Some(command) = edit_command {
        params["commands"] = json!([edit_command_name(command)]);
    }
    ("Input.dispatchKeyEvent", params)
}

fn key_up_cdp(
    physical_key: Option<BrowserKey>,
    key: BrowserKey,
    text: Option<&str>,
    modifiers: BrowserModifiers,
) -> (&'static str, Value) {
    let dom_key = dispatch_dom_key(key, text, modifiers.shift);
    let hardware_key = physical_key.unwrap_or(key);
    (
        "Input.dispatchKeyEvent",
        json!({
            "type": "keyUp",
            "key": dom_key,
            "windowsVirtualKeyCode": vk_code(hardware_key),
            "code": code_name(hardware_key),
            "modifiers": modifier_bits(modifiers),
        }),
    )
}

fn dom_key_name(key: BrowserKey) -> &'static str {
    match key {
        ArrowUp => "ArrowUp",
        ArrowDown => "ArrowDown",
        ArrowLeft => "ArrowLeft",
        ArrowRight => "ArrowRight",
        Enter => "Enter",
        Tab => "Tab",
        Escape => "Escape",
        Space => " ",
        Backspace => "Backspace",
        Delete => "Delete",
        Home => "Home",
        End => "End",
        PageUp => "PageUp",
        PageDown => "PageDown",
        Insert => "Insert",
        F1 => "F1",
        F2 => "F2",
        F3 => "F3",
        F4 => "F4",
        F5 => "F5",
        F6 => "F6",
        F7 => "F7",
        F8 => "F8",
        F9 => "F9",
        F10 => "F10",
        F11 => "F11",
        F12 => "F12",
        F13 => "F13",
        F14 => "F14",
        F15 => "F15",
        Char(character) => char_dom_key_name(character),
    }
}

fn code_name(key: BrowserKey) -> &'static str {
    match key {
        ArrowUp => "ArrowUp",
        ArrowDown => "ArrowDown",
        ArrowLeft => "ArrowLeft",
        ArrowRight => "ArrowRight",
        Enter => "Enter",
        Tab => "Tab",
        Escape => "Escape",
        Space => "Space",
        Backspace => "Backspace",
        Delete => "Delete",
        Home => "Home",
        End => "End",
        PageUp => "PageUp",
        PageDown => "PageDown",
        Insert => "Insert",
        F1 => "F1",
        F2 => "F2",
        F3 => "F3",
        F4 => "F4",
        F5 => "F5",
        F6 => "F6",
        F7 => "F7",
        F8 => "F8",
        F9 => "F9",
        F10 => "F10",
        F11 => "F11",
        F12 => "F12",
        F13 => "F13",
        F14 => "F14",
        F15 => "F15",
        Char(character) => char_code_name(character),
    }
}

fn vk_code(key: BrowserKey) -> u32 {
    match key {
        ArrowUp => 0x26,
        ArrowDown => 0x28,
        ArrowRight => 0x27,
        ArrowLeft => 0x25,
        Enter => 0x0D,
        Tab => 0x09,
        Escape => 0x1B,
        Space => 0x20,
        Backspace => 0x08,
        Delete => 0x2E,
        Home => 0x24,
        End => 0x23,
        PageUp => 0x21,
        PageDown => 0x22,
        Insert => 0x2D,
        F1 => 0x70,
        F2 => 0x71,
        F3 => 0x72,
        F4 => 0x73,
        F5 => 0x74,
        F6 => 0x75,
        F7 => 0x76,
        F8 => 0x77,
        F9 => 0x78,
        F10 => 0x79,
        F11 => 0x7A,
        F12 => 0x7B,
        F13 => 0x7C,
        F14 => 0x7D,
        F15 => 0x7E,
        Char(character) => vk_for_char(character),
    }
}

fn vk_for_char(character: char) -> u32 {
    match character {
        'a'..='z' | 'A'..='Z' => (character.to_ascii_uppercase() as u32) - ('A' as u32) + 0x41,
        '0'..='9' => (character as u32) - ('0' as u32) + 0x30,
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
        other => other as u32,
    }
}

fn char_dom_key_name(c: char) -> &'static str {
    match c {
        'a'..='z' | 'A'..='Z' => match c.to_ascii_uppercase() as u8 - b'A' {
            0 => "a",
            1 => "b",
            2 => "c",
            3 => "d",
            4 => "e",
            5 => "f",
            6 => "g",
            7 => "h",
            8 => "i",
            9 => "j",
            10 => "k",
            11 => "l",
            12 => "m",
            13 => "n",
            14 => "o",
            15 => "p",
            16 => "q",
            17 => "r",
            18 => "s",
            19 => "t",
            20 => "u",
            21 => "v",
            22 => "w",
            23 => "x",
            24 => "y",
            25 => "z",
            _ => "Unidentified",
        },
        '0'..='9' => match c {
            '0' => "0",
            '1' => "1",
            '2' => "2",
            '3' => "3",
            '4' => "4",
            '5' => "5",
            '6' => "6",
            '7' => "7",
            '8' => "8",
            _ => "9",
        },
        ';' => ";",
        '=' => "=",
        ',' => ",",
        '-' => "-",
        '.' => ".",
        '/' => "/",
        '`' => "`",
        '[' => "[",
        '\\' => "\\",
        ']' => "]",
        '\'' => "'",
        _ => "Unidentified",
    }
}

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
        ';' => "Semicolon",
        '=' => "Equal",
        ',' => "Comma",
        '-' => "Minus",
        '.' => "Period",
        '/' => "Slash",
        '`' => "Backquote",
        '[' => "BracketLeft",
        '\\' => "Backslash",
        ']' => "BracketRight",
        '\'' => "Quote",
        _ => "Unidentified",
    }
}

fn dispatch_dom_key(key: BrowserKey, text: Option<&str>, shift: bool) -> std::borrow::Cow<'_, str> {
    if let Some(text) = text {
        let mut characters = text.chars();
        if characters.next().is_some_and(|character| !character.is_control()) && characters.next().is_none() {
            return std::borrow::Cow::Borrowed(text);
        }
    }
    if shift && let Some(character) = key.printable_char_with_shift(true) {
        return std::borrow::Cow::Owned(character.to_string());
    }
    std::borrow::Cow::Borrowed(dom_key_name(key))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn modifier_bits_match_cdp() {
        assert_eq!(modifier_bits(BrowserModifiers::none()), 0);
        assert_eq!(
            modifier_bits(BrowserModifiers {
                alt: true,
                ctrl: true,
                meta: false,
                shift: true,
            }),
            1 | 2 | 8
        );
    }

    #[test]
    fn virtual_key_codes_match_cdp() {
        assert_eq!(vk_code(BrowserKey::Enter), 0x0D);
        assert_eq!(vk_code(BrowserKey::F5), 0x74);
        assert_eq!(vk_code(BrowserKey::ArrowLeft), 0x25);
        assert_eq!(vk_code(BrowserKey::Char('a')), 0x41);
        assert_eq!(vk_code(BrowserKey::Char('7')), 0x37);
    }

    #[test]
    fn mouse_params_have_cdp_shape() {
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
    fn character_key_sends_keydown_with_text() {
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
    fn physical_key_does_not_change_layout_text() {
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
    fn editing_keydowns_carry_chromium_commands() {
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
    fn only_copying_commands_request_a_selection_snapshot() {
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
            assert_eq!(code_name(key), code);
            assert_eq!(vk_code(key), virtual_key);
            assert_eq!(dom_key_name(key), character.to_string());
        }
    }

    #[test]
    fn wheel_params_have_cdp_shape() {
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
    fn insert_text_has_cdp_shape() {
        let (method, params) = BrowserInput::InsertText {
            text: "hello".to_string(),
        }
        .cdp();
        assert_eq!(method, "Input.insertText");
        assert_eq!(params["text"], "hello");
    }
}
