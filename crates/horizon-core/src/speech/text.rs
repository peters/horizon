//! The clean of one text that a transcriber answered.
//!
//! The text comes from a service or from the standard output of a program, so it
//! is text of an answer and not a command. A terminal reads an escape byte, a
//! carriage return and every other control byte as a command, so this module
//! removes them: every line break and every tab becomes one space, every other
//! control character of C0, DEL and C1 goes, the result is trimmed and it is cut
//! at [`MAX_TEXT_BYTES`]. The insert path of the terminal paste passes the same
//! text, so an escape that survives no clean reaches no program.

/// The most bytes of one inserted text.
pub const MAX_TEXT_BYTES: usize = 8 * 1024;

/// `raw` as the text of one insert: no line break, no tab and no other control
/// character, trimmed and cut at [`MAX_TEXT_BYTES`].
#[must_use]
pub fn clean_text(raw: &str) -> String {
    let mut text = String::with_capacity(raw.len().min(MAX_TEXT_BYTES));
    let mut characters = raw.chars();
    while let Some(character) = characters.next() {
        match character {
            // One line break is one space, also when the service wrote two bytes
            // for it.
            '\r' => {
                if characters.clone().next() == Some('\n') {
                    characters.next();
                }
                text.push(' ');
            }
            '\n' | '\t' => text.push(' '),
            // C0, DEL and C1: an escape, a carriage return that a terminal reads
            // as a command, and the marker of a bracketed paste.
            character if character.is_control() => {}
            character => text.push(character),
        }
        if text.len() >= MAX_TEXT_BYTES {
            break;
        }
    }
    let trimmed = text.trim();
    cut(trimmed)
}

/// The first [`MAX_TEXT_BYTES`] bytes of `text`, at a character boundary.
fn cut(text: &str) -> String {
    if text.len() <= MAX_TEXT_BYTES {
        return text.to_owned();
    }
    let mut end = MAX_TEXT_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::{MAX_TEXT_BYTES, clean_text};

    #[test]
    fn every_line_break_and_tab_becomes_one_space() {
        assert_eq!(clean_text("one\ntwo\tthree\r\nfour\rfive"), "one two three four five");
        assert_eq!(clean_text("end\n"), "end");
        assert_eq!(clean_text("\n  start"), "start");
    }

    #[test]
    fn a_control_character_goes_and_an_escape_sequence_leaves_its_text() {
        // An escape and every other byte of a control sequence: a terminal would
        // read them as a command.
        assert_eq!(clean_text("\x1b[201~x\x1b[Ay"), "[201~x[Ay");
        assert_eq!(clean_text("a\u{7}b\u{0}c\u{7f}d\u{9b}e"), "abcde");
        assert_eq!(clean_text("\u{85}bell\u{7}"), "bell");
    }

    #[test]
    fn a_bracketed_paste_marker_inside_the_text_is_text() {
        let text = clean_text("before\u{1b}[201~after");
        assert!(!text.contains('\u{1b}'), "the marker keeps its escape byte: {text:?}");
        assert_eq!(text, "before[201~after");
    }

    #[test]
    fn one_megabyte_of_text_is_cut_at_the_bound() {
        let text = clean_text(&"a".repeat(1024 * 1024));
        assert_eq!(text.len(), MAX_TEXT_BYTES);

        // A cut lands on a character boundary, so a multi-byte character is never
        // half inserted.
        let text = clean_text(&"é".repeat(MAX_TEXT_BYTES));
        assert_eq!(text.len(), MAX_TEXT_BYTES);
        assert!(text.chars().all(|character| character == 'é'));

        // A cut that ends on a space keeps no trailing space.
        let raw = format!("{}x", "b".repeat(MAX_TEXT_BYTES - 1));
        assert_eq!(clean_text(&raw).len(), MAX_TEXT_BYTES);
        let raw = format!("{} ", "c".repeat(MAX_TEXT_BYTES));
        assert_eq!(clean_text(&raw).len(), MAX_TEXT_BYTES);
    }

    #[test]
    fn text_with_no_printable_character_becomes_no_text() {
        assert_eq!(clean_text("\n\t\r\n"), "");
        assert_eq!(clean_text("\u{1}\u{2}\u{1b}"), "");
        assert_eq!(clean_text(""), "");
    }
}
