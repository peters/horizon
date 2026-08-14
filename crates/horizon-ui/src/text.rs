//! Shared single-line text layout helpers.

use std::{borrow::Cow, sync::Arc};

use egui::{
    Color32, FontId, FontSelection, Label, Response, Ui, WidgetText,
    text::{LayoutJob, TextFormat, TextWrapping},
};

const TOOLTIP_VIEWPORT_MARGIN: f32 = 24.0;

/// Empty single-line layout job that elides overflow with an ellipsis instead
/// of wrapping.
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
    let mut job = LayoutJob::single_section(
        flatten_newlines(text).into_owned(),
        TextFormat {
            font_id: font.clone(),
            color,
            ..Default::default()
        },
    );
    apply_single_line_wrapping(&mut job, max_width);
    job
}

/// Adds a single-line, width-bounded tooltip to `response`.
#[must_use]
pub(crate) fn stable_hover_text(response: Response, text: impl Into<WidgetText>) -> Response {
    stable_hover_text_lazy(response, || text)
}

/// Lazily builds a single-line, width-bounded tooltip for `response`.
#[must_use]
pub(crate) fn stable_hover_text_lazy<T>(response: Response, text: impl FnOnce() -> T) -> Response
where
    T: Into<WidgetText>,
{
    response.on_hover_ui(|ui| {
        single_line_tooltip(ui, text());
    })
}

/// Renders tooltip text without allowing last frame's tooltip size to feed
/// back into wrapping on the next frame.
pub(crate) fn single_line_tooltip(ui: &mut Ui, text: impl Into<WidgetText>) {
    let max_width = tooltip_max_width(ui.ctx().content_rect().width(), ui.spacing().tooltip_width);
    let mut job = Arc::unwrap_or_clone(text.into().into_layout_job(
        ui.style(),
        FontSelection::Default,
        ui.text_valign(),
    ));
    apply_single_line_constraints(&mut job, max_width);
    ui.set_max_width(max_width);
    let _ = ui.add(Label::new(job).show_tooltip_when_elided(false));
}

fn tooltip_max_width(content_width: f32, configured_width: f32) -> f32 {
    (content_width - TOOLTIP_VIEWPORT_MARGIN)
        .max(0.0)
        .min(configured_width.max(0.0))
}

fn apply_single_line_constraints(job: &mut LayoutJob, max_width: f32) {
    if let Cow::Owned(text) = flatten_newlines(&job.text) {
        // Replacing ASCII line breaks with ASCII spaces preserves byte-indexed
        // formatting ranges already present in the layout job.
        job.text = text;
    }
    apply_single_line_wrapping(job, max_width);
}

fn apply_single_line_wrapping(job: &mut LayoutJob, max_width: f32) {
    let constraints = single_line_job(max_width);
    job.break_on_newline = constraints.break_on_newline;
    job.wrap = constraints.wrap;
}

fn flatten_newlines(text: &str) -> Cow<'_, str> {
    if text.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
        Cow::Owned(
            text.chars()
                .map(|character| match character {
                    '\r' | '\n' => ' ',
                    other => other,
                })
                .collect(),
        )
    } else {
        Cow::Borrowed(text)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use egui::{AreaState, Color32, Event, FontId, Id, Pos2, RawInput, Rect, Tooltip, Vec2, text::TextFormat};

    use super::{
        apply_single_line_constraints, single_line_job, single_line_label_job, stable_hover_text_lazy,
        tooltip_max_width,
    };

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
        let job = single_line_label_job("first\nsecond", &font, color, 96.0);

        assert_eq!(job.text, "first second");
        assert!(!job.break_on_newline);
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].format.font_id, font);
        assert_eq!(job.sections[0].format.color, color);
    }

    #[test]
    fn tooltip_width_uses_stable_viewport_bounds() {
        assert!((tooltip_max_width(1_200.0, 600.0) - 600.0).abs() < f32::EPSILON);
        assert!((tooltip_max_width(320.0, 600.0) - 296.0).abs() < f32::EPSILON);
        assert!((tooltip_max_width(60.0, 600.0) - 36.0).abs() < f32::EPSILON);
        assert!((tooltip_max_width(1_200.0, 40.0) - 40.0).abs() < f32::EPSILON);
        assert!(tooltip_max_width(12.0, -4.0).abs() < f32::EPSILON);
    }

    #[test]
    fn tooltip_constraints_flatten_newlines_without_changing_section_ranges() {
        let mut job = egui::text::LayoutJob::single_section("first\r\nsecond".to_string(), TextFormat::default());
        let original_range = job.sections[0].byte_range.clone();

        apply_single_line_constraints(&mut job, 96.0);

        assert_eq!(job.text, "first  second");
        assert_eq!(job.sections[0].byte_range, original_range);
        assert!(!job.break_on_newline);
        assert_eq!(job.wrap.max_rows, 1);
        assert_eq!(job.wrap.overflow_character, Some('…'));
    }

    #[test]
    fn lazy_tooltip_text_is_not_built_without_hover() {
        let ctx = egui::Context::default();
        let built = Cell::new(false);

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = ui.label("target");
                let _ = stable_hover_text_lazy(response, || {
                    built.set(true);
                    "tooltip"
                });
            });
        });

        assert!(!built.get());
    }

    #[test]
    fn hovered_tooltip_width_stays_stable_across_frames() {
        let ctx = egui::Context::default();
        ctx.style_mut(|style| {
            style.interaction.tooltip_delay = 0.0;
            style.interaction.show_tooltips_only_when_still = false;
        });
        let target_id = Cell::new(None::<Id>);
        let target_rect = Cell::new(None::<Rect>);
        let build_count = Cell::new(0_usize);

        let run_frame = |time: f64, pointer: Option<Pos2>| {
            let events = pointer.map_or_else(Vec::new, |position| vec![Event::PointerMoved(position)]);
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 200.0))),
                time: Some(time),
                events,
                ..RawInput::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let response = ui.label("target");
                    target_id.set(Some(response.id));
                    target_rect.set(Some(response.rect));
                    let _ = stable_hover_text_lazy(response, || {
                        build_count.set(build_count.get() + 1);
                        "A long first line\nwith equally important update details that must remain visible"
                    });
                });
            });
        };

        run_frame(0.0, None);
        assert_eq!(build_count.get(), 0);
        let Some(pointer) = target_rect.get().map(|rect| rect.center()) else {
            panic!("target rectangle was not recorded");
        };
        let Some(tooltip_id) = target_id.get().map(|id| Tooltip::tooltip_id(id, 0)) else {
            panic!("target id was not recorded");
        };

        let mut widths = Vec::new();
        for frame in 1..=4 {
            run_frame(f64::from(frame), Some(pointer));
            let Some(size) = AreaState::load(&ctx, tooltip_id).and_then(|state| state.size) else {
                panic!("tooltip area was not recorded on frame {frame}");
            };
            widths.push(size.x);
        }

        assert_eq!(build_count.get(), widths.len());
        assert!(widths.windows(2).all(|pair| (pair[0] - pair[1]).abs() < f32::EPSILON));
    }
}
