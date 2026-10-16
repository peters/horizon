//! Key chords named in text, such as `Tab`, `Shift+Tab` or `Control+a`.

use super::{BrowserKey, BrowserModifiers, shifted_char};

/// Longest chord text accepted, in bytes.
const MAX_CHORD_BYTES: usize = 64;

/// One key with the modifiers held while it is pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrowserKeyChord {
    pub key: BrowserKey,
    pub modifiers: BrowserModifiers,
}

impl BrowserKeyChord {
    /// Parse `Modifier+...+Key`. Modifiers are `Shift`, `Control` (`Ctrl`),
    /// `Alt` (`Option`) and `Meta` (`Command`, `Cmd`); names are not case
    /// sensitive. A key is a named key (`Tab`, `Enter`, `Escape`, `Space`,
    /// `Backspace`, `Delete`, `Home`, `End`, `PageUp`, `PageDown`, `Insert`,
    /// `ArrowUp` (`Up`) and the other arrows, `F1` to `F12`) or one printable
    /// US-layout ASCII character. An uppercase letter or a shifted symbol
    /// adds `Shift` to its base key, so `A` is `Shift+a` and `!` is `Shift+1`.
    ///
    /// # Errors
    /// Returns a stable explanation for empty, oversized or unknown chords.
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let text = text.trim();
        if text.is_empty() {
            return Err("press requires a key");
        }
        if text.len() > MAX_CHORD_BYTES {
            return Err("key chord exceeds 64 bytes");
        }
        // A trailing `+` key is spelled `+` or `Modifier++`.
        let (modifier_text, key_text) = if text == "+" {
            (None, "+")
        } else if let Some(prefix) = text.strip_suffix("++") {
            (Some(prefix), "+")
        } else {
            text.rsplit_once('+')
                .map_or((None, text), |(modifiers, key)| (Some(modifiers), key))
        };
        let mut modifiers = BrowserModifiers::none();
        for name in modifier_text.into_iter().flat_map(|text| text.split('+')) {
            apply_modifier(&mut modifiers, name.trim())?;
        }
        let (key, shift) = parse_key(key_text.trim())?;
        modifiers.shift |= shift;
        Ok(Self { key, modifiers })
    }

    /// Text the press commits to a focused field, when it commits any.
    /// A chord with `Control`, `Alt` or `Meta` is a shortcut, not text.
    #[must_use]
    pub fn text(self) -> Option<String> {
        if self.modifiers.ctrl || self.modifiers.alt || self.modifiers.meta {
            return None;
        }
        match self.key {
            BrowserKey::Space => Some(" ".to_string()),
            key => key
                .printable_char_with_shift(self.modifiers.shift)
                .map(|character| character.to_string()),
        }
    }
}

fn apply_modifier(modifiers: &mut BrowserModifiers, name: &str) -> Result<(), &'static str> {
    match name.to_ascii_lowercase().as_str() {
        "shift" => modifiers.shift = true,
        "control" | "ctrl" => modifiers.ctrl = true,
        "alt" | "option" => modifiers.alt = true,
        "meta" | "command" | "cmd" => modifiers.meta = true,
        "" => return Err("key chord has an empty modifier"),
        _ => return Err("unknown modifier; use Shift, Control, Alt or Meta"),
    }
    Ok(())
}

fn parse_key(name: &str) -> Result<(BrowserKey, bool), &'static str> {
    let mut characters = name.chars();
    if let (Some(character), None) = (characters.next(), characters.next()) {
        return printable_key(character).ok_or("press accepts printable US-layout ASCII keys; use fill for other text");
    }
    let key = match name.to_ascii_lowercase().as_str() {
        "tab" => BrowserKey::Tab,
        "enter" | "return" => BrowserKey::Enter,
        "escape" | "esc" => BrowserKey::Escape,
        "space" => BrowserKey::Space,
        "backspace" => BrowserKey::Backspace,
        "delete" | "del" => BrowserKey::Delete,
        "home" => BrowserKey::Home,
        "end" => BrowserKey::End,
        "pageup" => BrowserKey::PageUp,
        "pagedown" => BrowserKey::PageDown,
        "insert" => BrowserKey::Insert,
        "arrowup" | "up" => BrowserKey::ArrowUp,
        "arrowdown" | "down" => BrowserKey::ArrowDown,
        "arrowleft" | "left" => BrowserKey::ArrowLeft,
        "arrowright" | "right" => BrowserKey::ArrowRight,
        "" => return Err("key chord has no key"),
        other => function_key(other).ok_or("unknown key name")?,
    };
    Ok((key, false))
}

fn function_key(name: &str) -> Option<BrowserKey> {
    // WebDriver key actions define no code for F13 and above.
    const KEYS: [BrowserKey; 12] = [
        BrowserKey::F1,
        BrowserKey::F2,
        BrowserKey::F3,
        BrowserKey::F4,
        BrowserKey::F5,
        BrowserKey::F6,
        BrowserKey::F7,
        BrowserKey::F8,
        BrowserKey::F9,
        BrowserKey::F10,
        BrowserKey::F11,
        BrowserKey::F12,
    ];
    let number = name.strip_prefix('f')?;
    if number.starts_with('0') {
        return None;
    }
    let index = number.parse::<usize>().ok()?.checked_sub(1)?;
    KEYS.get(index).copied()
}

/// The base key and whether `Shift` produces `character` on the US layout.
fn printable_key(character: char) -> Option<(BrowserKey, bool)> {
    if !character.is_ascii_graphic() {
        return None;
    }
    if character.is_ascii_uppercase() {
        return Some((BrowserKey::Char(character.to_ascii_lowercase()), true));
    }
    let base = (' '..='~').find(|base| {
        base.is_ascii_graphic() && !base.is_ascii_uppercase() && *base != character && shifted_char(*base) == character
    });
    Some(base.map_or((BrowserKey::Char(character), false), |base| {
        (BrowserKey::Char(base), true)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(key: BrowserKey, modifiers: BrowserModifiers) -> BrowserKeyChord {
        BrowserKeyChord { key, modifiers }
    }

    const SHIFT: BrowserModifiers = BrowserModifiers {
        alt: false,
        ctrl: false,
        meta: false,
        shift: true,
    };

    #[test]
    fn named_keys_and_modifiers_parse_without_case() {
        assert_eq!(
            BrowserKeyChord::parse("Tab"),
            Ok(chord(BrowserKey::Tab, BrowserModifiers::none()))
        );
        assert_eq!(BrowserKeyChord::parse(" shift+TAB "), Ok(chord(BrowserKey::Tab, SHIFT)));
        let all = BrowserKeyChord::parse("Ctrl+Alt+Meta+Shift+ArrowDown").expect("chord");
        assert_eq!(all.key, BrowserKey::ArrowDown);
        assert!(all.modifiers.ctrl && all.modifiers.alt && all.modifiers.meta && all.modifiers.shift);
        assert_eq!(
            BrowserKeyChord::parse("esc").map(|chord| chord.key),
            Ok(BrowserKey::Escape)
        );
        assert_eq!(
            BrowserKeyChord::parse("F12").map(|chord| chord.key),
            Ok(BrowserKey::F12)
        );
        assert_eq!(
            BrowserKeyChord::parse("Left").map(|chord| chord.key),
            Ok(BrowserKey::ArrowLeft)
        );
    }

    #[test]
    fn printable_keys_use_their_us_layout_base_key() {
        assert_eq!(
            BrowserKeyChord::parse("a"),
            Ok(chord(BrowserKey::Char('a'), BrowserModifiers::none()))
        );
        assert_eq!(BrowserKeyChord::parse("A"), Ok(chord(BrowserKey::Char('a'), SHIFT)));
        assert_eq!(BrowserKeyChord::parse("!"), Ok(chord(BrowserKey::Char('1'), SHIFT)));
        assert_eq!(BrowserKeyChord::parse("+"), Ok(chord(BrowserKey::Char('='), SHIFT)));
        assert_eq!(
            BrowserKeyChord::parse("Control++").map(|chord| (chord.key, chord.modifiers.ctrl, chord.modifiers.shift)),
            Ok((BrowserKey::Char('='), true, true))
        );
        let select_all = BrowserKeyChord::parse("Control+a").expect("chord");
        assert_eq!(select_all.key, BrowserKey::Char('a'));
        assert!(select_all.modifiers.ctrl && !select_all.modifiers.shift);
    }

    #[test]
    fn committed_text_follows_shift_and_shortcut_modifiers() {
        let text = |chord: &str| BrowserKeyChord::parse(chord).expect("chord").text();
        assert_eq!(text("a").as_deref(), Some("a"));
        assert_eq!(text("Shift+a").as_deref(), Some("A"));
        assert_eq!(text("?").as_deref(), Some("?"));
        assert_eq!(text("Space").as_deref(), Some(" "));
        assert_eq!(text("Control+a"), None);
        assert_eq!(text("Alt+Space"), None);
        assert_eq!(text("Tab"), None);
        assert_eq!(text("Enter"), None);
    }

    #[test]
    fn malformed_chords_are_refused() {
        for text in [
            "",
            "   ",
            "Shift+",
            "+Tab",
            "Hyper+Tab",
            "Tabby",
            "F0",
            "F13",
            "F16",
            "F01",
            "å",
            "Shift+Control",
            "Shift++Tab",
        ] {
            assert!(BrowserKeyChord::parse(text).is_err(), "{text:?} must be refused");
        }
        assert!(BrowserKeyChord::parse(&"Shift+".repeat(11)).is_err());
    }
}
