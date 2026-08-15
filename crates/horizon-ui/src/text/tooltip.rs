use std::borrow::Cow;

use egui::{FontSelection, Label, Response, Ui, WidgetText, text::LayoutJob};
use horizon_core::{flatten_and_truncate_chars, normalize_wrapped_line_separators};

use super::single_line_wrapping;

const TOOLTIP_MIN_WIDTH: f32 = 80.0;
const TOOLTIP_VIEWPORT_MARGIN: f32 = 24.0;
const MAX_TOOLTIP_SCALARS: usize = 4_096;

/// Adds a single-line, width-bounded tooltip to `response`.
#[must_use]
pub(crate) fn stable_hover_text(response: Response, text: impl AsRef<str>) -> Response {
    stable_hover_text_lazy(response, || text)
}

/// Adds a width-bounded tooltip that preserves and wraps all content.
#[must_use]
pub(crate) fn stable_wrapped_hover_text(response: Response, text: impl AsRef<str>) -> Response {
    stable_wrapped_hover_text_lazy(response, || text)
}

/// Lazily builds a single-line, width-bounded tooltip for `response`.
#[must_use]
pub(crate) fn stable_hover_text_lazy<T>(response: Response, text: impl FnOnce() -> T) -> Response
where
    T: AsRef<str>,
{
    response.on_hover_ui(|ui| {
        single_line_tooltip(ui, text());
    })
}

/// Lazily builds a width-bounded tooltip that preserves deliberate line
/// breaks and wraps all content instead of eliding it.
#[must_use]
pub(crate) fn stable_wrapped_hover_text_lazy<T>(response: Response, text: impl FnOnce() -> T) -> Response
where
    T: AsRef<str>,
{
    response.on_hover_ui(|ui| {
        wrapped_tooltip(ui, text());
    })
}

/// Renders tooltip text without allowing last frame's tooltip size to feed
/// back into wrapping on the next frame.
pub(crate) fn single_line_tooltip(ui: &mut Ui, text: impl AsRef<str>) {
    constrained_tooltip(ui, text, apply_single_line_constraints);
}

fn wrapped_tooltip(ui: &mut Ui, text: impl AsRef<str>) {
    constrained_tooltip(ui, text, apply_wrapped_constraints);
}

fn constrained_tooltip(ui: &mut Ui, text: impl AsRef<str>, apply_constraints: impl FnOnce(&mut LayoutJob, f32)) {
    let max_width = tooltip_max_width(ui.ctx().content_rect().width(), ui.spacing().tooltip_width);
    let mut job = std::sync::Arc::unwrap_or_clone(WidgetText::from(text.as_ref()).into_layout_job(
        ui.style(),
        FontSelection::Default,
        ui.text_valign(),
    ));
    apply_constraints(&mut job, max_width);
    show_tooltip_job(ui, job);
}

fn show_tooltip_job(ui: &mut Ui, job: LayoutJob) {
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let _ = ui.add(Label::new(WidgetText::Galley(galley)).show_tooltip_when_elided(false));
}

fn tooltip_max_width(content_width: f32, configured_width: f32) -> f32 {
    let maximum = configured_width.max(0.0);
    let minimum = TOOLTIP_MIN_WIDTH.min(maximum);
    (content_width - TOOLTIP_VIEWPORT_MARGIN).clamp(minimum, maximum)
}

fn apply_single_line_constraints(job: &mut LayoutJob, max_width: f32) {
    normalize_layout_job_text(job, flatten_and_truncate_chars);
    job.break_on_newline = false;
    job.wrap = single_line_wrapping(max_width);
}

fn apply_wrapped_constraints(job: &mut LayoutJob, max_width: f32) {
    normalize_layout_job_text(job, normalize_wrapped_line_separators);
    job.break_on_newline = true;
    job.wrap.max_width = max_width.max(0.0);
    job.wrap.max_rows = usize::MAX;
    // egui prefers word boundaries when this is false, then falls back to the
    // last fitting glyph so an individual overlong token still hard-wraps.
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = None;
}

fn normalize_layout_job_text(job: &mut LayoutJob, normalize: for<'a> fn(&'a str, usize) -> Cow<'a, str>) {
    let original = std::mem::take(&mut job.text);
    let normalized = normalize(&original, MAX_TOOLTIP_SCALARS);
    let normalized_len = normalized.len();
    let ranges = job
        .sections
        .iter()
        .map(|section| {
            let start = normalize(&original[..section.byte_range.start], MAX_TOOLTIP_SCALARS)
                .len()
                .min(normalized_len);
            let end = normalize(&original[..section.byte_range.end], MAX_TOOLTIP_SCALARS)
                .len()
                .min(normalized_len);
            start..end
        })
        .collect::<Vec<_>>();
    job.text = match normalized {
        Cow::Borrowed(_) => original,
        Cow::Owned(normalized) => normalized,
    };
    for (section, range) in job.sections.iter_mut().zip(ranges) {
        section.byte_range = range;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use egui::{
        AreaState, Color32, Event, FontId, Id, Pos2, RawInput, Rect, Tooltip, Vec2,
        text::{LayoutJob, LayoutSection, TextFormat},
    };

    use super::{
        MAX_TOOLTIP_SCALARS, apply_single_line_constraints, apply_wrapped_constraints, stable_hover_text_lazy,
        stable_wrapped_hover_text_lazy, tooltip_max_width,
    };

    fn assert_near(actual: f32, expected: f32) {
        assert!((actual - expected).abs() <= 0.001, "expected {expected}, got {actual}");
    }

    #[test]
    fn tooltip_width_uses_stable_viewport_bounds() {
        assert_near(tooltip_max_width(1_200.0, 600.0), 600.0);
        assert_near(tooltip_max_width(320.0, 600.0), 296.0);
        assert_near(tooltip_max_width(60.0, 600.0), 80.0);
        assert_near(tooltip_max_width(1_200.0, 40.0), 40.0);
        assert_near(tooltip_max_width(12.0, -4.0), 0.0);
    }

    #[test]
    fn tooltip_constraints_flatten_newlines_in_the_single_text_section() {
        let mut job =
            LayoutJob::simple_singleline("blå\r\n二\u{2028}third".to_string(), FontId::default(), Color32::WHITE);

        apply_single_line_constraints(&mut job, 96.0);

        assert_eq!(job.text, "blå 二 third");
        assert_eq!(job.sections[0].byte_range, 0..job.text.len());
        assert!(!job.break_on_newline);
        assert_eq!(job.wrap.max_rows, 1);
        assert_eq!(job.wrap.overflow_character, Some('…'));
    }

    #[test]
    fn multi_section_tooltip_constraints_flatten_and_remap_section_ranges() {
        let mut job = LayoutJob {
            text: "a\r\nb".to_string(),
            sections: vec![
                LayoutSection {
                    byte_range: 0..1,
                    leading_space: 0.0,
                    format: TextFormat::default(),
                },
                LayoutSection {
                    byte_range: 1..4,
                    leading_space: 0.0,
                    format: TextFormat::default(),
                },
            ],
            ..Default::default()
        };

        apply_single_line_constraints(&mut job, 96.0);

        assert_eq!(job.text, "a b");
        assert_eq!(job.sections[0].byte_range, 0..1);
        assert_eq!(job.sections[1].byte_range, 1..3);
        let ctx = egui::Context::default();
        let _ = ctx.run(RawInput::default(), |ctx| {
            let _ = ctx.fonts_mut(|fonts| fonts.layout_job(job.clone()));
        });
    }

    #[test]
    fn wrapped_tooltip_constraints_preserve_lines_and_wrap_at_words() {
        let mut job = LayoutJob::simple_singleline(
            "first line\r\nsecond line\u{2028}third line".to_string(),
            FontId::default(),
            Color32::WHITE,
        );

        apply_wrapped_constraints(&mut job, 96.0);

        assert_eq!(job.text, "first line\nsecond line\nthird line");
        assert!(job.break_on_newline);
        assert_ne!(job.wrap.max_rows, 1);
        assert!(!job.wrap.break_anywhere);
        assert_eq!(job.wrap.overflow_character, None);
        assert_near(job.wrap.max_width, 96.0);
    }

    #[test]
    fn tooltip_constraints_cap_text_before_glyph_shaping() {
        let mut single = LayoutJob::simple_singleline("x".repeat(8_192), FontId::default(), Color32::WHITE);
        apply_single_line_constraints(&mut single, 96.0);
        assert_eq!(single.text.chars().count(), MAX_TOOLTIP_SCALARS);
        assert!(single.text.ends_with('…'));

        let mut wrapped = LayoutJob::simple_singleline("x".repeat(8_192), FontId::default(), Color32::WHITE);
        apply_wrapped_constraints(&mut wrapped, 96.0);
        assert_eq!(wrapped.text.chars().count(), MAX_TOOLTIP_SCALARS);
        assert!(wrapped.text.ends_with('…'));
    }

    #[test]
    fn wrapped_tooltip_layout_breaks_between_words() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let rows = std::cell::RefCell::new(Vec::<String>::new());

        let _ = ctx.run(RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut job = LayoutJob::simple_singleline(
                    "alpha beta gamma".to_string(),
                    FontId::proportional(14.0),
                    Color32::WHITE,
                );
                apply_wrapped_constraints(&mut job, 50.0);
                let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                rows.replace(galley.rows.iter().map(|row| row.text()).collect());
            });
        });

        let rows = rows.borrow();
        let normalized: Vec<_> = rows.iter().map(|row| row.trim()).collect();
        assert_eq!(normalized, ["alpha", "beta", "gamma"]);
    }

    #[test]
    fn wrapped_tooltip_layout_hard_wraps_an_overlong_token() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let row_widths = std::cell::RefCell::new(Vec::<f32>::new());

        let _ = ctx.run(RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut job = LayoutJob::simple_singleline("x".repeat(128), FontId::proportional(14.0), Color32::WHITE);
                apply_wrapped_constraints(&mut job, 50.0);
                let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                row_widths.replace(galley.rows.iter().map(|row| row.rect().width()).collect());
            });
        });

        let row_widths = row_widths.borrow();
        assert!(row_widths.len() > 1);
        assert!(row_widths.iter().all(|width| *width <= 50.0));
    }

    #[test]
    fn lazy_tooltip_text_is_not_built_without_hover() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
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

    fn hovered_tooltip_sizes(wrapped: bool) -> (Vec<Vec2>, usize) {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
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
                    if wrapped {
                        let _ = stable_wrapped_hover_text_lazy(response, || {
                            build_count.set(build_count.get() + 1);
                            concat!(
                                "line one\nline two\nline three\n",
                                "line four contains a deliberately long diagnostic message that must wrap ",
                                "at word boundaries while preserving every user-visible failure detail"
                            )
                        });
                    } else {
                        let _ = stable_hover_text_lazy(response, || {
                            build_count.set(build_count.get() + 1);
                            "A long first line\nwith equally important update details"
                        });
                    }
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

        let mut sizes = Vec::new();
        for frame in 1..=4 {
            run_frame(f64::from(frame), Some(pointer));
            let Some(size) = AreaState::load(&ctx, tooltip_id).and_then(|state| state.size) else {
                panic!("tooltip area was not recorded on frame {frame}");
            };
            sizes.push(size);
        }

        assert_eq!(build_count.get(), sizes.len());
        (sizes, build_count.get())
    }

    fn assert_stable_tooltip_widths(sizes: &[Vec2]) {
        const WIDTH_TOLERANCE: f32 = 0.5;
        assert!(
            sizes
                .windows(2)
                .all(|pair| (pair[0].x - pair[1].x).abs() <= WIDTH_TOLERANCE),
            "tooltip width drifted across frames: {sizes:?}"
        );
    }

    #[test]
    fn hovered_single_line_tooltip_width_stays_stable_across_frames() {
        let (sizes, build_count) = hovered_tooltip_sizes(false);

        assert_eq!(build_count, 4);
        assert!(
            sizes.iter().all(|size| size.y < 40.0),
            "single-line tooltip unexpectedly wrapped: {sizes:?}"
        );
        assert_stable_tooltip_widths(&sizes);
    }

    #[test]
    fn hovered_wrapped_tooltip_preserves_rows_and_stays_stable() {
        let (sizes, build_count) = hovered_tooltip_sizes(true);

        assert_eq!(build_count, 4);
        assert!(
            sizes.iter().all(|size| size.y > 60.0),
            "wrapped rows were lost: {sizes:?}"
        );
        assert_stable_tooltip_widths(&sizes);
    }
}
