use std::fmt;

use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ShortcutModifiers(u8);

impl ShortcutModifiers {
    const ALT_BIT: u8 = 1 << 0;
    const CTRL_BIT: u8 = 1 << 1;
    const SHIFT_BIT: u8 = 1 << 2;
    const COMMAND_BIT: u8 = 1 << 3;
    const MAC_CMD_BIT: u8 = 1 << 4;

    pub const NONE: Self = Self(0);
    pub const ALT: Self = Self(Self::ALT_BIT);
    pub const CTRL: Self = Self(Self::CTRL_BIT);
    pub const SHIFT: Self = Self(Self::SHIFT_BIT);
    pub const PRIMARY: Self = Self(Self::COMMAND_BIT);
    pub const MAC_CMD: Self = Self(Self::MAC_CMD_BIT);

    #[must_use]
    pub const fn plus(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    #[must_use]
    pub const fn alt(self) -> bool {
        self.contains(Self::ALT)
    }

    #[must_use]
    pub const fn ctrl(self) -> bool {
        self.contains(Self::CTRL)
    }

    #[must_use]
    pub const fn shift(self) -> bool {
        self.contains(Self::SHIFT)
    }

    #[must_use]
    pub const fn command(self) -> bool {
        self.contains(Self::PRIMARY)
    }

    #[must_use]
    pub const fn mac_cmd(self) -> bool {
        self.contains(Self::MAC_CMD)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ShortcutKey {
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Escape,
    Enter,
    Tab,
    Comma,
    Minus,
    Plus,
    Digit(u8),
    Letter(char),
    Function(u8),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ShortcutBinding {
    pub modifiers: ShortcutModifiers,
    pub key: ShortcutKey,
}

impl ShortcutBinding {
    #[must_use]
    pub const fn new(modifiers: ShortcutModifiers, key: ShortcutKey) -> Self {
        Self { modifiers, key }
    }

    /// Parse a keyboard shortcut string such as `Ctrl+K` or `F11`.
    ///
    /// # Errors
    ///
    /// Returns an error if the shortcut contains unsupported modifiers,
    /// unsupported key names, duplicate modifiers, or empty components.
    pub fn parse(value: &str) -> Result<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(Error::Config("shortcut cannot be empty".to_string()));
        }

        let parts: Vec<&str> = trimmed.split('+').map(str::trim).collect();
        if parts.iter().any(|part| part.is_empty()) {
            return Err(Error::Config(
                "shortcut contains an empty component; use `Plus` for the `+` key".to_string(),
            ));
        }

        let Some((key_token, modifier_tokens)) = parts.split_last() else {
            return Err(Error::Config("shortcut is missing a key".to_string()));
        };

        let mut modifiers = ShortcutModifiers::NONE;
        for token in modifier_tokens {
            let token = token.to_ascii_lowercase();
            let modifier = match token.as_str() {
                "ctrl" | "primary" | "cmdorctrl" | "commandorcontrol" => ShortcutModifiers::PRIMARY,
                "control" => ShortcutModifiers::CTRL,
                "cmd" | "command" | "maccmd" => ShortcutModifiers::MAC_CMD,
                "alt" | "option" => ShortcutModifiers::ALT,
                "shift" => ShortcutModifiers::SHIFT,
                _ => return Err(Error::Config(format!("unsupported modifier `{token}`"))),
            };
            if modifiers.contains(modifier) {
                return Err(Error::Config(format!("duplicate modifier `{token}`")));
            }
            modifiers.insert(modifier);
        }

        let key = parse_key(key_token)?;
        Ok(Self::new(modifiers, key))
    }

    #[must_use]
    pub fn display_label(self, primary_label: &str) -> String {
        let mut parts = Vec::new();
        if self.modifiers.command() {
            parts.push(primary_label.to_string());
        }
        if self.modifiers.ctrl() {
            parts.push("Control".to_string());
        }
        if self.modifiers.alt() {
            parts.push("Alt".to_string());
        }
        if self.modifiers.shift() {
            parts.push("Shift".to_string());
        }
        if self.modifiers.mac_cmd() {
            parts.push("Cmd".to_string());
        }
        parts.push(key_name(self.key).unwrap_or("Unknown").to_string());

        parts.join("+")
    }

    #[must_use]
    pub(crate) fn overlaps(self, other: Self) -> bool {
        self.key == other.key
            && (self
                .matching_event_modifiers()
                .into_iter()
                .any(|event| modifiers_match_logically(event, other.modifiers))
                || other
                    .matching_event_modifiers()
                    .into_iter()
                    .any(|event| modifiers_match_logically(event, self.modifiers)))
    }

    fn matching_event_modifiers(self) -> Vec<ShortcutModifiers> {
        let mut events = Vec::with_capacity(4);
        let base = self.modifiers;
        events.push(base);

        if self.modifiers.command() {
            push_unique_modifier(&mut events, base.plus(ShortcutModifiers::CTRL));
            push_unique_modifier(&mut events, base.plus(ShortcutModifiers::MAC_CMD));
        }
        if self.modifiers.ctrl() && !self.modifiers.command() {
            push_unique_modifier(&mut events, base.plus(ShortcutModifiers::PRIMARY));
        }
        if self.modifiers.mac_cmd() && !self.modifiers.command() {
            push_unique_modifier(&mut events, base.plus(ShortcutModifiers::PRIMARY));
        }

        events
    }
}

impl fmt::Display for ShortcutBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if self.modifiers.command() {
            parts.push("Ctrl");
        }
        if self.modifiers.ctrl() {
            parts.push("Control");
        }
        if self.modifiers.alt() {
            parts.push("Alt");
        }
        if self.modifiers.shift() {
            parts.push("Shift");
        }
        if self.modifiers.mac_cmd() {
            parts.push("Cmd");
        }
        parts.push(key_name(self.key)?);

        write!(f, "{}", parts.join("+"))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppShortcuts {
    pub command_palette: ShortcutBinding,
    pub new_terminal: ShortcutBinding,
    pub open_remote_hosts: ShortcutBinding,
    pub toggle_sidebar: ShortcutBinding,
    pub toggle_hud: ShortcutBinding,
    pub toggle_minimap: ShortcutBinding,
    pub align_workspaces_horizontally: ShortcutBinding,
    pub toggle_settings: ShortcutBinding,
    pub reset_view: ShortcutBinding,
    pub zoom_in: ShortcutBinding,
    pub zoom_out: ShortcutBinding,
    pub fullscreen_panel: ShortcutBinding,
    pub exit_fullscreen_panel: ShortcutBinding,
    pub fullscreen_window: ShortcutBinding,
    pub save_editor: ShortcutBinding,
}

impl Default for AppShortcuts {
    fn default() -> Self {
        let primary = ShortcutModifiers::PRIMARY;
        Self {
            command_palette: ShortcutBinding::new(primary, ShortcutKey::Letter('K')),
            new_terminal: ShortcutBinding::new(primary, ShortcutKey::Letter('N')),
            open_remote_hosts: ShortcutBinding::new(primary.plus(ShortcutModifiers::SHIFT), ShortcutKey::Letter('R')),
            toggle_sidebar: ShortcutBinding::new(primary, ShortcutKey::Letter('B')),
            toggle_hud: ShortcutBinding::new(primary.plus(ShortcutModifiers::SHIFT), ShortcutKey::Letter('H')),
            toggle_minimap: ShortcutBinding::new(primary.plus(ShortcutModifiers::SHIFT), ShortcutKey::Letter('M')),
            align_workspaces_horizontally: ShortcutBinding::new(
                primary.plus(ShortcutModifiers::SHIFT),
                ShortcutKey::Letter('A'),
            ),
            toggle_settings: ShortcutBinding::new(primary, ShortcutKey::Comma),
            reset_view: ShortcutBinding::new(primary, ShortcutKey::Digit(0)),
            zoom_in: ShortcutBinding::new(primary, ShortcutKey::Plus),
            zoom_out: ShortcutBinding::new(primary, ShortcutKey::Minus),
            fullscreen_panel: ShortcutBinding::new(ShortcutModifiers::NONE, ShortcutKey::Function(11)),
            exit_fullscreen_panel: ShortcutBinding::new(ShortcutModifiers::NONE, ShortcutKey::Escape),
            fullscreen_window: ShortcutBinding::new(primary, ShortcutKey::Function(11)),
            save_editor: ShortcutBinding::new(primary, ShortcutKey::Letter('S')),
        }
    }
}

fn push_unique_modifier(values: &mut Vec<ShortcutModifiers>, modifier: ShortcutModifiers) {
    if !values.contains(&modifier) {
        values.push(modifier);
    }
}

fn modifiers_match_logically(pressed: ShortcutModifiers, pattern: ShortcutModifiers) -> bool {
    if pattern.alt() && !pressed.alt() {
        return false;
    }
    if pattern.shift() && !pressed.shift() {
        return false;
    }

    if pattern.mac_cmd() {
        if !pressed.mac_cmd() {
            return false;
        }
        return pattern.ctrl() == pressed.ctrl();
    }

    if !pattern.ctrl() && !pattern.command() {
        return !pressed.ctrl() && !pressed.command();
    }

    if pattern.ctrl() && !pressed.ctrl() {
        return false;
    }
    if pattern.command() && !pressed.command() {
        return false;
    }

    true
}

fn parse_key(token: &str) -> Result<ShortcutKey> {
    let normalized = token.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "arrowdown" | "down" => Ok(ShortcutKey::ArrowDown),
        "arrowleft" | "left" => Ok(ShortcutKey::ArrowLeft),
        "arrowright" | "right" => Ok(ShortcutKey::ArrowRight),
        "arrowup" | "up" => Ok(ShortcutKey::ArrowUp),
        "escape" | "esc" => Ok(ShortcutKey::Escape),
        "enter" | "return" => Ok(ShortcutKey::Enter),
        "tab" => Ok(ShortcutKey::Tab),
        "comma" | "," => Ok(ShortcutKey::Comma),
        "minus" | "-" => Ok(ShortcutKey::Minus),
        "plus" | "+" | "equals" | "=" => Ok(ShortcutKey::Plus),
        _ => {
            if let Some(ch) = parse_letter(token) {
                return Ok(ShortcutKey::Letter(ch));
            }
            if let Some(digit) = parse_digit(token) {
                return Ok(ShortcutKey::Digit(digit));
            }
            if let Some(function) = parse_function(token) {
                return Ok(ShortcutKey::Function(function));
            }
            Err(Error::Config(format!("unsupported key `{token}`")))
        }
    }
}

fn parse_letter(token: &str) -> Option<char> {
    let mut chars = token.chars();
    let ch = chars.next()?;
    if chars.next().is_none() && ch.is_ascii_alphabetic() {
        Some(ch.to_ascii_uppercase())
    } else {
        None
    }
}

fn parse_digit(token: &str) -> Option<u8> {
    let normalized = token.trim().to_ascii_lowercase();
    if let Some(number) = normalized.strip_prefix("num") {
        return number.parse::<u8>().ok().filter(|digit| *digit <= 9);
    }

    let mut chars = normalized.chars();
    let ch = chars.next()?;
    if chars.next().is_none() && ch.is_ascii_digit() {
        ch.to_digit(10).and_then(|digit| u8::try_from(digit).ok())
    } else {
        None
    }
}

fn parse_function(token: &str) -> Option<u8> {
    let normalized = token.trim().to_ascii_lowercase();
    let number = normalized.strip_prefix('f')?.parse::<u8>().ok()?;
    (1..=35).contains(&number).then_some(number)
}

fn key_name(key: ShortcutKey) -> std::result::Result<&'static str, fmt::Error> {
    match key {
        ShortcutKey::ArrowDown => Ok("ArrowDown"),
        ShortcutKey::ArrowLeft => Ok("ArrowLeft"),
        ShortcutKey::ArrowRight => Ok("ArrowRight"),
        ShortcutKey::ArrowUp => Ok("ArrowUp"),
        ShortcutKey::Escape => Ok("Escape"),
        ShortcutKey::Enter => Ok("Enter"),
        ShortcutKey::Tab => Ok("Tab"),
        ShortcutKey::Comma => Ok("Comma"),
        ShortcutKey::Minus => Ok("Minus"),
        ShortcutKey::Plus => Ok("Plus"),
        ShortcutKey::Digit(0) => Ok("0"),
        ShortcutKey::Digit(1) => Ok("1"),
        ShortcutKey::Digit(2) => Ok("2"),
        ShortcutKey::Digit(3) => Ok("3"),
        ShortcutKey::Digit(4) => Ok("4"),
        ShortcutKey::Digit(5) => Ok("5"),
        ShortcutKey::Digit(6) => Ok("6"),
        ShortcutKey::Digit(7) => Ok("7"),
        ShortcutKey::Digit(8) => Ok("8"),
        ShortcutKey::Digit(9) => Ok("9"),
        ShortcutKey::Letter('A') => Ok("A"),
        ShortcutKey::Letter('B') => Ok("B"),
        ShortcutKey::Letter('C') => Ok("C"),
        ShortcutKey::Letter('D') => Ok("D"),
        ShortcutKey::Letter('E') => Ok("E"),
        ShortcutKey::Letter('F') => Ok("F"),
        ShortcutKey::Letter('G') => Ok("G"),
        ShortcutKey::Letter('H') => Ok("H"),
        ShortcutKey::Letter('I') => Ok("I"),
        ShortcutKey::Letter('J') => Ok("J"),
        ShortcutKey::Letter('K') => Ok("K"),
        ShortcutKey::Letter('L') => Ok("L"),
        ShortcutKey::Letter('M') => Ok("M"),
        ShortcutKey::Letter('N') => Ok("N"),
        ShortcutKey::Letter('O') => Ok("O"),
        ShortcutKey::Letter('P') => Ok("P"),
        ShortcutKey::Letter('Q') => Ok("Q"),
        ShortcutKey::Letter('R') => Ok("R"),
        ShortcutKey::Letter('S') => Ok("S"),
        ShortcutKey::Letter('T') => Ok("T"),
        ShortcutKey::Letter('U') => Ok("U"),
        ShortcutKey::Letter('V') => Ok("V"),
        ShortcutKey::Letter('W') => Ok("W"),
        ShortcutKey::Letter('X') => Ok("X"),
        ShortcutKey::Letter('Y') => Ok("Y"),
        ShortcutKey::Letter('Z') => Ok("Z"),
        ShortcutKey::Function(1) => Ok("F1"),
        ShortcutKey::Function(2) => Ok("F2"),
        ShortcutKey::Function(3) => Ok("F3"),
        ShortcutKey::Function(4) => Ok("F4"),
        ShortcutKey::Function(5) => Ok("F5"),
        ShortcutKey::Function(6) => Ok("F6"),
        ShortcutKey::Function(7) => Ok("F7"),
        ShortcutKey::Function(8) => Ok("F8"),
        ShortcutKey::Function(9) => Ok("F9"),
        ShortcutKey::Function(10) => Ok("F10"),
        ShortcutKey::Function(11) => Ok("F11"),
        ShortcutKey::Function(12) => Ok("F12"),
        ShortcutKey::Function(13) => Ok("F13"),
        ShortcutKey::Function(14) => Ok("F14"),
        ShortcutKey::Function(15) => Ok("F15"),
        ShortcutKey::Function(16) => Ok("F16"),
        ShortcutKey::Function(17) => Ok("F17"),
        ShortcutKey::Function(18) => Ok("F18"),
        ShortcutKey::Function(19) => Ok("F19"),
        ShortcutKey::Function(20) => Ok("F20"),
        ShortcutKey::Function(21) => Ok("F21"),
        ShortcutKey::Function(22) => Ok("F22"),
        ShortcutKey::Function(23) => Ok("F23"),
        ShortcutKey::Function(24) => Ok("F24"),
        ShortcutKey::Function(25) => Ok("F25"),
        ShortcutKey::Function(26) => Ok("F26"),
        ShortcutKey::Function(27) => Ok("F27"),
        ShortcutKey::Function(28) => Ok("F28"),
        ShortcutKey::Function(29) => Ok("F29"),
        ShortcutKey::Function(30) => Ok("F30"),
        ShortcutKey::Function(31) => Ok("F31"),
        ShortcutKey::Function(32) => Ok("F32"),
        ShortcutKey::Function(33) => Ok("F33"),
        ShortcutKey::Function(34) => Ok("F34"),
        ShortcutKey::Function(35) => Ok("F35"),
        ShortcutKey::Digit(_) | ShortcutKey::Letter(_) | ShortcutKey::Function(_) => Err(fmt::Error),
    }
}

#[cfg(test)]
mod tests {
    use super::{AppShortcuts, ShortcutBinding, ShortcutKey, ShortcutModifiers};

    #[test]
    fn parses_primary_letter_shortcuts() {
        let binding = ShortcutBinding::parse("Ctrl+K").expect("shortcut should parse");

        assert_eq!(
            binding,
            ShortcutBinding::new(ShortcutModifiers::PRIMARY, ShortcutKey::Letter('K'))
        );
    }

    #[test]
    fn parses_plus_aliases_to_single_key() {
        let plus = ShortcutBinding::parse("Ctrl+Plus").expect("plus shortcut should parse");
        let equals = ShortcutBinding::parse("Ctrl+=").expect("equals shortcut should parse");

        assert_eq!(plus, equals);
        assert_eq!(plus.key, ShortcutKey::Plus);
    }

    #[test]
    fn rejects_empty_components() {
        let error = ShortcutBinding::parse("Ctrl++").expect_err("shortcut should be rejected");

        assert!(error.to_string().contains("empty component"));
    }

    #[test]
    fn app_shortcuts_default_matches_documented_bindings() {
        let shortcuts = AppShortcuts::default();

        assert_eq!(
            shortcuts.command_palette,
            ShortcutBinding::new(ShortcutModifiers::PRIMARY, ShortcutKey::Letter('K'))
        );
        assert_eq!(
            shortcuts.toggle_hud,
            ShortcutBinding::new(
                ShortcutModifiers::PRIMARY.plus(ShortcutModifiers::SHIFT),
                ShortcutKey::Letter('H'),
            )
        );
        assert_eq!(
            shortcuts.open_remote_hosts,
            ShortcutBinding::new(
                ShortcutModifiers::PRIMARY.plus(ShortcutModifiers::SHIFT),
                ShortcutKey::Letter('R'),
            )
        );
        assert_eq!(
            shortcuts.toggle_minimap,
            ShortcutBinding::new(
                ShortcutModifiers::PRIMARY.plus(ShortcutModifiers::SHIFT),
                ShortcutKey::Letter('M'),
            )
        );
        assert_eq!(
            shortcuts.fullscreen_window,
            ShortcutBinding::new(ShortcutModifiers::PRIMARY, ShortcutKey::Function(11))
        );
        assert_eq!(
            shortcuts.save_editor,
            ShortcutBinding::new(ShortcutModifiers::PRIMARY, ShortcutKey::Letter('S'))
        );
    }

    #[test]
    fn shifted_and_unshifted_same_key_overlap() {
        let plain = ShortcutBinding::new(ShortcutModifiers::PRIMARY, ShortcutKey::Letter('B'));
        let shifted = ShortcutBinding::new(
            ShortcutModifiers::PRIMARY.plus(ShortcutModifiers::SHIFT),
            ShortcutKey::Letter('B'),
        );

        assert!(plain.overlaps(shifted));
        assert!(shifted.overlaps(plain));
    }

    #[test]
    fn different_keys_do_not_overlap() {
        let left = ShortcutBinding::new(ShortcutModifiers::PRIMARY, ShortcutKey::Letter('A'));
        let right = ShortcutBinding::new(ShortcutModifiers::PRIMARY, ShortcutKey::Letter('B'));

        assert!(!left.overlaps(right));
    }

    #[test]
    fn display_label_uses_platform_primary_label() {
        let binding = ShortcutBinding::new(
            ShortcutModifiers::PRIMARY.plus(ShortcutModifiers::SHIFT),
            ShortcutKey::Letter('A'),
        );

        assert_eq!(binding.display_label("Cmd"), "Cmd+Shift+A");
    }
}
