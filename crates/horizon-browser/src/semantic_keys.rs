//! Key events for the semantic `press` action, shared by the Chromium and
//! `WebDriver` engines so both deliver the same trusted key sequence.

use crate::{BrowserEditCommand, BrowserInput, BrowserKey, BrowserKeyChord, BrowserModifiers};

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
