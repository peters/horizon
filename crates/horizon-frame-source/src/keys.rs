//! The names of the keys of the contract, in one table.
//!
//! A `key` message names a key: one of the modifiers, one of the named keys below,
//! or one character. Horizon shares the mapping of its own keys with the VNC
//! viewer, which turns an egui key into an X11 keysym
//! ([`horizon_vnc::keysym_for_char`]); this table turns that same keysym into the
//! name of the contract, so the two viewers agree on every key that both send, and
//! one keysym is spelled one way here.
use horizon_vnc::input::{
    XK_ALT_L, XK_BACKSPACE, XK_CONTROL_L, XK_DELETE, XK_DOWN, XK_END, XK_ESCAPE, XK_F1, XK_HOME, XK_INSERT, XK_LEFT,
    XK_PAGE_DOWN, XK_PAGE_UP, XK_RETURN, XK_RIGHT, XK_SHIFT_L, XK_SUPER_L, XK_TAB, XK_UP,
};
use horizon_vnc::{BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT};

/// One key that the contract names, and no character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedKey {
    Ctrl,
    Shift,
    Alt,
    Super,
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Space,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Menu,
    CapsLock,
    /// F1 to F12.
    Function(u8),
}

/// The keysym of the menu key and of the caps lock key, which no egui key of the
/// viewer produces and which the contract names all the same.
pub const XK_MENU: u32 = 0xff67;
pub const XK_CAPS_LOCK: u32 = 0xffe5;

/// Every name of the contract, with the keysym that the VNC viewer sends for it.
///
/// This is the one table of the names. A test reads it against the names of the
/// contract, so a name that is missing or spelled twice fails the build.
pub const NAMED: &[(&str, u32)] = &[
    ("ctrl", XK_CONTROL_L),
    ("shift", XK_SHIFT_L),
    ("alt", XK_ALT_L),
    ("super", XK_SUPER_L),
    ("enter", XK_RETURN),
    ("tab", XK_TAB),
    ("escape", XK_ESCAPE),
    ("backspace", XK_BACKSPACE),
    ("delete", XK_DELETE),
    ("space", 0x20),
    ("up", XK_UP),
    ("down", XK_DOWN),
    ("left", XK_LEFT),
    ("right", XK_RIGHT),
    ("home", XK_HOME),
    ("end", XK_END),
    ("pageup", XK_PAGE_UP),
    ("pagedown", XK_PAGE_DOWN),
    ("insert", XK_INSERT),
    ("menu", XK_MENU),
    ("capslock", XK_CAPS_LOCK),
];

/// One key of the contract: a name of the table, or one character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Named(NamedKey),
    Character(char),
}

impl Key {
    /// The name of this key, as the `key` member of an `input` message.
    #[must_use]
    pub fn name(self) -> String {
        match self {
            Self::Character(character) => character.to_string(),
            Self::Named(NamedKey::Function(number)) => format!("f{number}"),
            Self::Named(named) => NAMED
                .iter()
                .find(|(_, keysym)| named_of(*keysym) == Some(named))
                .map_or("?", |(name, _)| name)
                .to_owned(),
        }
    }

    /// The key of one keysym that the viewer sends. A keysym of a printable
    /// character is text and not one of these names, so it answers `None`.
    #[must_use]
    pub fn of_keysym(keysym: u32) -> Option<Self> {
        if let Some(named) = named_of(keysym) {
            return Some(Self::Named(named));
        }
        let number = keysym.checked_sub(XK_F1)?;
        u8::try_from(number + 1)
            .ok()
            .filter(|number| (1..=12).contains(number))
            .map(|number| Self::Named(NamedKey::Function(number)))
    }
}

/// The named key of one keysym, from the one table of the names.
#[must_use]
pub fn named_of(keysym: u32) -> Option<NamedKey> {
    Some(match keysym {
        XK_CONTROL_L => NamedKey::Ctrl,
        XK_SHIFT_L => NamedKey::Shift,
        XK_ALT_L => NamedKey::Alt,
        XK_SUPER_L => NamedKey::Super,
        XK_RETURN => NamedKey::Enter,
        XK_TAB => NamedKey::Tab,
        XK_ESCAPE => NamedKey::Escape,
        XK_BACKSPACE => NamedKey::Backspace,
        XK_DELETE => NamedKey::Delete,
        XK_UP => NamedKey::Up,
        XK_DOWN => NamedKey::Down,
        XK_LEFT => NamedKey::Left,
        XK_RIGHT => NamedKey::Right,
        XK_HOME => NamedKey::Home,
        XK_END => NamedKey::End,
        XK_PAGE_UP => NamedKey::PageUp,
        XK_PAGE_DOWN => NamedKey::PageDown,
        XK_INSERT => NamedKey::Insert,
        XK_MENU => NamedKey::Menu,
        XK_CAPS_LOCK => NamedKey::CapsLock,
        keysym if keysym == u32::from(b' ') => NamedKey::Space,
        _ => return None,
    })
}

/// One of the three buttons of the contract, from the RFB mask bit of the VNC
/// viewer's [`horizon_vnc::InputState`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

impl Button {
    /// The name of this button in a `button` message.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Middle => "middle",
            Self::Right => "right",
        }
    }

    /// The button of one RFB mask bit, or `None` for a bit that is no button of the
    /// contract (the wheel of RFB and the extra buttons of a pointer).
    #[must_use]
    pub const fn of_bit(bit: u8) -> Option<Self> {
        match bit {
            BUTTON_LEFT => Some(Self::Left),
            BUTTON_MIDDLE => Some(Self::Middle),
            BUTTON_RIGHT => Some(Self::Right),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_of_the_contract_are_one_table_without_a_double_name() {
        let mut names: Vec<&str> = NAMED.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        let mut unique = names.clone();
        unique.dedup();
        assert_eq!(names, unique, "one keysym and one name each");
        for name in [
            "ctrl",
            "shift",
            "alt",
            "super",
            "enter",
            "tab",
            "escape",
            "backspace",
            "delete",
            "space",
            "up",
            "down",
            "left",
            "right",
            "home",
            "end",
            "pageup",
            "pagedown",
            "insert",
            "menu",
            "capslock",
        ] {
            assert!(names.contains(&name), "{name} is a name of the contract");
        }
    }

    #[test]
    fn one_keysym_names_one_key_and_one_character_is_its_own_name() {
        assert_eq!(Key::of_keysym(XK_RETURN), Some(Key::Named(NamedKey::Enter)));
        assert_eq!(Key::of_keysym(XK_LEFT), Some(Key::Named(NamedKey::Left)));
        assert_eq!(Key::of_keysym(XK_F1 + 11), Some(Key::Named(NamedKey::Function(12))));
        assert_eq!(Key::of_keysym(XK_F1 + 12), None, "F13 is not a name of the contract");
        assert_eq!(Key::of_keysym(XK_CONTROL_L), Some(Key::Named(NamedKey::Ctrl)));
        assert_eq!(Key::of_keysym(u32::from(b'a')), None, "a letter is text, not a name");

        assert_eq!(Key::Named(NamedKey::PageUp).name(), "pageup");
        assert_eq!(Key::Named(NamedKey::Function(7)).name(), "f7");
        assert_eq!(Key::Named(NamedKey::CapsLock).name(), "capslock");
        assert_eq!(Key::Character('ø').name(), "ø");
        for (name, keysym) in NAMED {
            assert_eq!(Key::of_keysym(*keysym).map(Key::name).as_deref(), Some(*name), "{name}");
        }
    }

    #[test]
    fn the_three_buttons_are_the_three_bits_of_the_pointer() {
        assert_eq!(Button::of_bit(BUTTON_LEFT), Some(Button::Left));
        assert_eq!(Button::of_bit(BUTTON_MIDDLE), Some(Button::Middle));
        assert_eq!(Button::of_bit(BUTTON_RIGHT), Some(Button::Right));
        assert_eq!(Button::of_bit(8), None, "a wheel of RFB is not a button");
        assert_eq!((Button::Left.name(), Button::Right.name()), ("left", "right"));
    }
}
