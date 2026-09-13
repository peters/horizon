//! Shared single-line text layout and truncation helpers.

use std::borrow::Cow;

use egui::{
    Color32, FontId,
    text::{LayoutJob, TextFormat, TextWrapping},
};

/// Caps `label` at `max_chars` characters, ending a truncated result with an
/// ellipsis that counts toward the budget. Borrows when nothing is cut.
///
/// The cut is character-based: byte-indexed slicing would panic whenever a
/// multi-byte character straddles the cut point.
pub(crate) fn truncate_chars(label: &str, max_chars: usize) -> Cow<'_, str> {
    if max_chars == 0 {
        return Cow::Borrowed(if label.is_empty() { label } else { "" });
    }

    let mut ellipsis_at = 0;
    for (char_index, (byte_index, _)) in label.char_indices().enumerate() {
        if char_index + 1 == max_chars {
            ellipsis_at = byte_index;
        } else if char_index + 1 > max_chars {
            let mut truncated = String::with_capacity(ellipsis_at + '…'.len_utf8());
            truncated.push_str(&label[..ellipsis_at]);
            truncated.push('…');
            return Cow::Owned(truncated);
        }
    }

    Cow::Borrowed(label)
}

/// Empty single-line layout job that elides overflow with an ellipsis instead
/// of wrapping; newlines render as spaces rather than swallowing the line.
pub(crate) fn single_line_job(max_width: f32) -> LayoutJob {
    LayoutJob {
        break_on_newline: false,
        wrap: TextWrapping {
            max_width: max_width.max(0.0),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('\u{2026}'),
        },
        ..Default::default()
    }
}

/// [`single_line_job`] pre-filled with one uniformly styled section.
pub(crate) fn single_line_label_job(text: &str, font: &FontId, color: Color32, max_width: f32) -> LayoutJob {
    let mut job = single_line_job(max_width);
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font.clone(),
            color,
            ..Default::default()
        },
    );
    job
}

#[cfg(test)]
mod tests {
    use super::{single_line_job, truncate_chars};

    #[test]
    fn single_line_job_enables_ellipsis_wrapping() {
        let job = single_line_job(96.0);

        assert!(!job.break_on_newline);
        assert!((job.wrap.max_width - 96.0).abs() < f32::EPSILON);
        assert_eq!(job.wrap.max_rows, 1);
        assert!(job.wrap.break_anywhere);
        assert_eq!(job.wrap.overflow_character, Some('\u{2026}'));
    }

    #[test]
    fn single_line_job_sanitizes_negative_width() {
        assert!(single_line_job(-4.0).wrap.max_width.abs() < f32::EPSILON);
    }

    #[test]
    fn truncate_chars_keeps_short_labels_untouched() {
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
