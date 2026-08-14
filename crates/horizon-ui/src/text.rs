//! Shared single-line text layout helpers.

use std::{borrow::Cow, sync::Arc};

use egui::{
    Color32, Context, FontId, FontSelection, Galley, Label, Painter, Response, Ui, WidgetText,
    text::{LayoutJob, LayoutSection, TextFormat, TextWrapping},
};
use horizon_core::flatten_line_separators;

const TOOLTIP_MIN_WIDTH: f32 = 80.0;
const TOOLTIP_VIEWPORT_MARGIN: f32 = 24.0;
const MAX_SHAPING_VISIBLE_SCALARS: usize = 512;
const MAX_SHAPING_SCANNED_SCALARS: usize = 4_096;
const SHAPING_WIDTH_BUDGET_MULTIPLIER: f32 = 4.0;

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

/// Lays out painter text once so callers can use its exact glyph width and
/// reuse the same galley for painting.
pub(crate) fn painter_text_galley(
    painter: &Painter,
    text: &str,
    font: &FontId,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    let display_text = precut_text_for_shaping(painter.ctx(), text, font, max_width);
    painter.layout_job(single_line_label_job(display_text.as_ref(), font, color, max_width))
}

/// Conservatively bounds the prefix sent to the text shaper while keeping
/// zero-width scalars from consuming the visible-content budget.
pub(crate) fn precut_text_for_shaping<'a>(ctx: &Context, text: &'a str, font: &FontId, max_width: f32) -> Cow<'a, str> {
    if text.is_empty() || !max_width.is_finite() {
        return Cow::Borrowed(text);
    }

    let shaping_width_budget = max_width.max(0.0) * SHAPING_WIDTH_BUDGET_MULTIPLIER;
    let (cut_at, append_ellipsis) = ctx.fonts_mut(|fonts| {
        let mut measured_width = 0.0;
        let mut visible_scalars = 0;
        for (scanned_scalars, (byte_index, character)) in text.char_indices().enumerate() {
            if scanned_scalars == MAX_SHAPING_SCANNED_SCALARS {
                return (byte_index, true);
            }

            let glyph_width = fonts.glyph_width(font, character).max(0.0);
            if glyph_width == 0.0 {
                continue;
            }
            if visible_scalars == MAX_SHAPING_VISIBLE_SCALARS {
                return (byte_index, true);
            }
            if measured_width + glyph_width > shaping_width_budget {
                let cut_at = if visible_scalars == 0 {
                    byte_index + character.len_utf8()
                } else {
                    byte_index
                };
                return (cut_at, false);
            }
            measured_width += glyph_width;
            visible_scalars += 1;
        }
        (text.len(), false)
    });
    if append_ellipsis {
        let mut bounded = String::with_capacity(cut_at + '…'.len_utf8());
        bounded.push_str(&text[..cut_at]);
        bounded.push('…');
        Cow::Owned(bounded)
    } else {
        Cow::Borrowed(&text[..cut_at])
    }
}

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
    let mut job = Arc::unwrap_or_clone(WidgetText::from(text.as_ref()).into_layout_job(
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
    flatten_layout_job_newlines(job);
    job.break_on_newline = false;
    job.wrap = single_line_wrapping(max_width);
}

fn apply_wrapped_constraints(job: &mut LayoutJob, max_width: f32) {
    job.break_on_newline = true;
    job.wrap.max_width = max_width.max(0.0);
    job.wrap.max_rows = usize::MAX;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = None;
}

fn flatten_layout_job_newlines(job: &mut LayoutJob) {
    if !job.text.chars().any(is_line_separator) {
        return;
    }

    debug_assert!(job.sections.len() <= 1);
    let original = std::mem::take(&mut job.text);
    job.text.reserve(original.len());

    push_flattened_newlines(&mut job.text, &original);
    if let Some(section) = job.sections.first_mut() {
        section.byte_range = 0..job.text.len();
    }
}

fn push_flattened_newlines(output: &mut String, text: &str) {
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\r' {
            if characters.peek().is_some_and(|next| *next == '\n') {
                let _ = characters.next();
            }
            output.push(' ');
        } else if is_line_separator(character) {
            output.push(' ');
        } else {
            output.push(character);
        }
    }
}

fn is_line_separator(character: char) -> bool {
    matches!(
        character,
        '\r' | '\n' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use egui::{
        AreaState, Color32, Event, FontId, Id, Pos2, RawInput, Rect, Tooltip, Vec2,
        text::{LayoutJob, TextFormat},
    };

    use super::{
        append_single_line_text, apply_single_line_constraints, apply_wrapped_constraints, painter_text_galley,
        single_line_job, single_line_label_job, stable_hover_text_lazy, stable_wrapped_hover_text_lazy,
        tooltip_max_width,
    };

    fn assert_near(actual: f32, expected: f32) {
        assert!((actual - expected).abs() <= 0.001, "expected {expected}, got {actual}");
    }

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

    #[test]
    fn painter_text_galley_uses_actual_glyph_metrics() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let narrow_width = Cell::new(0.0_f32);
        let wide_width = Cell::new(0.0_f32);

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                narrow_width.set(
                    painter_text_galley(
                        ui.painter(),
                        "iii",
                        &FontId::proportional(12.0),
                        Color32::WHITE,
                        f32::INFINITY,
                    )
                    .size()
                    .x,
                );
                wide_width.set(
                    painter_text_galley(
                        ui.painter(),
                        "WWW",
                        &FontId::proportional(12.0),
                        Color32::WHITE,
                        f32::INFINITY,
                    )
                    .size()
                    .x,
                );
            });
        });

        assert!(
            wide_width.get() > narrow_width.get(),
            "glyph widths were not measured: narrow={}, wide={}",
            narrow_width.get(),
            wide_width.get()
        );
    }

    #[test]
    fn painter_text_galley_precuts_pathological_invisible_prefixes() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let source = format!("{}visible suffix", "\u{200B}".repeat(4_100));
        let job_text = RefCell::new(String::new());

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                *job_text.borrow_mut() =
                    painter_text_galley(ui.painter(), &source, &FontId::proportional(12.0), Color32::WHITE, 80.0)
                        .job
                        .text
                        .clone();
            });
        });

        assert!(job_text.borrow().ends_with('…'));
        assert!(!job_text.borrow().contains("visible suffix"));
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
    fn wrapped_tooltip_constraints_preserve_lines_and_wrap_at_words() {
        let mut job =
            LayoutJob::simple_singleline("first line\nsecond line".to_string(), FontId::default(), Color32::WHITE);

        apply_wrapped_constraints(&mut job, 96.0);

        assert!(job.break_on_newline);
        assert_ne!(job.wrap.max_rows, 1);
        assert!(!job.wrap.break_anywhere);
        assert_eq!(job.wrap.overflow_character, None);
        assert_near(job.wrap.max_width, 96.0);
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
