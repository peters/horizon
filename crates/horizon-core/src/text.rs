//! Shared text transformation helpers.

use std::borrow::Cow;

/// Replaces mandatory line separators with spaces for single-line UI text.
/// A CRLF pair becomes one space.
///
/// The original string is borrowed when no normalization is needed.
#[must_use]
pub fn flatten_line_separators(text: &str) -> Cow<'_, str> {
    if !text.chars().any(is_line_separator) {
        return Cow::Borrowed(text);
    }

    let mut flattened = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\r' {
            if characters.peek().is_some_and(|next| *next == '\n') {
                let _ = characters.next();
            }
            flattened.push(' ');
        } else if is_line_separator(character) {
            flattened.push(' ');
        } else {
            flattened.push(character);
        }
    }

    Cow::Owned(flattened)
}

/// Returns whether `character` forces a new line in plain text.
#[must_use]
pub fn is_line_separator(character: char) -> bool {
    matches!(
        character,
        '\r' | '\n' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

/// Caps `text` at `max_chars` Unicode scalar values, ending a truncated
/// result with an ellipsis that counts toward the budget.
///
/// The original string is borrowed when no truncation is needed.
#[must_use]
pub fn truncate_chars(text: &str, max_chars: usize) -> Cow<'_, str> {
    if max_chars == 0 {
        return Cow::Borrowed("");
    }

    let mut ellipsis_at = 0;
    for (char_index, (byte_index, _)) in text.char_indices().enumerate() {
        if char_index + 1 == max_chars {
            ellipsis_at = byte_index;
        } else if char_index + 1 > max_chars {
            let mut truncated = String::with_capacity(ellipsis_at + '…'.len_utf8());
            truncated.push_str(&text[..ellipsis_at]);
            truncated.push('…');
            return Cow::Owned(truncated);
        }
    }

    Cow::Borrowed(text)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{flatten_line_separators, truncate_chars};

    #[test]
    fn line_separator_flattening_borrows_plain_text_and_normalizes_crlf() {
        assert!(matches!(flatten_line_separators("build server"), Cow::Borrowed(_)));
        assert_eq!(
            flatten_line_separators("build\r\nserver\u{2028}ready\u{0085}now"),
            "build server ready now"
        );
    }

    #[test]
    fn keeps_short_text_borrowed() {
        assert_eq!(truncate_chars("build", 8), "build");
        assert_eq!(truncate_chars("exactly8", 8), "exactly8");
        assert!(matches!(truncate_chars("build", 8), Cow::Borrowed(_)));
    }

    #[test]
    fn caps_at_budget_including_ellipsis() {
        let truncated = truncate_chars("docker compose up", 8);

        assert_eq!(truncated, "docker …");
        assert_eq!(truncated.chars().count(), 8);
    }

    #[test]
    fn honors_tiny_budgets() {
        assert_eq!(truncate_chars("abc", 0), "");
        assert_eq!(truncate_chars("abc", 1), "…");
        assert_eq!(truncate_chars("", 0), "");
    }

    #[test]
    fn truncates_only_at_utf8_character_boundaries() {
        assert_eq!(truncate_chars("blåbærsyltetøy", 6), "blåbæ…");
        assert_eq!(
            truncate_chars("aaaaaaaaaaaaaaaaaaaaaaaaaaaaåz", 30),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaåz"
        );
        assert_eq!(
            truncate_chars("aaaaaaaaaaaaaaaaaaaaaaaaaaaaåzz", 30),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaå…"
        );
    }
}
