//! Chords a terminal serves itself: the clipboard and the scrollback.
//!
//! A terminal sends most keys to the program in the PTY, but the clipboard
//! chords and the scrollback chords belong to the terminal window. The set
//! below is the one GNOME Terminal and xterm ship, so a user who knows one of
//! them finds the same keys here. The toolkit also owns some of these chords;
//! the ones it does not classify as clipboard commands reach this module.
use egui::{Key, Modifiers};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TerminalLocalCommand {
    /// Copy the selection to the clipboard and the PRIMARY selection.
    Copy,
    /// Paste the clipboard text, or the path of a clipboard image.
    Paste,
    /// Scroll toward older output. A positive count moves up, a negative down.
    ScrollLines(i32),
    /// Scroll one page of the panel, counted like [`Self::ScrollLines`].
    ScrollPage(i32),
    /// Show the oldest output that the scrollback keeps.
    ScrollToTop,
    /// Return to the newest output.
    ScrollToBottom,
}

/// The terminal command this chord runs, or `None` to send the key on to the
/// program in the PTY.
///
/// `Alt` chords stay with the PTY, because terminals use them as Meta. So do
/// the unmodified navigation keys: full-screen and line-editing programs bind
/// them.
pub(crate) fn terminal_local_command(
    key: Key,
    physical_key: Option<Key>,
    modifiers: Modifiers,
) -> Option<TerminalLocalCommand> {
    if modifiers.alt {
        return None;
    }

    let matches = |wanted: Key| key == wanted || physical_key == Some(wanted);
    // The toolkit reports Control as `command` too, and macOS reports Command
    // there; both reach the same chords on their platform.
    let control = (modifiers.ctrl || modifiers.command) && !modifiers.mac_cmd;
    let shift_only = modifiers.shift && !control;

    if control {
        if modifiers.shift && matches(Key::C) {
            return Some(TerminalLocalCommand::Copy);
        }
        if modifiers.shift && matches(Key::V) {
            return Some(TerminalLocalCommand::Paste);
        }
        if !modifiers.shift && matches(Key::Insert) {
            return Some(TerminalLocalCommand::Copy);
        }
        if modifiers.shift {
            if matches(Key::ArrowUp) {
                return Some(TerminalLocalCommand::ScrollLines(1));
            }
            if matches(Key::ArrowDown) {
                return Some(TerminalLocalCommand::ScrollLines(-1));
            }
        }
        return None;
    }

    if shift_only {
        if matches(Key::Insert) {
            return Some(TerminalLocalCommand::Paste);
        }
        if matches(Key::PageUp) {
            return Some(TerminalLocalCommand::ScrollPage(1));
        }
        if matches(Key::PageDown) {
            return Some(TerminalLocalCommand::ScrollPage(-1));
        }
        if matches(Key::Home) {
            return Some(TerminalLocalCommand::ScrollToTop);
        }
        if matches(Key::End) {
            return Some(TerminalLocalCommand::ScrollToBottom);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{TerminalLocalCommand, terminal_local_command};
    use egui::{Key, Modifiers};

    fn command(key: Key, modifiers: Modifiers) -> Option<TerminalLocalCommand> {
        terminal_local_command(key, Some(key), modifiers)
    }

    #[test]
    fn terminal_copy_and_paste_chords_follow_gnome_and_xterm() {
        for modifiers in [
            Modifiers::CTRL | Modifiers::SHIFT,
            Modifiers::COMMAND | Modifiers::SHIFT,
        ] {
            assert_eq!(
                command(Key::C, modifiers),
                Some(TerminalLocalCommand::Copy),
                "{modifiers:?}"
            );
            assert_eq!(
                command(Key::V, modifiers),
                Some(TerminalLocalCommand::Paste),
                "{modifiers:?}"
            );
        }
        assert_eq!(command(Key::Insert, Modifiers::CTRL), Some(TerminalLocalCommand::Copy));
        assert_eq!(
            command(Key::Insert, Modifiers::SHIFT),
            Some(TerminalLocalCommand::Paste)
        );
    }

    #[test]
    fn plain_control_letters_stay_with_the_program() {
        assert_eq!(command(Key::C, Modifiers::CTRL), None, "Ctrl+C is a signal");
        assert_eq!(command(Key::V, Modifiers::CTRL), None, "the toolkit pastes Ctrl+V");
        assert_eq!(command(Key::A, Modifiers::CTRL), None);
    }

    #[test]
    fn scrollback_chords_match_gnome_terminal() {
        assert_eq!(
            command(Key::PageUp, Modifiers::SHIFT),
            Some(TerminalLocalCommand::ScrollPage(1))
        );
        assert_eq!(
            command(Key::PageDown, Modifiers::SHIFT),
            Some(TerminalLocalCommand::ScrollPage(-1))
        );
        assert_eq!(
            command(Key::Home, Modifiers::SHIFT),
            Some(TerminalLocalCommand::ScrollToTop)
        );
        assert_eq!(
            command(Key::End, Modifiers::SHIFT),
            Some(TerminalLocalCommand::ScrollToBottom)
        );
        assert_eq!(
            command(Key::ArrowUp, Modifiers::CTRL | Modifiers::SHIFT),
            Some(TerminalLocalCommand::ScrollLines(1))
        );
        assert_eq!(
            command(Key::ArrowDown, Modifiers::CTRL | Modifiers::SHIFT),
            Some(TerminalLocalCommand::ScrollLines(-1))
        );
    }

    #[test]
    fn unmodified_navigation_keys_stay_with_the_program() {
        for key in [Key::PageUp, Key::PageDown, Key::Home, Key::End, Key::ArrowUp] {
            assert_eq!(command(key, Modifiers::NONE), None, "{key:?}");
        }
    }

    #[test]
    fn alt_chords_and_other_control_chords_stay_with_the_program() {
        assert_eq!(command(Key::PageUp, Modifiers::ALT | Modifiers::SHIFT), None);
        assert_eq!(
            command(Key::C, Modifiers::ALT | Modifiers::CTRL | Modifiers::SHIFT),
            None
        );
        assert_eq!(command(Key::PageUp, Modifiers::CTRL | Modifiers::SHIFT), None);
        assert_eq!(command(Key::Home, Modifiers::CTRL | Modifiers::SHIFT), None);
        assert_eq!(command(Key::Home, Modifiers::CTRL), None);
        assert_eq!(command(Key::Insert, Modifiers::CTRL | Modifiers::SHIFT), None);
        assert_eq!(command(Key::Insert, Modifiers::ALT | Modifiers::SHIFT), None);
    }

    #[test]
    fn physical_key_serves_layouts_without_a_latin_letter() {
        assert_eq!(
            terminal_local_command(Key::PageUp, Some(Key::C), Modifiers::CTRL | Modifiers::SHIFT),
            Some(TerminalLocalCommand::Copy)
        );
        assert_eq!(
            terminal_local_command(Key::PageUp, Some(Key::V), Modifiers::CTRL | Modifiers::SHIFT),
            Some(TerminalLocalCommand::Paste)
        );
    }
}
