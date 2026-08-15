use std::{borrow::Cow, sync::Arc};

use egui::{
    Color32, Context, FontId, Galley, Painter,
    epaint::text::FontsView,
    text::{LayoutJob, TextFormat},
};
use horizon_core::{flatten_line_separators, is_line_separator};

use super::{append_flattened_single_line_text, single_line_job, single_line_label_job_from_flattened};

const MAX_UNSCANNED_TEXT_BYTES: usize = 512;
const MAX_SHAPING_VISIBLE_SCALARS: usize = 512;
const MAX_SHAPING_SCANNED_SCALARS: usize = 4_096;
const MAX_SHAPING_SECTIONS: usize = 512;
const SHAPING_WIDTH_BUDGET_MULTIPLIER: f32 = 4.0;

struct ShapingBudget {
    width: f32,
    measured_width: f32,
    scanned_scalars: usize,
    visible_scalars: usize,
    capped: bool,
}

impl ShapingBudget {
    fn new(max_width: f32) -> Self {
        let width = if max_width.is_nan() {
            0.0
        } else {
            max_width.max(0.0) * SHAPING_WIDTH_BUDGET_MULTIPLIER
        };
        Self {
            width,
            measured_width: 0.0,
            scanned_scalars: 0,
            visible_scalars: 0,
            capped: false,
        }
    }

    fn flatten_and_bound<'a>(&mut self, text: &'a str, mut glyph_width: impl FnMut(char) -> f32) -> Cow<'a, str> {
        let mut flattened = None;
        let mut characters = text.char_indices().peekable();
        let mut cut_at = None;
        let mut append_ellipsis = false;

        while let Some((byte_index, character)) = characters.next() {
            if self.scanned_scalars == MAX_SHAPING_SCANNED_SCALARS {
                cut_at = Some(byte_index);
                append_ellipsis = true;
                self.capped = true;
                break;
            }
            self.scanned_scalars += 1;

            let mut source_end = byte_index + character.len_utf8();
            let separator = is_line_separator(character);
            let normalized = if separator { ' ' } else { character };
            if character == '\r'
                && characters.peek().is_some_and(|(_, next)| *next == '\n')
                && self.scanned_scalars < MAX_SHAPING_SCANNED_SCALARS
                && let Some((next_index, next)) = characters.next()
            {
                self.scanned_scalars += 1;
                source_end = next_index + next.len_utf8();
            }

            let width = glyph_width(normalized).max(0.0);
            if width > 0.0 {
                if self.visible_scalars == MAX_SHAPING_VISIBLE_SCALARS {
                    cut_at = Some(byte_index);
                    append_ellipsis = true;
                    self.capped = true;
                    break;
                }
                if self.measured_width + width > self.width && self.visible_scalars > 0 {
                    cut_at = Some(byte_index);
                    self.capped = true;
                    break;
                }
            }

            if separator {
                let output = flattened.get_or_insert_with(|| {
                    let capacity = text.len().min(MAX_SHAPING_SCANNED_SCALARS * 4 + '…'.len_utf8());
                    let mut output = String::with_capacity(capacity);
                    output.push_str(&text[..byte_index]);
                    output
                });
                output.push(normalized);
            } else if let Some(output) = flattened.as_mut() {
                output.push(character);
            }

            if width > 0.0 {
                let exceeds_width = self.measured_width + width > self.width;
                self.measured_width += width;
                self.visible_scalars += 1;
                if exceeds_width {
                    cut_at = Some(source_end);
                    self.capped = true;
                    break;
                }
            }
        }

        if let Some(mut output) = flattened {
            if append_ellipsis {
                output.push('…');
            }
            Cow::Owned(output)
        } else {
            let cut_at = cut_at.unwrap_or(text.len());
            if append_ellipsis {
                let mut output = String::with_capacity(cut_at + '…'.len_utf8());
                output.push_str(&text[..cut_at]);
                output.push('…');
                Cow::Owned(output)
            } else {
                Cow::Borrowed(&text[..cut_at])
            }
        }
    }
}

/// Builds a styled single-line job while enforcing one shaping budget across
/// every section appended to it.
pub(crate) struct BoundedSingleLineJob {
    job: LayoutJob,
    budget: ShapingBudget,
    sections: usize,
}

impl BoundedSingleLineJob {
    pub(crate) fn new(max_width: f32) -> Self {
        Self {
            job: single_line_job(max_width),
            budget: ShapingBudget::new(max_width),
            sections: 0,
        }
    }

    /// Appends a styled section. Returns `false` once later sections can no
    /// longer contribute visible text within the shared shaping budget.
    pub(crate) fn append(
        &mut self,
        fonts: &mut FontsView<'_>,
        text: &str,
        leading_space: f32,
        format: TextFormat,
    ) -> bool {
        if self.budget.capped {
            return false;
        }
        if text.is_empty() {
            return true;
        }
        if self.sections == MAX_SHAPING_SECTIONS {
            self.budget.capped = true;
            if !self.job.text.ends_with('…') {
                self.job.text.push('…');
                if let Some(section) = self.job.sections.last_mut() {
                    section.byte_range.end = self.job.text.len();
                }
            }
            return false;
        }
        self.sections += 1;

        let budget = &mut self.budget;
        let bounded = budget.flatten_and_bound(text, |character| fonts.glyph_width(&format.font_id, character));
        if !bounded.is_empty() {
            append_flattened_single_line_text(&mut self.job, bounded.as_ref(), leading_space, format);
        }
        !self.budget.capped
    }

    pub(crate) fn was_capped(&self) -> bool {
        self.budget.capped
    }

    pub(crate) fn finish(self) -> LayoutJob {
        self.job
    }
}

/// Lays out the bounded display text once so callers can reuse the painted
/// galley and its exact post-cap glyph width. Pathological inputs may be
/// shortened even when `max_width` is infinite.
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
    let display_text = bounded_flattened_text_for_shaping(ctx, text, font, max_width);
    single_line_label_job_from_flattened(display_text.as_ref(), font, color, max_width)
}

/// Conservatively bounds the prefix sent to the text shaper while keeping
/// zero-width scalars from consuming the visible-content budget. Long input
/// is normalized during the bounded scan so unseen suffixes are never visited.
fn bounded_flattened_text_for_shaping<'a>(ctx: &Context, text: &'a str, font: &FontId, max_width: f32) -> Cow<'a, str> {
    if text.is_empty() || text.len() <= MAX_UNSCANNED_TEXT_BYTES {
        return flatten_line_separators(text);
    }

    let mut budget = ShapingBudget::new(max_width);
    ctx.fonts_mut(|fonts| budget.flatten_and_bound(text, |character| fonts.glyph_width(font, character)))
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use egui::{Color32, FontId, text::TextFormat};

    use super::{BoundedSingleLineJob, MAX_SHAPING_SECTIONS, ShapingBudget, context_text_galley, painter_text_galley};

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
    fn separator_normalization_stops_at_the_original_input_scan_cap() {
        let source = format!("{}unvisited suffix", "\r\n".repeat(4_096));
        let measured_characters = Cell::new(0_usize);
        let mut budget = ShapingBudget::new(f32::INFINITY);

        let bounded = budget.flatten_and_bound(&source, |_| {
            measured_characters.set(measured_characters.get() + 1);
            0.0
        });

        assert!(budget.capped);
        assert_eq!(budget.scanned_scalars, 4_096);
        assert_eq!(measured_characters.get(), 2_048);
        assert_eq!(bounded.chars().count(), 2_049);
        assert!(bounded.chars().take(2_048).all(|character| character == ' '));
        assert!(bounded.ends_with('…'));
        assert!(!bounded.contains("unvisited suffix"));
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

    #[test]
    fn styled_sections_share_one_visible_scalar_budget() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let mut job_text = String::new();

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            job_text = ctx.fonts_mut(|fonts| {
                let mut job = BoundedSingleLineJob::new(f32::INFINITY);
                assert!(job.append(
                    fonts,
                    &"a".repeat(400),
                    0.0,
                    TextFormat {
                        font_id: FontId::proportional(12.0),
                        color: Color32::WHITE,
                        ..Default::default()
                    },
                ));
                assert!(!job.append(
                    fonts,
                    &"b".repeat(400),
                    0.0,
                    TextFormat {
                        font_id: FontId::monospace(10.0),
                        color: Color32::GRAY,
                        ..Default::default()
                    },
                ));
                job.finish().text
            });
        });

        assert_eq!(job_text.chars().count(), 513);
        assert!(job_text.ends_with('…'));
    }

    #[test]
    fn empty_sections_do_not_consume_the_section_budget() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let mut job = None;

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            job = Some(ctx.fonts_mut(|fonts| {
                let mut builder = BoundedSingleLineJob::new(f32::INFINITY);
                let format = TextFormat {
                    font_id: FontId::proportional(12.0),
                    color: Color32::WHITE,
                    ..Default::default()
                };
                for _ in 0..600 {
                    assert!(builder.append(fonts, "", 0.0, format.clone()));
                }
                assert!(builder.append(fonts, "visible", 0.0, format));
                builder.finish()
            }));
        });

        let Some(job) = job else {
            panic!("bounded job was not built");
        };
        assert_eq!(job.text, "visible");
        assert_eq!(job.sections.len(), 1);
    }

    #[test]
    fn section_budget_marks_omitted_sections_with_an_ellipsis() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let mut job = None;

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            job = Some(ctx.fonts_mut(|fonts| {
                let mut builder = BoundedSingleLineJob::new(f32::INFINITY);
                let format = TextFormat {
                    font_id: FontId::proportional(12.0),
                    color: Color32::WHITE,
                    ..Default::default()
                };
                for _ in 0..MAX_SHAPING_SECTIONS {
                    assert!(builder.append(fonts, "x", 0.0, format.clone()));
                }
                assert!(!builder.append(fonts, "omitted", 0.0, format));
                assert!(builder.was_capped());
                builder.finish()
            }));
        });

        let Some(job) = job else {
            panic!("bounded job was not built");
        };
        assert!(job.text.ends_with('…'));
        assert_eq!(job.sections.len(), MAX_SHAPING_SECTIONS);
        assert_eq!(
            job.sections.last().map(|section| section.byte_range.end),
            Some(job.text.len())
        );
    }
}
