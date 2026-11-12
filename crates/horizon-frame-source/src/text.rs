//! The text of a program as a panel shows it: bounded, flattened and trimmed.
//!
//! Every text that reaches a panel comes from the program, which is untrusted: the
//! name of the source, the reason of a `state` message, the message of an `error`
//! message and the last line of its standard error. One control character would
//! move a line of a panel or reach a terminal, so each of them becomes one space,
//! and the text is cut at the bound of the contract.

/// The text of `raw`, cut at `max` characters, with every control character as one
/// space and the ends trimmed. An empty text is `None`.
#[must_use]
pub fn label(raw: Option<&str>, max: usize) -> Option<String> {
    let raw = raw?;
    let bounded: String = raw
        .chars()
        .take(max)
        .map(|character| if character.is_control() { ' ' } else { character })
        .collect();
    let trimmed = bounded.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::label;

    #[test]
    fn a_text_of_a_program_loses_its_controls_and_its_end() {
        assert_eq!(label(Some("  a b  "), 10).as_deref(), Some("a b"));
        assert_eq!(label(Some("a\nb\tc"), 10).as_deref(), Some("a b c"));
        assert_eq!(label(Some("\u{7}\r"), 10), None);
        assert_eq!(label(None, 10), None);
        // The cut counts characters, not bytes, so a name of two bytes is not split.
        assert_eq!(label(Some("øøøø"), 2).as_deref(), Some("øø"));
        assert_eq!(label(Some(&"a".repeat(300)), 200).map(|text| text.len()), Some(200));
    }
}
