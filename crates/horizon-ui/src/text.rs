//! Shared single-line text layout helpers.

use egui::{
    Color32, FontId, Label, TextWrapMode, Ui,
    text::{LayoutJob, TextFormat, TextWrapping},
};

/// Character-safe truncation; the canonical implementation lives in
/// `horizon-core` so core and UI call sites share one helper.
pub(crate) use horizon_core::truncate_chars;

/// Stable width budget for tooltip contents.
///
/// egui lays an open tooltip out inside the size it measured last frame, so a
/// wrapping label in a continuously open tooltip narrows a little every frame
/// until it collapses into a glyph-wide strip. Bounding the contents by a
/// width derived from the content area — not the tooltip's own measured size —
/// breaks that feedback loop.
pub(crate) fn stable_tooltip_max_width(ui: &Ui) -> f32 {
    tooltip_width_budget(ui.ctx().content_rect().width(), ui.spacing().tooltip_width)
}

/// Width budget for tooltip contents: the content-area width minus a margin,
/// clamped so a tiny window cannot shrink a tooltip below a readable floor and
/// a huge one cannot grow it past the style's tooltip width.
pub(crate) fn tooltip_width_budget(content_width: f32, tooltip_width: f32) -> f32 {
    (content_width - 24.0).clamp(80.0, tooltip_width.max(80.0))
}

/// Tooltip content as a single line elided to a stable width.
pub(crate) fn truncating_tooltip_label(ui: &mut Ui, text: &str) {
    ui.set_max_width(stable_tooltip_max_width(ui));
    ui.add(Label::new(text).wrap_mode(TextWrapMode::Truncate));
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
    use super::{single_line_job, tooltip_width_budget};

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

    fn nearly_eq(a: f32, b: f32) {
        assert!((a - b).abs() < f32::EPSILON);
    }

    #[test]
    fn tooltip_width_budget_clamps_to_the_style_tooltip_width() {
        // Wide content area: the style cap wins.
        nearly_eq(tooltip_width_budget(2000.0, 320.0), 320.0);
        // Exactly at the cap: no growth.
        nearly_eq(tooltip_width_budget(344.0, 320.0), 320.0);
    }

    #[test]
    fn tooltip_width_budget_respects_narrow_content_areas() {
        // Inside the normal range: content width minus the margin.
        nearly_eq(tooltip_width_budget(300.0, 320.0), 276.0);
        nearly_eq(tooltip_width_budget(104.0, 320.0), 80.0);
    }

    #[test]
    fn tooltip_width_budget_never_drops_below_the_readable_floor() {
        // Tiny windows clamp up to the floor instead of collapsing.
        nearly_eq(tooltip_width_budget(50.0, 320.0), 80.0);
        nearly_eq(tooltip_width_budget(0.0, 320.0), 80.0);
    }

    #[test]
    fn tooltip_width_budget_floor_wins_over_a_tiny_style_cap() {
        // A style cap below the floor is overridden so tooltips stay readable.
        nearly_eq(tooltip_width_budget(2000.0, 40.0), 80.0);
    }
}
