//! Text that the canvas zoom draws, made sharp at every zoom.
//!
//! The canvas is one egui layer under a scale transform (`Context::set_transform_layer`,
//! scale = the canvas zoom). A text shape under a layer transform keeps the glyph
//! pictures that were made for scale 1 and only scales their rectangles, so the
//! GPU stretches a finished picture of every letter: at a zoom of 0.94 each letter
//! is resampled and its stems land on different pixel fractions.
//!
//! This module lays text out at the size it has on the screen, and hands the
//! painter a galley whose glyph offsets are divided by the zoom of the size the
//! pictures were made for. The layer then multiplies them back, so one glyph texel
//! meets one pixel at rest. The layout of a panel never moves: every size, wrap
//! width and line height is the value of zoom 1 multiplied by one factor, and
//! because font advances grow linearly with the font size, a paragraph breaks at
//! the words of zoom 1 at every zoom.
//!
//! The numbers are [`SCREEN_SIZE_QUANTUM`] (the screen sizes are quarter points),
//! [`SETTLE`] (a gesture keeps the pictures it started with for this long) and
//! [`MAX_SCREEN_SIZES`] (the ladder that the state keeps).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::emath::TSTransform;
use egui::epaint::TextShape;
use egui::epaint::text::FontsView;
use egui::text::{ByteIndex, LayoutJob, LayoutSection, TextFormat, TextWrapping};
use egui::{Align, Align2, Color32, Context, FontId, Galley, Id, Painter, Pos2, Rect, Shape, ViewportId, WidgetText};

/// The step of the ladder of screen font sizes, in points.
pub(crate) const SCREEN_SIZE_QUANTUM: f32 = 0.25;
/// How long the zoom must stand still before the glyph pictures are made for it.
pub(crate) const SETTLE: Duration = Duration::from_millis(120);
/// The most screen sizes whose glyph pictures the state keeps alive.
pub(crate) const MAX_SCREEN_SIZES: usize = 64;
/// The least screen size that is rasterised, in points. Below it no letter is
/// readable, and the pictures would only fill the atlas.
const MIN_SCREEN_SIZE: f32 = 3.0;
/// The largest screen size that is rasterised, in points.
const MAX_SCREEN_SIZE: f32 = 96.0;
/// A change of the layer scale smaller than this is the same zoom.
const SCALE_TOLERANCE: f32 = 1.0e-3;

/// The screen sizes of one viewport, and the zoom whose pictures are in use.
#[derive(Clone, Default)]
struct ZoomState {
    /// The zoom whose glyph pictures are in use; 0.0 before the first frame.
    glyph_zoom: f32,
    /// When the requested zoom last differed from the glyph zoom.
    changed_at: Option<Instant>,
    /// The screen sizes that are alive, oldest use first.
    sizes: Vec<ScreenSize>,
    /// The counter that says which size was used last.
    uses: u64,
}

#[derive(Clone, Copy)]
struct ScreenSize {
    size: f32,
    used: u64,
}

impl ZoomState {
    /// The zoom whose glyph pictures this frame uses, and how long the caller
    /// must wait for the pictures of the new zoom if it needs one.
    fn observe(&mut self, requested: f32, now: Instant) -> (f32, Option<Duration>) {
        if self.glyph_zoom <= 0.0 {
            self.glyph_zoom = requested;
            return (self.glyph_zoom, None);
        }
        if (self.glyph_zoom - requested).abs() <= SCALE_TOLERANCE {
            self.changed_at = None;
            return (self.glyph_zoom, None);
        }
        match self.changed_at {
            None => {
                self.changed_at = Some(now);
                (self.glyph_zoom, Some(SETTLE))
            }
            Some(changed_at) if now.saturating_duration_since(changed_at) >= SETTLE => {
                self.glyph_zoom = requested;
                self.changed_at = None;
                (self.glyph_zoom, None)
            }
            Some(_) => (self.glyph_zoom, None),
        }
    }

    /// The screen size of `logical` at the glyph zoom that is in use.
    fn screen_size(&mut self, logical: f32, glyph_zoom: f32) -> f32 {
        let wanted = quantise(logical * glyph_zoom);
        if wanted <= 0.0 {
            return wanted;
        }
        self.uses += 1;
        if let Some(entry) = self
            .sizes
            .iter_mut()
            .find(|entry| (entry.size - wanted).abs() < f32::EPSILON)
        {
            entry.used = self.uses;
            return wanted;
        }
        self.sizes.push(ScreenSize {
            size: wanted,
            used: self.uses,
        });
        if self.sizes.len() > MAX_SCREEN_SIZES
            && let Some((index, _)) = self.sizes.iter().enumerate().min_by_key(|(_, entry)| entry.used)
        {
            self.sizes.remove(index);
        }
        wanted
    }
}

/// The zoom states of every viewport that draws text under a transform.
#[derive(Clone, Default)]
struct ZoomStates {
    per_viewport: BTreeMap<ViewportId, ZoomState>,
}

/// The number of screen sizes that the state of `ctx` keeps alive at this
/// moment, across every viewport. A test reads it; the paint path does not.
#[cfg(test)]
fn live_screen_sizes(ctx: &Context) -> usize {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_insert_with(state_id(), ZoomStates::default)
            .per_viewport
            .values()
            .map(|state| state.sizes.len())
            .sum()
    })
}

fn state_id() -> Id {
    Id::unique("horizon_zoom_text")
}

/// Round a screen font size onto the ladder of [`SCREEN_SIZE_QUANTUM`] and keep
/// it inside the range that is worth rasterising.
fn quantise(screen: f32) -> f32 {
    if !screen.is_finite() || screen <= 0.0 {
        return screen;
    }
    let clamped = screen.clamp(MIN_SCREEN_SIZE, MAX_SCREEN_SIZE);
    (clamped / SCREEN_SIZE_QUANTUM).round() * SCREEN_SIZE_QUANTUM
}

/// The zoom of the layer that `painter` draws in, and the size of the glyph
/// pictures that the frame uses.
#[derive(Clone, Debug)]
pub(crate) struct ZoomText {
    ctx: Context,
    /// The zoom whose glyph pictures are in use.
    glyph: f32,
}

impl ZoomText {
    /// The zoom of the painter's layer, and the glyph pictures for this frame.
    pub(crate) fn of(painter: &Painter) -> Self {
        Self::of_at(painter, Instant::now())
    }

    /// [`Self::of`] at an instant that a test gives.
    pub(crate) fn of_at(painter: &Painter, now: Instant) -> Self {
        let ctx = painter.ctx().clone();
        let scale = layer_scale(painter);
        let viewport = ctx.viewport_id();
        let (glyph, wait) = ctx.data_mut(|data| {
            let states = data.get_temp_mut_or_insert_with(state_id(), ZoomStates::default);
            states.per_viewport.entry(viewport).or_default().observe(scale, now)
        });
        if let Some(wait) = wait {
            ctx.request_repaint_after(wait);
        }
        Self { ctx, glyph }
    }

    /// The zoom whose glyph pictures this frame uses.
    pub(crate) fn glyph_zoom(&self) -> f32 {
        self.glyph
    }

    /// Whether the glyph pictures are the ones of zoom 1: then this module does
    /// nothing and the painter draws exactly what it drew before.
    pub(crate) fn is_logical(&self) -> bool {
        (self.glyph - 1.0).abs() <= f32::EPSILON
    }

    /// The screen size of a logical font size.
    pub(crate) fn screen_size(&self, logical: f32) -> f32 {
        if self.is_logical() {
            return logical;
        }
        let viewport = self.ctx.viewport_id();
        let glyph = self.glyph;
        self.ctx.data_mut(|data| {
            let states = data.get_temp_mut_or_insert_with(state_id(), ZoomStates::default);
            let state = states.per_viewport.entry(viewport).or_default();
            state.screen_size(logical, glyph)
        })
    }

    /// `font` at the size it has on the screen.
    pub(crate) fn font(&self, font: &FontId) -> FontId {
        if self.is_logical() {
            return font.clone();
        }
        FontId::new(self.screen_size(font.size), font.family.clone())
    }

    /// The factor that undoes the zoom of the glyph pictures, so that the layer
    /// transform draws one glyph texel on one pixel.
    pub(crate) fn inverse(&self) -> f32 {
        1.0 / self.glyph
    }

    /// `format` at the size of the screen.
    fn screen_format(&self, format: &TextFormat) -> TextFormat {
        let logical = format.font_id.size;
        let factor = if logical > 0.0 {
            self.screen_size(logical) / logical
        } else {
            self.glyph
        };
        let mut screen = format.clone();
        screen.font_id.size = self.screen_size(logical);
        screen.extra_letter_spacing *= factor;
        if let Some(line_height) = screen.line_height.as_mut() {
            *line_height *= factor;
        }
        screen
    }

    /// `job` laid out for the screen with the line breaks of `logical`, the
    /// layout of zoom 1.
    ///
    /// A job of this toolkit breaks its lines on the pixel-rounded position of
    /// a glyph, so a text can break at one zoom and not at another even though
    /// its advances grow with its size. The breaks of a text are therefore the
    /// breaks of zoom 1: this job names one line per row of that layout and
    /// takes no wrap width, so the screen layout follows those lines.
    pub(crate) fn kept_lines(&self, job: &LayoutJob, logical: &Galley) -> LayoutJob {
        let mut kept = if logical.elided || logical.rows.len() > 1 {
            self.lines_of(job, logical)
        } else {
            // One line that the logical layout did not cut: the text is the
            // text of the job, and only its sizes move to the screen.
            let mut kept = job.clone();
            for section in &mut kept.sections {
                section.format = self.screen_format(&section.format);
            }
            kept
        };
        kept.wrap = TextWrapping {
            max_width: f32::INFINITY,
            max_rows: usize::MAX,
            ..TextWrapping::default()
        };
        kept.break_on_newline = true;
        kept.halign = Align::LEFT;
        kept.justify = false;
        kept
    }

    /// The text of `logical` as one row per line, in the formats of `job`.
    fn lines_of(&self, job: &LayoutJob, logical: &Galley) -> LayoutJob {
        let mut text = String::new();
        let mut sections: Vec<LayoutSection> = Vec::new();
        // The section of the job that the open section of the new job copies.
        let mut from = usize::MAX;
        let mut source = Source::new(job);
        for (index, placed) in logical.rows.iter().enumerate() {
            if index > 0 {
                text.push('\n');
            }
            for glyph in &placed.row.glyphs {
                let section = source.take(glyph.chr);
                if section != from {
                    from = section;
                    sections.push(LayoutSection {
                        leading_space: 0.0,
                        byte_range: ByteIndex(text.len())..ByteIndex(text.len()),
                        format: self.screen_format(&job.sections[section].format),
                    });
                }
                text.push(glyph.chr);
                if let Some(open) = sections.last_mut() {
                    open.byte_range.end = ByteIndex(text.len());
                }
            }
        }
        // A row without a glyph opens no section, so the last one must reach
        // the end of the text.
        if let Some(last) = sections.last_mut() {
            last.byte_range.end = ByteIndex(text.len());
        }
        LayoutJob {
            text,
            sections,
            wrap: TextWrapping::default(),
            first_row_min_height: job.first_row_min_height,
            break_on_newline: true,
            halign: Align::LEFT,
            justify: false,
            round_output_to_gui: job.round_output_to_gui,
            keep_trailing_whitespace: job.keep_trailing_whitespace,
        }
    }
}

/// The place of a character of the text of a job, so that the row of a layout
/// keeps the format that the job gave it.
struct Source<'a> {
    job: &'a LayoutJob,
    /// The byte of the text of the job that the next row character is at.
    byte: usize,
}

impl<'a> Source<'a> {
    fn new(job: &'a LayoutJob) -> Self {
        Self { job, byte: 0 }
    }

    /// The section of `chr` in the text of the job, and the walk past it. A
    /// break eats the whitespace that it breaks at, and an elision adds a
    /// character that the text does not have.
    fn take(&mut self, chr: char) -> usize {
        let text = &self.job.text;
        while let Some(current) = text[self.byte..].chars().next() {
            if current == chr || !current.is_whitespace() {
                break;
            }
            self.byte += current.len_utf8();
        }
        let section = section_at(self.job, self.byte);
        if text[self.byte..].starts_with(chr) {
            self.byte += chr.len_utf8();
        }
        section
    }
}

/// The index of the section of `job` that holds `byte`.
fn section_at(job: &LayoutJob, byte: usize) -> usize {
    job.sections
        .iter()
        .position(|section| section.byte_range.contains(&ByteIndex(byte)))
        .unwrap_or_else(|| job.sections.len().saturating_sub(1))
}

/// The scale of the layer that `painter` draws in, which is 1.0 for the text
/// that the canvas zoom does not touch: the sidebar, the settings, a detached
/// workspace window and the fullscreen panel.
pub(crate) fn layer_scale(painter: &Painter) -> f32 {
    painter
        .ctx()
        .layer_transform_to_global(painter.layer_id())
        .map_or(1.0, |transform| transform.scaling)
}

/// `job` laid out with the glyph pictures of the screen size, on the lines of
/// zoom 1, and with its offsets divided by the zoom of those pictures.
///
/// The rectangle of the galley is the rectangle of zoom 1, so a part that is
/// laid out around this text keeps the size and the height of zoom 1.
pub(crate) fn layout_job(painter: &Painter, job: LayoutJob) -> Arc<Galley> {
    let zoom = ZoomText::of(painter);
    if zoom.is_logical() {
        return painter.layout_job(job);
    }
    let logical = painter.layout_job(job.clone());
    let galley = painter.layout_job(zoom.kept_lines(&job, &logical));
    let mut galley = sharpen(galley, zoom.inverse());
    Arc::make_mut(&mut galley).rect = logical.rect;
    galley
}

/// `text` on one line at the screen size.
pub(crate) fn layout_no_wrap(painter: &Painter, text: String, font: FontId, color: Color32) -> Arc<Galley> {
    let zoom = ZoomText::of(painter);
    if zoom.is_logical() {
        return painter.layout_no_wrap(text, font, color);
    }
    let galley = painter.layout_no_wrap(text, zoom.font(&font), color);
    sharpen(galley, zoom.inverse())
}

/// `text` on one line at the screen size, anchored at `pos`, and the rectangle
/// it covers.
pub(crate) fn text(painter: &Painter, pos: Pos2, anchor: Align2, text: &str, font: &FontId, color: Color32) -> Rect {
    let zoom = ZoomText::of(painter);
    if zoom.is_logical() {
        return painter.text(pos, anchor, text, font.clone(), color);
    }
    let galley = painter.layout_no_wrap(text.to_owned(), zoom.font(font), color);
    let galley = sharpen(galley, zoom.inverse());
    let rect = anchor.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
    rect
}

/// `text` as the widget text that a stock label or a stock button takes.
///
/// A stock widget lays an [`egui::RichText`] out at the font size of zoom 1, and
/// the canvas layer then scales the finished picture of every letter. This galley
/// is laid out at the screen size and keeps the size and the lines of zoom 1, so a
/// widget that takes it stays where and as large as it is at zoom 1 and its
/// letters stay sharp at every zoom. `width` is the room the text may take: a text
/// that does not fit ends with an ellipsis, as the label would shorten it.
pub(crate) fn widget_text(
    painter: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    width: Option<f32>,
) -> WidgetText {
    let job = match width {
        Some(width) => crate::text::single_line_label_job(text, &font, color, width),
        None => LayoutJob::single_section(
            text.to_owned(),
            TextFormat {
                font_id: font,
                color,
                ..TextFormat::default()
            },
        ),
    };
    WidgetText::Galley(layout_job(painter, job))
}

/// One text run of the terminal: `text` at the screen size, drawn from `pos`.
///
/// `font` is already the size of the screen, from [`ZoomText::font`]: the
/// rasteriser of the caller holds the lock of the context, so this path cannot
/// ask the state of the zoom for anything.
pub(crate) fn text_shape(
    fonts: &mut FontsView<'_>,
    pos: Pos2,
    text: &str,
    font: &FontId,
    color: Color32,
    zoom: &ZoomText,
) -> Shape {
    if zoom.is_logical() {
        return Shape::text(fonts, pos, Align2::LEFT_TOP, text, font.clone(), color);
    }
    let galley = fonts.layout_no_wrap(text.to_owned(), font.clone(), color);
    let galley = sharpen(galley, zoom.inverse());
    Shape::Text(TextShape::new(pos, galley, color))
}

/// A galley whose glyph offsets are divided by `inverse`'s reciprocal, so that
/// the layer transform of the canvas paints one glyph texel on one pixel.
///
/// The toolkit scales a text shape by walking its rows and its mesh; the shape
/// is built on the origin so that only the glyph offsets move.
fn sharpen(galley: Arc<Galley>, inverse: f32) -> Arc<Galley> {
    if (inverse - 1.0).abs() <= f32::EPSILON {
        return galley;
    }
    let mut shape = TextShape::new(Pos2::ZERO, galley, Color32::PLACEHOLDER);
    shape.transform(TSTransform::from_scaling(inverse));
    shape.galley
}

/// The glyph quads of one canvas pass: how many there are, the worst distance of a
/// quad corner from a whole device pixel, and the worst stretch between a quad and
/// the texels of its glyph picture.
///
/// A place that hands its text to the layout of zoom 1 measures a stretch here,
/// because the picture was made for that size and the pass draws it at another.
#[cfg(test)]
pub(crate) fn measure_canvas_quads(
    pixels_per_point: f32,
    zoom: f32,
    draw: impl FnOnce(&mut egui::Ui, &ZoomText),
) -> (usize, f32, f32) {
    use egui::emath::TSTransform;

    let ctx = Context::default();
    ctx.set_fonts(crate::app::bootstrap::configure_fonts());
    let transform = TSTransform {
        scaling: zoom,
        translation: egui::vec2(13.7, 21.3),
    };
    let layer = egui::LayerId::new(egui::Order::Background, egui::Id::unique("canvas"));
    let mut input = egui::RawInput::default();
    input.viewports.insert(
        egui::ViewportId::ROOT,
        egui::ViewportInfo {
            native_pixels_per_point: Some(pixels_per_point),
            ..egui::ViewportInfo::default()
        },
    );
    let mut draw = Some(draw);
    let output = ctx.run_ui(input, |ui| {
        assert!(
            (ui.ctx().pixels_per_point() - pixels_per_point).abs() < f32::EPSILON,
            "the pass draws at the display scale of the test"
        );
        ctx.set_transform_layer(layer, transform);
        let layer_ui = egui::UiBuilder::new()
            .layer_id(layer)
            .max_rect(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0)));
        let mut child = egui::Ui::new(ctx.clone(), egui::Id::unique("canvas ui"), layer_ui);
        let zoom_text = ZoomText::of(child.painter());
        assert!(
            (zoom_text.glyph_zoom() - zoom).abs() < f32::EPSILON,
            "the first frame takes the glyph pictures of its zoom"
        );
        if let Some(draw) = draw.take() {
            draw(&mut child, &zoom_text);
        }
    });
    let shapes = output.shapes.clone();
    let _ = crate::test_egui::DiscardTextures::discard_textures(output);
    let primitives = ctx.tessellate(shapes, pixels_per_point);
    let [atlas_width, atlas_height] = atlas_size(&ctx);
    assert!(atlas_width > 0.0 && atlas_height > 0.0);
    let mut quads = 0_usize;
    let mut worst_phase: f32 = 0.0;
    let mut worst_stretch: f32 = 0.0;
    for primitive in &primitives {
        let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive else {
            continue;
        };
        for quad in mesh.vertices.chunks(4) {
            let [a, b, c, d] = [quad[0], quad[1], quad[2], quad[3]];
            // A filled rectangle has one colour and no glyph picture, so it says
            // nothing about the size that a picture was made for.
            let texels = (b.uv.x - a.uv.x).abs() * atlas_width;
            let rows = (c.uv.y - b.uv.y).abs() * atlas_height;
            if texels <= 0.0 || rows <= 0.0 {
                continue;
            }
            quads += 1;
            for vertex in [a, b, c, d] {
                for value in [vertex.pos.x, vertex.pos.y] {
                    let device = value * pixels_per_point;
                    worst_phase = worst_phase.max((device - device.round()).abs());
                }
            }
            // The quad covers the texels of its glyph picture, so the picture is
            // drawn at the size that it was made for.
            let width = (b.pos.x - a.pos.x).abs() * pixels_per_point;
            let height = (c.pos.y - b.pos.y).abs() * pixels_per_point;
            worst_stretch = worst_stretch.max((width - texels).abs()).max((height - rows).abs());
        }
    }
    (quads, worst_phase, worst_stretch)
}

/// The size of the glyph atlas of its context, in texels.
#[cfg(test)]
#[allow(
    clippy::redundant_closure_for_method_calls,
    reason = "Context::fonts wants a higher-ranked closure, so the method path does not compile"
)]
pub(crate) fn atlas_size(ctx: &Context) -> [f32; 2] {
    let [width, height] = ctx.fonts(|fonts| fonts.font_image_size());
    #[allow(clippy::cast_precision_loss)]
    [width as f32, height as f32]
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use egui::text::{LayoutJob, TextFormat, TextWrapping};
    use egui::{Color32, Context, FontId, Pos2};

    use super::{
        MAX_SCREEN_SIZES, SCREEN_SIZE_QUANTUM, SETTLE, ZoomState, ZoomText, layout_job, live_screen_sizes, quantise,
    };
    use crate::test_egui::with_canvas_layer;
    use std::cell::Cell;

    const LOGICAL: f32 = 13.0;

    fn paragraph() -> LayoutJob {
        let mut job = LayoutJob::single_section(
            "A paragraph that wraps over several lines and names each word of it.".to_owned(),
            TextFormat {
                font_id: FontId::proportional(LOGICAL),
                color: Color32::WHITE,
                ..TextFormat::default()
            },
        );
        job.wrap = TextWrapping {
            max_width: 180.0,
            ..TextWrapping::default()
        };
        job
    }

    fn row_texts(galley: &egui::Galley) -> Vec<String> {
        galley
            .rows
            .iter()
            .map(|placed| {
                placed
                    .row
                    .glyphs
                    .iter()
                    .map(|glyph| glyph.chr)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn the_screen_size_is_the_logical_size_times_the_zoom_on_a_quarter_point_ladder() {
        assert!((quantise(12.22) - 12.25).abs() < f32::EPSILON);
        assert!((quantise(13.0) - 13.0).abs() < f32::EPSILON);
        assert!((quantise(8.71) - 8.75).abs() < f32::EPSILON);
        assert!((SCREEN_SIZE_QUANTUM - 0.25).abs() < f32::EPSILON);

        let ctx = Context::default();
        let font = FontId::monospace(LOGICAL);
        let zoom = with_canvas_layer(&ctx, 0.67, |ui| {
            let galley = layout_job(
                ui.painter(),
                LayoutJob::single_section(
                    "MMMM".to_owned(),
                    TextFormat {
                        font_id: font.clone(),
                        color: Color32::WHITE,
                        ..TextFormat::default()
                    },
                ),
            );
            let logical = ui.painter().layout_job(LayoutJob::single_section(
                "MMMM".to_owned(),
                TextFormat {
                    font_id: font.clone(),
                    color: Color32::WHITE,
                    ..TextFormat::default()
                },
            ));
            (ZoomText::of(ui.painter()), galley, logical.size().x)
        })
        .expect("one frame");
        let (zoom, galley, logical_width) = zoom;
        assert!((zoom.screen_size(LOGICAL) - 8.75).abs() < f32::EPSILON);
        assert!(
            (zoom.inverse() * 0.67 - 1.0).abs() < 1.0e-6,
            "the draw factor is the exact inverse of the zoom"
        );
        // The galley the painter draws holds the screen-size glyphs, divided by
        // the zoom of those pictures: its width is the width of zoom 1.
        assert!(
            (galley.size().x - logical_width).abs() <= 0.03 * logical_width,
            "the laid out width {} should stay the logical width {logical_width}",
            galley.size().x
        );
        assert_eq!(galley.rows.len(), 1);
    }

    #[test]
    fn a_paragraph_breaks_at_the_same_words_at_two_zooms() {
        let ctx = Context::default();
        let logical = with_canvas_layer(&ctx, 1.0, |ui| layout_job(ui.painter(), paragraph())).expect("one frame");
        let zoomed = with_canvas_layer(&ctx, 0.67, |ui| layout_job(ui.painter(), paragraph())).expect("one frame");
        let other = with_canvas_layer(&ctx, 1.5, |ui| layout_job(ui.painter(), paragraph())).expect("one frame");

        let expected = row_texts(&logical);
        assert!(expected.len() > 1, "the paragraph should wrap: {expected:?}");
        assert_eq!(row_texts(&zoomed), expected);
        assert_eq!(row_texts(&other), expected);
    }

    #[test]
    fn a_gesture_keeps_the_glyphs_of_its_start_and_settles_into_one_repaint() {
        let start = Instant::now();
        let mut state = ZoomState::default();
        // The first frame of the frame sets the pictures of the current zoom.
        assert_eq!(state.observe(1.0, start), (1.0, None));

        // The zoom moves: the pictures stay, and the first change asks for one
        // frame at the settle time.
        let mut requests = 0;
        for step in 1_u64..=5 {
            #[allow(clippy::cast_precision_loss)]
            let zoom = 1.0 - step as f32 * 0.02;
            let (glyph, wait) = state.observe(zoom, start + Duration::from_millis(step * 16));
            assert!(
                (glyph - 1.0).abs() < f32::EPSILON,
                "a gesture keeps the glyph size it started with"
            );
            if wait.is_some() {
                requests += 1;
                assert_eq!(wait, Some(SETTLE));
            }
        }
        assert_eq!(requests, 1, "one repaint request for the whole gesture");

        // The frame after the settle time uses the new pictures and asks for
        // nothing more.
        let (glyph, wait) = state.observe(0.8, start + SETTLE + Duration::from_millis(100));
        assert!((glyph - 0.8).abs() < f32::EPSILON);
        assert_eq!(wait, None);
        // Idle frames ask for nothing.
        assert_eq!(state.observe(0.8, start + Duration::from_secs(5)), (0.8, None));
    }

    #[test]
    fn the_ladder_of_screen_sizes_stays_bounded_over_five_hundred_zooms() {
        let mut state = ZoomState::default();
        let mut seed = 7_u64;
        for _ in 0..500 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            #[allow(clippy::cast_precision_loss)]
            let zoom = 0.05 + f32::from(u16::try_from(seed >> 48).expect("16 bits")) / 65_535.0 * 3.95;
            state.glyph_zoom = zoom;
            for logical in [11.0_f32, 11.5, 12.0, 12.5, 13.0, 14.0, 16.0, 18.0] {
                let size = state.screen_size(logical, zoom);
                assert!(size > 0.0);
            }
        }
        assert!(
            state.sizes.len() <= MAX_SCREEN_SIZES,
            "{} screen sizes is more than the {} that the state keeps",
            state.sizes.len(),
            MAX_SCREEN_SIZES
        );
    }

    #[test]
    fn the_terminal_cells_keep_their_origins_at_three_zooms() {
        let ctx = Context::default();
        let char_width = 7.804_687_5_f32;
        let line_height = 15.0_f32;
        let start = Instant::now();
        let frame = Cell::new(0_u64);
        // The zoom of each frame settles into the glyph pictures of that zoom,
        // so each of them is drawn twice, a settle time apart.
        let origins = |zoom: f32| {
            let mut cells = Vec::new();
            for _ in 0..2 {
                let at = start + SETTLE * 4 * u32::try_from(frame.get()).expect("a few frames");
                frame.set(frame.get() + 1);
                cells = with_canvas_layer(&ctx, zoom, |ui| {
                    let zoom = ZoomText::of_at(ui.painter(), at);
                    let font = FontId::monospace(LOGICAL);
                    (0..4)
                        .map(|column| {
                            Pos2::new(
                                f32::from(u8::try_from(column).expect("four cells")) * char_width,
                                2.0 * line_height,
                            )
                        })
                        .map(|origin| (origin, zoom.font(&font).size))
                        .collect::<Vec<(Pos2, f32)>>()
                })
                .expect("one frame");
            }
            cells
        };

        let (at_80, at_100, at_130) = (origins(0.8), origins(1.0), origins(1.3));
        let positions = |cells: &[(Pos2, f32)]| cells.iter().map(|(pos, _)| *pos).collect::<Vec<Pos2>>();
        assert_eq!(positions(&at_80), positions(&at_100));
        assert_eq!(positions(&at_130), positions(&at_100));
        assert!(
            (at_100[0].1 - LOGICAL).abs() < f32::EPSILON,
            "zoom 1 keeps the logical glyph size"
        );
        assert!(
            (at_80[0].1 - 10.5).abs() < f32::EPSILON,
            "0.8 of 13 points is 10.4, on the ladder 10.5"
        );
        assert!(
            (at_130[0].1 - 17.0).abs() < f32::EPSILON,
            "1.3 of 13 points is 16.9, on the ladder 17.0"
        );
    }

    /// A line of a card, a title of a panel and three runs of a terminal grid,
    /// drawn in the pass of a canvas at `zoom`.
    fn draw_canvas_text(child: &egui::Ui, zoom_text: &ZoomText, zoom: f32) {
        let painter = child.painter();
        let mut job = LayoutJob::single_section(
            "build-1 and the machine it runs on".to_owned(),
            TextFormat {
                font_id: FontId::proportional(11.0),
                color: Color32::WHITE,
                ..TextFormat::default()
            },
        );
        job.wrap.max_width = 400.0;
        let card = layout_job(painter, job);
        assert!(card.size().x > 0.0 && card.size().y > 0.0);
        painter.galley(egui::pos2(40.3, 60.7), card, Color32::WHITE);

        let title = super::text(
            painter,
            egui::pos2(40.3, 160.7),
            egui::Align2::LEFT_TOP,
            "Device input test",
            &FontId::proportional(13.0),
            Color32::WHITE,
        );
        assert!(title.width() > 0.0);

        // The cell origins of the zoom, which a zoom of 0.94 leaves between
        // two device pixels.
        let cell = zoom_text.font(&FontId::monospace(13.0));
        let char_width = 7.8 * zoom;
        let line_height = 16.9 * zoom;
        let runs = child.fonts_mut(|fonts| {
            ["The quick brown fox", "jumps over", "0123456789 {}[]|"]
                .into_iter()
                .enumerate()
                .map(|(run, text)| {
                    #[allow(clippy::cast_precision_loss)]
                    let run = run as f32;
                    super::text_shape(
                        fonts,
                        egui::pos2(40.0 + run * char_width * 3.0, 200.0 + run * line_height),
                        text,
                        &cell,
                        Color32::WHITE,
                        zoom_text,
                    )
                })
                .collect::<Vec<_>>()
        });
        for shape in runs {
            painter.add(shape);
        }
    }

    /// The words of every row of `galley`, in order.
    fn row_words(galley: &egui::Galley) -> Vec<String> {
        galley
            .rows
            .iter()
            .map(|placed| {
                placed
                    .row
                    .glyphs
                    .iter()
                    .map(|glyph| glyph.chr)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    /// The jobs of the three shapes of text that a card, a title and a label
    /// take: a paragraph, a line that a narrow column elides, and a job of two
    /// formats.
    fn tight_jobs() -> Vec<(&'static str, LayoutJob)> {
        let font = FontId::proportional(12.5);
        let mut paragraph = LayoutJob::single_section(
            "The machine was created at the provider and the agent is waiting for it.".to_owned(),
            TextFormat {
                font_id: font.clone(),
                color: Color32::WHITE,
                ..TextFormat::default()
            },
        );
        paragraph.wrap = TextWrapping {
            max_width: 150.0,
            ..TextWrapping::default()
        };
        let elided = crate::text::single_line_label_job(
            "Agent integrations are off, so the agent has no Horizon skills.",
            &font,
            Color32::WHITE,
            96.0,
        );
        let mut two_formats = LayoutJob::default();
        two_formats.append(
            "Set up needs an agent ",
            0.0,
            TextFormat {
                font_id: FontId::proportional(15.0),
                color: Color32::WHITE,
                ..TextFormat::default()
            },
        );
        two_formats.append(
            "and the agent is waiting for the machine that it asked for",
            0.0,
            TextFormat {
                font_id: FontId::proportional(11.0),
                color: Color32::WHITE,
                ..TextFormat::default()
            },
        );
        two_formats.wrap = TextWrapping {
            max_width: 130.0,
            ..TextWrapping::default()
        };
        vec![
            ("a paragraph", paragraph),
            ("an elided line", elided),
            ("two formats", two_formats),
        ]
    }

    /// A text breaks its lines where the layout of zoom 1 breaks them, at every
    /// zoom between 0.5 and 2.0: the screen layout follows the lines that zoom
    /// 1 made instead of deciding them again.
    #[test]
    fn a_text_keeps_the_lines_of_zoom_one_at_forty_zooms() {
        use crate::test_egui::with_canvas_transform;

        let rows_at = |zoom: f32, name: &str, job: &LayoutJob| -> Vec<String> {
            let ctx = Context::default();
            let transform = egui::emath::TSTransform {
                scaling: zoom,
                translation: egui::vec2(13.7, 21.3),
            };
            let galley =
                with_canvas_transform(&ctx, transform, |ui| layout_job(ui.painter(), job.clone())).expect("one frame");
            let rows = row_words(&galley);
            assert!(!rows.is_empty(), "{name} at zoom {zoom} has a row");
            rows
        };

        for (name, job) in tight_jobs() {
            let expected = rows_at(1.0, name, &job);
            assert!(
                expected.len() > 1 || expected[0].contains('\u{2026}'),
                "{name} wraps or elides at zoom 1: {expected:?}"
            );
            for step in 0..40 {
                #[allow(clippy::cast_precision_loss)]
                let zoom = 0.5 + step as f32 * (1.5 / 39.0);
                assert_eq!(
                    rows_at(zoom, name, &job),
                    expected,
                    "{name} breaks at the words of zoom 1 at zoom {zoom}"
                );
            }
        }
    }

    /// One pass of the canvas at `zoom` and the display scale of the test, with
    /// a pan offset that leaves the text between two whole pixels.
    fn canvas_quads(pixels_per_point: f32, zoom: f32) -> (usize, f32, f32) {
        super::measure_canvas_quads(pixels_per_point, zoom, |child, zoom_text| {
            draw_canvas_text(child, zoom_text, zoom);
        })
    }

    /// At rest, the corners of every glyph quad of the canvas sit on whole
    /// device pixels and cover as many pixels as their glyph has texels, so a
    /// stem of one pixel is one pixel and no letter is resampled: a card line,
    /// a title of a panel and three terminal runs, at display scale 1 and 2.
    #[test]
    fn the_glyph_quads_of_the_canvas_sit_on_whole_device_pixels() {
        for pixels_per_point in [1.0_f32, 2.0] {
            for zoom in [0.94_f32, 0.67] {
                let (quads, worst_phase, worst_stretch) = canvas_quads(pixels_per_point, zoom);
                assert!(quads > 60, "the pass draws the three kinds of text: {quads} quads");
                assert!(
                    worst_phase <= 0.01,
                    "at zoom {zoom} and display scale {pixels_per_point} a glyph corner is {worst_phase} device pixels from a whole pixel"
                );
                assert!(
                    worst_stretch <= 0.01,
                    "at zoom {zoom} and display scale {pixels_per_point} a glyph picture is stretched by {worst_stretch} device pixels"
                );
            }
        }
    }

    #[test]
    fn the_paint_path_keeps_at_most_one_ladders_worth_of_sizes() {
        let ctx = Context::default();
        let mut frame = 0;
        for zoom in [0.67_f32, 0.94, 1.0, 1.5, 2.0] {
            with_canvas_layer(&ctx, zoom, |ui| {
                let _ = layout_job(ui.painter(), paragraph());
            });
            frame += 1;
        }
        assert!(frame > 0);
        assert!(live_screen_sizes(&ctx) <= MAX_SCREEN_SIZES);
    }

    #[test]
    fn a_frame_at_zoom_one_takes_the_plain_path() {
        let ctx = Context::default();
        let (logical, plain_size, sharp_size) = with_canvas_layer(&ctx, 1.0, |ui| {
            let plains = ui.painter().layout_job(paragraph());
            (
                ZoomText::of(ui.painter()).is_logical(),
                plains.size(),
                layout_job(ui.painter(), paragraph()).size(),
            )
        })
        .expect("one frame");
        assert!(logical, "zoom 1 is the logical path");
        assert_eq!(plain_size, sharp_size, "zoom 1 draws the galley it always drew");
    }
}
