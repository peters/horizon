//! Shared single-line text layout helpers.

use egui::{
    Color32, FontId,
    text::{LayoutJob, TextFormat, TextWrapping},
};

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
    use super::single_line_job;

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
}
