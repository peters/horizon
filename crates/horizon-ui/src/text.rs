//! Shared single-line text layout helpers.

use std::sync::Arc;

use egui::{
    Color32, FontId, FontSelection, Galley, Label, Painter, Response, Ui, WidgetText,
    text::{LayoutJob, LayoutSection, TextFormat, TextWrapping},
};

const TOOLTIP_MIN_WIDTH: f32 = 80.0;
const TOOLTIP_VIEWPORT_MARGIN: f32 = 24.0;

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
    if text.chars().any(is_line_separator) {
        push_flattened_newlines(&mut job.text, text);
    } else {
        job.text.push_str(text);
    }
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
pub(crate) fn painter_text_galley(painter: &Painter, text: &str, font: FontId, color: Color32) -> Arc<Galley> {
    painter.layout_no_wrap(text.to_owned(), font, color)
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

/// Lazily builds a width-bounded tooltip that preserves deliberate line
/// breaks and wraps all content instead of eliding it.
#[must_use]
pub(crate) fn stable_wrapped_hover_text_lazy<T>(response: Response, text: impl FnOnce() -> T) -> Response
where
    T: Into<WidgetText>,
{
    response.on_hover_ui(|ui| {
        wrapped_tooltip(ui, text());
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
    show_tooltip_job(ui, job);
}

fn wrapped_tooltip(ui: &mut Ui, text: impl Into<WidgetText>) {
    let max_width = tooltip_max_width(ui.ctx().content_rect().width(), ui.spacing().tooltip_width);
    let mut job = Arc::unwrap_or_clone(text.into().into_layout_job(
        ui.style(),
        FontSelection::Default,
        ui.text_valign(),
    ));
    apply_wrapped_constraints(&mut job, max_width);
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

    let original = std::mem::take(&mut job.text);
    let mut offsets = vec![0_usize; original.len() + 1];
    job.text.reserve(original.len());

    let mut characters = original.char_indices().peekable();
    while let Some((byte_index, character)) = characters.next() {
        offsets[byte_index] = job.text.len();
        if character == '\r' {
            job.text.push(' ');
            let end = byte_index + character.len_utf8();
            offsets[end] = job.text.len();
            if characters.peek().is_some_and(|(_, next)| *next == '\n')
                && let Some((line_feed_index, line_feed)) = characters.next()
            {
                offsets[line_feed_index] = job.text.len();
                offsets[line_feed_index + line_feed.len_utf8()] = job.text.len();
            }
        } else {
            if is_line_separator(character) {
                job.text.push(' ');
            } else {
                job.text.push(character);
            }
            offsets[byte_index + character.len_utf8()] = job.text.len();
        }
    }

    for section in &mut job.sections {
        section.byte_range = offsets[section.byte_range.start]..offsets[section.byte_range.end];
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
    matches!(character, '\r' | '\n' | '\u{0085}' | '\u{2028}' | '\u{2029}')
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

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
        let narrow_width = Cell::new(0.0_f32);
        let wide_width = Cell::new(0.0_f32);

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                narrow_width.set(
                    painter_text_galley(ui.painter(), "iii", FontId::proportional(12.0), Color32::WHITE)
                        .size()
                        .x,
                );
                wide_width.set(
                    painter_text_galley(ui.painter(), "WWW", FontId::proportional(12.0), Color32::WHITE)
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
    fn tooltip_width_uses_stable_viewport_bounds() {
        assert_near(tooltip_max_width(1_200.0, 600.0), 600.0);
        assert_near(tooltip_max_width(320.0, 600.0), 296.0);
        assert_near(tooltip_max_width(60.0, 600.0), 80.0);
        assert_near(tooltip_max_width(1_200.0, 40.0), 40.0);
        assert_near(tooltip_max_width(12.0, -4.0), 0.0);
    }

    #[test]
    fn tooltip_constraints_flatten_newlines_and_remap_section_ranges() {
        let mut job = LayoutJob::default();
        job.append("blå\r", 0.0, TextFormat::default());
        job.append("\n二\u{2028}third", 0.0, TextFormat::default());

        apply_single_line_constraints(&mut job, 96.0);

        assert_eq!(job.text, "blå 二 third");
        assert_eq!(job.sections[0].byte_range, 0.."blå ".len());
        assert_eq!(job.sections[1].byte_range, "blå ".len()..job.text.len());
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
