//! Shared bounded text layout primitives.

mod shaping;
mod tooltip;

use egui::{
    Color32, FontId,
    text::{LayoutJob, LayoutSection, TextFormat, TextWrapping},
};

pub(crate) use shaping::{BoundedSingleLineJob, context_text_galley, paint_section_header, painter_text_galley};
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
fn single_line_job(max_width: f32) -> LayoutJob {
    LayoutJob {
        break_on_newline: false,
        wrap: single_line_wrapping(max_width),
        ..Default::default()
    }
}

fn append_flattened_single_line_text(job: &mut LayoutJob, text: &str, leading_space: f32, format: TextFormat) {
    let start = job.text.len();
    job.text.reserve(text.len());
    job.text.push_str(text);
    job.sections.push(LayoutSection {
        leading_space,
        byte_range: start..job.text.len(),
        format,
    });
}

fn single_line_label_job_from_flattened(text: &str, font: &FontId, color: Color32, max_width: f32) -> LayoutJob {
    let mut job = single_line_job(max_width);
    append_flattened_single_line_text(
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
