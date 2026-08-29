//! Chromium CDP input serialization.

use serde_json::{Value, json};

use super::{BrowserEditCommand, BrowserInput, BrowserKey, BrowserModifiers};

impl BrowserInput {
    /// Map to a CDP `(method, params)` pair.
    #[must_use]
    pub fn cdp(self) -> (&'static str, Value) {
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
                    "modifiers": modifiers.cdp_bits(),
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
                    "button": button.cdp_name(),
                    "clickCount": click_count,
                    "buttons": buttons,
                    "modifiers": modifiers.cdp_bits(),
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
                    "button": button.cdp_name(),
                    "clickCount": click_count,
                    "buttons": buttons,
                    "modifiers": modifiers.cdp_bits(),
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
                    "modifiers": modifiers.cdp_bits(),
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
        "code": hardware_key.code_name(),
        "windowsVirtualKeyCode": hardware_key.vk_code(),
        "autoRepeat": repeat,
        "modifiers": modifiers.cdp_bits(),
    });
    if let Some(text) = text {
        params["text"] = json!(text);
    }
    if let Some(command) = edit_command {
        params["commands"] = json!([command.cdp_name()]);
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
            "windowsVirtualKeyCode": hardware_key.vk_code(),
            "code": hardware_key.code_name(),
            "modifiers": modifiers.cdp_bits(),
        }),
    )
}

impl BrowserKey {
    /// DOM `KeyboardEvent.key` value for CDP key-down events.
    #[must_use]
    pub fn dom_key_name(self) -> &'static str {
        match self {
            Self::ArrowUp => "ArrowUp",
            Self::ArrowDown => "ArrowDown",
            Self::ArrowLeft => "ArrowLeft",
            Self::ArrowRight => "ArrowRight",
            Self::Enter => "Enter",
            Self::Tab => "Tab",
            Self::Escape => "Escape",
            Self::Space => " ",
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
            Self::Char(c) => char_dom_key_name(c),
        }
    }

    /// CSS `code` identifier for CDP key events.
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
    std::borrow::Cow::Borrowed(key.dom_key_name())
}
