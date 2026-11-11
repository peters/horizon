//! Small shared helpers with no more specific module.

use std::borrow::Cow;

/// A program that ends at once with success, for a test that must start no
/// shell, no ssh and no kill command.
///
/// The path exists on Linux and on macOS: macOS keeps `true` in `/usr/bin`, and
/// a smaller Linux keeps it in `/bin`. A test that names one of those paths
/// passes on one system and fails on the other, so it asks for this program.
#[doc(hidden)]
#[must_use]
pub fn stand_in_program() -> &'static str {
    ["/usr/bin/true", "/bin/true"]
        .into_iter()
        .find(|candidate| std::path::Path::new(candidate).exists())
        .unwrap_or("/usr/bin/true")
}

/// Caps `value` at `max_chars` characters, ending a truncated result with an
/// ellipsis that counts toward the budget. Borrows when nothing is cut.
///
/// The cut is character-based: byte-indexed slicing would panic whenever a
/// multi-byte character straddling the cut point. This is the canonical
/// implementation; the UI re-exports it as `crate::text::truncate_chars`.
#[must_use]
pub fn truncate_chars(value: &str, max_chars: usize) -> Cow<'_, str> {
    if max_chars == 0 {
        return Cow::Borrowed(if value.is_empty() { value } else { "" });
    }

    let mut ellipsis_at = 0;
    for (char_index, (byte_index, _)) in value.char_indices().enumerate() {
        if char_index + 1 == max_chars {
            ellipsis_at = byte_index;
        } else if char_index + 1 > max_chars {
            let mut truncated = String::with_capacity(ellipsis_at + '…'.len_utf8());
            truncated.push_str(&value[..ellipsis_at]);
            truncated.push('…');
            return Cow::Owned(truncated);
        }
    }

    Cow::Borrowed(value)
}

#[cfg(test)]
mod tests {
    use super::{stand_in_program, truncate_chars};

    #[test]
    fn stand_in_program_names_a_program_that_exists() {
        assert!(
            std::path::Path::new(stand_in_program()).exists(),
            "the stand-in program of a test exists on this system"
        );
    }

    #[test]
    fn truncate_chars_keeps_short_values_untouched() {
        assert_eq!(truncate_chars("build", 8), "build");
        assert_eq!(truncate_chars("exactly8", 8), "exactly8");
    }

    #[test]
    fn truncate_chars_caps_at_budget_including_ellipsis() {
        let truncated = truncate_chars("docker compose up", 8);

        assert_eq!(truncated, "docker …");
        assert_eq!(truncated.chars().count(), 8);
    }

    #[test]
    fn truncate_chars_honors_tiny_budgets() {
        assert_eq!(truncate_chars("abc", 0), "");
        assert_eq!(truncate_chars("abc", 1), "…");
        assert_eq!(truncate_chars("", 0), "");
    }

    #[test]
    fn truncate_chars_counts_characters_not_bytes() {
        assert_eq!(truncate_chars("blåbærsyltetøy", 6), "blåbæ…");
    }
}
