//! Shared bounded text layout primitives.

mod shaping;
mod tooltip;

use egui::{
    Color32, FontId,
    text::{LayoutJob, LayoutSection, TextFormat, TextWrapping},
};
use horizon_core::flatten_line_separators;

pub(crate) use shaping::{context_text_galley, paint_section_header, painter_text_galley};
pub(crate) use tooltip::{
    single_line_tooltip, stable_hover_text, stable_hover_text_lazy, stable_wrapped_hover_text,
    stable_wrapped_hover_text_lazy,
};

fn single_line_wrapping(max_width: f32) -> TextWrapping {
    TextWrapping {
        max_width: max_width.max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('\u{2026}'),
    }
}

/// Empty single-line layout job that elides overflow with an ellipsis instead
/// of wrapping.
pub(crate) fn single_line_job(max_width: f32) -> LayoutJob {
    LayoutJob {
        break_on_newline: false,
        wrap: single_line_wrapping(max_width),
        ..Default::default()
    }
}

/// Appends one styled section while normalizing every line separator to a
/// space. A CRLF pair becomes one space.
pub(crate) fn append_single_line_text(job: &mut LayoutJob, text: &str, leading_space: f32, format: TextFormat) {
    let start = job.text.len();
    job.text.reserve(text.len());
    job.text.push_str(&flatten_line_separators(text));
    job.sections.push(LayoutSection {
        leading_space,
        byte_range: start..job.text.len(),
        format,
    });
}

/// [`single_line_job`] pre-filled with one uniformly styled section.
pub(crate) fn single_line_label_job(text: &str, font: &FontId, color: Color32, max_width: f32) -> LayoutJob {
    let mut job = single_line_job(max_width);
    append_single_line_text(
        &mut job,
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
    use egui::{Color32, FontId, text::TextFormat};

    use super::{append_single_line_text, single_line_job, single_line_label_job};

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
    fn single_line_label_job_preserves_text_and_style() {
        let font = FontId::proportional(13.0);
        let color = Color32::from_rgb(10, 20, 30);
        let job = single_line_label_job("first\r\nsecond\u{2028}third", &font, color, 96.0);

        assert_eq!(job.text, "first second third");
        assert!(!job.break_on_newline);
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].format.font_id, font);
        assert_eq!(job.sections[0].format.color, color);
    }

    #[test]
    fn single_line_text_flattens_all_mandatory_line_separators() {
        let font = FontId::proportional(13.0);
        let job = single_line_label_job(
            "cr\rlf\nvt\u{000B}ff\u{000C}nel\u{0085}ls\u{2028}ps\u{2029}end",
            &font,
            Color32::WHITE,
            400.0,
        );

        assert_eq!(job.text, "cr lf vt ff nel ls ps end");
    }

    #[test]
    fn append_single_line_text_flattens_newlines_without_extra_sections() {
        let mut job = single_line_job(96.0);
        append_single_line_text(&mut job, "first\r\nsecond", 2.0, TextFormat::default());

        assert_eq!(job.text, "first second");
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].byte_range, 0..job.text.len());
        assert!((job.sections[0].leading_space - 2.0).abs() < f32::EPSILON);
    }
}
