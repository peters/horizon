//! Key events for the semantic `press` and `fill` actions, shared by the
//! Chromium and `WebDriver` engines so both deliver the same trusted key
//! sequence.

use crate::{BrowserEditCommand, BrowserInput, BrowserKey, BrowserKeyChord, BrowserModifiers};

/// How many characters a fill types as trusted key events. A longer value's
/// remainder is inserted as one trusted text insertion, so a large fill stays
/// bounded while a normal value still reaches `keydown` handlers.
pub(crate) const MAX_TYPED_FILL_CHARS: usize = 256;

/// The inputs that type `text` into the focused field. Each typeable character
/// is one trusted press, so a widget that enables its control from `keydown`
/// sees the filling as typing. Line breaks, tabs and other control characters
/// are inserted as text: a key for them would move focus, submit a form, or
/// add a character the text does not contain.
pub(crate) fn text_inputs(text: &str) -> Vec<BrowserInput> {
    let mut inputs = Vec::new();
    let mut insertion = String::new();
    let mut typed = 0;
    for character in text.chars() {
        match typed_key(character).filter(|_| typed < MAX_TYPED_FILL_CHARS) {
            Some(key) => {
                push_insertion(&mut inputs, &mut insertion);
                inputs.extend(press_inputs(key, BrowserModifiers::none()));
                typed += 1;
            }
            None => insertion.push(character),
        }
    }
    push_insertion(&mut inputs, &mut insertion);
    inputs
}

fn typed_key(character: char) -> Option<BrowserKey> {
    if character.is_control() {
        return None;
    }
    Some(if character == ' ' {
        BrowserKey::Space
    } else {
        BrowserKey::Char(character)
    })
}

fn push_insertion(inputs: &mut Vec<BrowserInput>, insertion: &mut String) {
    if !insertion.is_empty() {
        inputs.push(BrowserInput::InsertText {
            text: std::mem::take(insertion),
        });
    }
}

/// The key-down and key-up of one press. The key-up releases the modifiers,
/// so no modifier stays held for the next action or for the user.
pub(crate) fn press_inputs(key: BrowserKey, modifiers: BrowserModifiers) -> [BrowserInput; 2] {
    let text = BrowserKeyChord { key, modifiers }.text();
    // Chromium runs editing shortcuts only when the key event names the
    // command; select-all is the one shortcut that stays inside the page.
    let edit_command =
        ((modifiers.ctrl != modifiers.meta) && !modifiers.alt && !modifiers.shift && key == BrowserKey::Char('a'))
            .then_some(BrowserEditCommand::SelectAll);
    [
        BrowserInput::KeyDown {
            physical_key: Some(key),
            key,
            text: text.clone(),
            modifiers,
            repeat: false,
            edit_command,
        },
        BrowserInput::KeyUp {
            physical_key: Some(key),
            key,
            text,
            modifiers: BrowserModifiers::none(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sequence(text: &str) -> Vec<String> {
        text_inputs(text)
            .iter()
            .map(|input| match input {
                BrowserInput::KeyDown { text: Some(text), .. } => format!("down:{text}"),
                BrowserInput::KeyDown { key, text: None, .. } => format!("raw:{key:?}"),
                BrowserInput::KeyUp { key, .. } => format!("up:{key:?}"),
                BrowserInput::InsertText { text } => format!("insert:{text:?}"),
                other => panic!("unexpected fill input {other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_fill_types_text_and_inserts_characters_a_key_cannot_type() {
        assert_eq!(
            sequence("a b\nc\td"),
            [
                "down:a",
                "up:Char('a')",
                "down: ",
                "up:Space",
                "down:b",
                "up:Char('b')",
                "insert:\"\\n\"",
                "down:c",
                "up:Char('c')",
                "insert:\"\\t\"",
                "down:d",
                "up:Char('d')",
            ]
        );
        assert_eq!(sequence(""), Vec::<String>::new());
        assert_eq!(sequence("\n"), ["insert:\"\\n\""]);
    }

    #[test]
    fn a_long_fill_types_a_bounded_prefix_and_inserts_the_rest() {
        let text = "a".repeat(MAX_TYPED_FILL_CHARS + 3);
        let inputs = text_inputs(&text);
        assert_eq!(
            inputs
                .iter()
                .filter(|input| matches!(input, BrowserInput::KeyDown { .. }))
                .count(),
            MAX_TYPED_FILL_CHARS
        );
        assert_eq!(
            inputs.last(),
            Some(&BrowserInput::InsertText {
                text: "aaa".to_string()
            })
        );
    }

    #[test]
    fn shifted_tab_holds_shift_for_the_key_down_and_releases_it() {
        let modifiers = BrowserKeyChord::parse("Shift+Tab").expect("chord").modifiers;
        let [down, up] = press_inputs(BrowserKey::Tab, modifiers);
        assert!(matches!(
            down,
            BrowserInput::KeyDown {
                key: BrowserKey::Tab,
                text: None,
                modifiers: BrowserModifiers { shift: true, .. },
                edit_command: None,
                ..
            }
        ));
        assert!(matches!(
            up,
            BrowserInput::KeyUp {
                key: BrowserKey::Tab,
                modifiers: BrowserModifiers {
                    shift: false,
                    ctrl: false,
                    alt: false,
                    meta: false,
                },
                ..
            }
        ));
    }

    #[test]
    fn printable_keys_commit_text_and_select_all_names_its_command() {
        let shift = BrowserKeyChord::parse("A").expect("chord");
        let [down, _] = press_inputs(shift.key, shift.modifiers);
        assert!(matches!(down, BrowserInput::KeyDown { text: Some(ref text), .. } if text == "A"));
        let select_all = BrowserKeyChord::parse("Control+a").expect("chord");
        let [down, _] = press_inputs(select_all.key, select_all.modifiers);
        assert!(matches!(
            down,
            BrowserInput::KeyDown {
                text: None,
                edit_command: Some(BrowserEditCommand::SelectAll),
                ..
            }
        ));
        let both = BrowserKeyChord::parse("Control+Meta+a").expect("chord");
        let [down, _] = press_inputs(both.key, both.modifiers);
        assert!(
            matches!(down, BrowserInput::KeyDown { edit_command: None, .. }),
            "only a single primary modifier names select-all, as WebDriver sends the chord itself"
        );
        let copy = BrowserKeyChord::parse("Control+c").expect("chord");
        let [down, _] = press_inputs(copy.key, copy.modifiers);
        assert!(
            matches!(down, BrowserInput::KeyDown { edit_command: None, .. }),
            "a pressed copy shortcut must not reach the host clipboard path"
        );
    }
}
