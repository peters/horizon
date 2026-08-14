use std::{borrow::Cow, sync::Arc};

use egui::{Color32, Context, FontId, Galley, Painter, Pos2, Rect, Ui, Vec2, text::LayoutJob};
use horizon_core::flatten_line_separators;

use super::single_line_label_job;

const MAX_UNSCANNED_TEXT_BYTES: usize = 512;
const MAX_SHAPING_VISIBLE_SCALARS: usize = 512;
const MAX_SHAPING_SCANNED_SCALARS: usize = 4_096;
const SHAPING_WIDTH_BUDGET_MULTIPLIER: f32 = 4.0;

/// Lays out painter text once so callers can use its exact glyph width and
/// reuse the same galley for painting.
pub(crate) fn painter_text_galley(
    painter: &Painter,
    text: &str,
    font: &FontId,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    painter.layout_job(bounded_single_line_job(painter.ctx(), text, font, color, max_width))
}

/// Context variant for layout paths that do not yet have a painter.
pub(crate) fn context_text_galley(
    ctx: &Context,
    text: &str,
    font: &FontId,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    let job = bounded_single_line_job(ctx, text, font, color, max_width);
    ctx.fonts_mut(|fonts| fonts.layout_job(job))
}

fn bounded_single_line_job(ctx: &Context, text: &str, font: &FontId, color: Color32, max_width: f32) -> LayoutJob {
    let flattened = flatten_line_separators(text);
    let display_text = precut_text_for_shaping(ctx, flattened.as_ref(), font, max_width);
    single_line_label_job(display_text.as_ref(), font, color, max_width)
}

/// Conservatively bounds the prefix sent to the text shaper while keeping
/// zero-width scalars from consuming the visible-content budget.
fn precut_text_for_shaping<'a>(ctx: &Context, text: &'a str, font: &FontId, max_width: f32) -> Cow<'a, str> {
    if text.is_empty() || text.len() <= MAX_UNSCANNED_TEXT_BYTES {
        return Cow::Borrowed(text);
    }

    let shaping_width_budget = if max_width.is_nan() {
        0.0
    } else {
        max_width.max(0.0) * SHAPING_WIDTH_BUDGET_MULTIPLIER
    };
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

pub(crate) fn paint_section_header(ui: &mut Ui, width: f32, height: f32, title: &str, font: &FontId, color: Color32) {
    let rect = ui.allocate_space(Vec2::new(width, height)).1;
    let painter = ui.painter_at(rect);
    let text_x = rect.min.x + 4.0;
    let max_x = (rect.max.x - 4.0).max(text_x);
    let galley = painter_text_galley(&painter, title, font, color, (max_x - text_x).max(0.0));
    let text_rect = Rect::from_min_max(Pos2::new(text_x, rect.min.y), Pos2::new(max_x, rect.max.y));
    painter.with_clip_rect(text_rect).galley(
        Pos2::new(text_x, rect.center().y - galley.size().y * 0.5),
        galley,
        Color32::TRANSPARENT,
    );
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use egui::{Color32, FontId};

    use super::{context_text_galley, painter_text_galley};

    fn context_job_text(source: &str, max_width: f32) -> String {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let job_text = RefCell::new(String::new());
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            *job_text.borrow_mut() =
                context_text_galley(ctx, source, &FontId::proportional(12.0), Color32::WHITE, max_width)
                    .job
                    .text
                    .clone();
        });
        job_text.into_inner()
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
    fn separator_runs_are_flattened_before_the_shaping_budget_is_measured() {
        let source = format!("{}visible suffix", "\n".repeat(600));
        let job_text = context_job_text(&source, 40.0);

        assert!(!job_text.contains('\n'));
        assert!(!job_text.contains("visible suffix"));
    }

    #[test]
    fn infinite_width_still_caps_visible_shaping_work() {
        let source = "a".repeat(600);
        let job_text = context_job_text(&source, f32::INFINITY);

        assert!(job_text.ends_with('…'));
        assert!(job_text.chars().count() <= 513);
    }

    #[test]
    fn nan_width_does_not_bypass_the_shaping_guard() {
        let source = "a".repeat(600);
        let job_text = context_job_text(&source, f32::NAN);

        assert!(job_text.chars().count() <= 1);
    }
}
