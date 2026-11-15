//! The small drawing of each tile, made of plain shapes.
//!
//! A drawing is static: it takes the width of its tile and not the state of
//! anything, so the page builds it once per layout and paints the same shapes on
//! every frame. Every colour is a token of the theme, so both themes draw their
//! own.

use egui::{CornerRadius, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};

use crate::theme;

use super::tiles::WelcomeTile;

/// The height of the band of a drawing.
pub(in crate::app) const ART_HEIGHT: f32 = 92.0;
/// The width that the drawings are drawn for: the widest of them reaches 101
/// points from the middle of its band, and four points of air stay on each side.
pub(in crate::app) const ART_WIDTH: f32 = 210.0;
/// The corner radius of the band of a drawing.
pub(in crate::app) const BAND_RADIUS: u8 = 10;
/// The corner radius of a shape inside a drawing.
const SHAPE_RADIUS: u8 = 5;

/// The shapes of the drawing of `tile`, in the coordinates of its band: the
/// origin is the top left corner of the band and it is `size` large.
pub(in crate::app) fn drawing(tile: WelcomeTile, size: Vec2) -> Vec<Shape> {
    let painter = Drawing {
        size,
        shapes: Vec::new(),
    };
    match tile {
        WelcomeTile::Agents => agents(painter),
        WelcomeTile::Machine => machine(painter),
        WelcomeTile::Server => server(painter),
        WelcomeTile::Browser => browser(&painter),
    }
}

/// One drawing in progress: the band that it draws in, and the shapes so far.
struct Drawing {
    size: Vec2,
    shapes: Vec<Shape>,
}

impl Drawing {
    /// The centre of the band, moved by `offset`.
    fn at(&self, offset: Vec2) -> Pos2 {
        let center = self.size * 0.5;
        pos2(center.x + offset.x, center.y + offset.y)
    }

    /// A rectangle of `size` centred on the band, moved by `offset`.
    fn rect(&self, offset: Vec2, size: Vec2) -> Rect {
        Rect::from_center_size(self.at(offset), size)
    }

    fn panel(&mut self, offset: Vec2, size: Vec2, outline: egui::Color32) {
        let rect = self.rect(offset, size);
        let radius = CornerRadius::same(SHAPE_RADIUS);
        self.shapes
            .push(Shape::rect_filled(rect, radius, theme::PANEL_BG_ALT()));
        self.shapes.push(Shape::rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, outline),
            StrokeKind::Inside,
        ));
        // The strip of a title.
        let strip = Rect::from_min_size(rect.min, vec2(rect.width(), 7.0));
        self.shapes.push(Shape::rect_filled(
            strip,
            CornerRadius::same(SHAPE_RADIUS),
            theme::PANEL_BG(),
        ));
    }

    fn line(&mut self, offset: Vec2, width: f32, color: egui::Color32) {
        let rect = self.rect(offset, vec2(width, 2.0));
        self.shapes.push(Shape::rect_filled(rect, CornerRadius::same(1), color));
    }

    fn outline_box(&mut self, offset: Vec2, size: Vec2, outline: egui::Color32, fill: egui::Color32, lines: usize) {
        let rect = self.rect(offset, size);
        let radius = CornerRadius::same(8);
        self.shapes.push(Shape::rect_filled(rect, radius, fill));
        self.shapes.push(Shape::rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, outline),
            StrokeKind::Inside,
        ));
        #[allow(clippy::cast_precision_loss)]
        let count = lines as f32;
        for index in 0..lines {
            #[allow(clippy::cast_precision_loss)]
            let y = index as f32;
            let width = rect.width() - 20.0;
            let line = Rect::from_min_size(pos2(rect.min.x + 10.0, rect.min.y + 20.0 + y * 9.0), vec2(width, 2.0));
            let _ = count;
            self.shapes
                .push(Shape::rect_filled(line, CornerRadius::same(1), outline));
        }
    }
}

/// Three agent panels side by side; the middle one waits for the person.
fn agents(mut drawing: Drawing) -> Vec<Shape> {
    let width = 46.0;
    let height = 58.0;
    for (index, offset) in [-56.0_f32, 0.0, 56.0].iter().enumerate() {
        let waiting = index == 1;
        let outline = if waiting {
            theme::PALETTE_YELLOW()
        } else {
            theme::BORDER_STRONG()
        };
        drawing.panel(vec2(*offset, 0.0), vec2(width, height), outline);
        if waiting {
            // The strip of the agent that waits for an answer.
            let rect = drawing.rect(vec2(*offset, 0.0), vec2(width, height));
            let strip = Rect::from_min_size(rect.min, vec2(rect.width(), 7.0));
            drawing.shapes.push(Shape::rect_filled(
                strip,
                CornerRadius::same(SHAPE_RADIUS),
                theme::alpha(theme::PALETTE_YELLOW(), 90),
            ));
        }
    }
    drawing.shapes
}

/// A small panel, a line to a bigger machine and its lines of text.
fn machine(mut drawing: Drawing) -> Vec<Shape> {
    drawing.panel(vec2(-84.0, 0.0), vec2(34.0, 26.0), theme::BORDER_STRONG());
    drawing.line(vec2(-56.0, 0.0), 20.0, theme::BORDER_STRONG());
    drawing.outline_box(
        vec2(14.0, 0.0),
        vec2(62.0, 62.0),
        theme::ACCENT(),
        theme::alpha(theme::ACCENT(), 30),
        3,
    );
    drawing.shapes
}

/// Two small panels with a green server between them.
fn server(mut drawing: Drawing) -> Vec<Shape> {
    drawing.panel(vec2(-84.0, 0.0), vec2(34.0, 26.0), theme::BORDER_STRONG());
    drawing.line(vec2(-56.0, 0.0), 12.0, theme::BORDER_STRONG());
    let circle = drawing.rect(Vec2::ZERO, Vec2::splat(30.0));
    drawing.shapes.push(Shape::circle_filled(
        circle.center(),
        15.0,
        theme::alpha(theme::PALETTE_GREEN(), 30),
    ));
    drawing.shapes.push(Shape::circle_stroke(
        circle.center(),
        14.5,
        Stroke::new(1.0, theme::PALETTE_GREEN()),
    ));
    drawing.line(vec2(40.0, 0.0), 12.0, theme::BORDER_STRONG());
    drawing.panel(vec2(84.0, 0.0), vec2(34.0, 26.0), theme::BORDER_STRONG());
    drawing.shapes
}

/// A window with its title strip, a dot that says it is signed in, and two
/// lines of a page.
fn browser(drawing: &Drawing) -> Vec<Shape> {
    let size = vec2(132.0, 66.0);
    let rect = drawing.rect(Vec2::ZERO, size);
    let radius = CornerRadius::same(7);
    let mut shapes = Vec::new();
    shapes.push(Shape::rect_filled(rect, radius, theme::PANEL_BG_ALT()));
    shapes.push(Shape::rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, theme::BORDER_STRONG()),
        StrokeKind::Inside,
    ));
    let title = Rect::from_min_size(rect.min, vec2(rect.width(), 16.0));
    shapes.push(Shape::rect_filled(
        title,
        CornerRadius {
            nw: 7,
            ne: 7,
            sw: 0,
            se: 0,
        },
        theme::PANEL_BG(),
    ));
    shapes.push(Shape::circle_filled(
        pos2(title.min.x + 9.0, title.center().y),
        3.0,
        theme::PALETTE_GREEN(),
    ));
    let address = Rect::from_min_size(pos2(title.min.x + 16.0, title.center().y - 2.5), vec2(60.0, 5.0));
    shapes.push(Shape::rect_filled(
        address,
        CornerRadius::same(3),
        theme::BORDER_STRONG(),
    ));
    for (index, width) in [0.7_f32, 0.45].iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let y = index as f32;
        let line = Rect::from_min_size(
            pos2(rect.min.x + 10.0, rect.min.y + 28.0 + y * 12.0),
            vec2((rect.width() - 20.0) * width, 2.0),
        );
        shapes.push(Shape::rect_filled(line, CornerRadius::same(1), theme::BORDER_STRONG()));
    }
    shapes
}

/// The band of a drawing: the quiet plate that every drawing stands on.
pub(in crate::app) fn band(rect: Rect) -> Vec<Shape> {
    let radius = CornerRadius::same(BAND_RADIUS);
    vec![
        Shape::rect_filled(rect, radius, theme::PANEL_BG()),
        Shape::rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, theme::BORDER_SUBTLE()),
            StrokeKind::Inside,
        ),
    ]
}

/// The band of a drawing and its shapes, in the coordinates of the band: its top
/// left corner is the origin and it is `size` large.
///
/// A band that is narrower than [`ART_WIDTH`] or shorter than [`ART_HEIGHT`] is a
/// compact form of the page, and the drawing is drawn smaller to stand in it, so
/// the four choices of a small window keep their drawings and no shape of a
/// drawing reaches outside its band. A band that holds the drawing whole keeps it
/// in the size of the mockup, in the middle of the band.
pub(in crate::app) fn local(tile: WelcomeTile, size: Vec2) -> Vec<Shape> {
    let mut shapes = band(Rect::from_min_size(Pos2::ZERO, size));
    let mut drawing = drawing(tile, Vec2::new(ART_WIDTH, ART_HEIGHT));
    let scale = (size.x / ART_WIDTH).min(size.y / ART_HEIGHT).clamp(0.0, 1.0);
    if (scale - 1.0).abs() > f32::EPSILON {
        // The drawing is centred on the band that it was drawn for, so it keeps
        // its middle when it is drawn smaller.
        let middle = Vec2::new((size.x - ART_WIDTH * scale) * 0.5, (size.y - ART_HEIGHT * scale) * 0.5);
        for shape in &mut drawing {
            shape.scale(scale);
            shape.translate(middle);
        }
    } else {
        let middle = Vec2::new((size.x - ART_WIDTH) * 0.5, 0.0);
        for shape in &mut drawing {
            shape.translate(middle);
        }
    }
    shapes.extend(drawing);
    shapes
}

#[cfg(test)]
mod tests {
    use egui::{Vec2, vec2};

    use super::{ART_HEIGHT, ART_WIDTH, WelcomeTile, drawing, local};

    #[test]
    fn every_tile_draws_shapes_inside_its_band() {
        let size = Vec2::new(240.0, ART_HEIGHT);
        for tile in WelcomeTile::ALL {
            let shapes = drawing(tile, size);
            assert!(!shapes.is_empty(), "{tile:?} draws something");
            for shape in &shapes {
                let bounds = shape.visual_bounding_rect();
                assert!(
                    bounds.min.x >= -1.0 && bounds.max.x <= size.x + 1.0,
                    "{tile:?} stays inside its band: {bounds:?}"
                );
                assert!(
                    bounds.min.y >= -1.0 && bounds.max.y <= size.y + 1.0,
                    "{tile:?} stays inside its band: {bounds:?}"
                );
            }
        }
    }

    #[test]
    fn the_band_moves_the_drawing_to_its_place() {
        let rect = egui::Rect::from_min_size(egui::pos2(30.0, 40.0), vec2(240.0, ART_HEIGHT));
        let mut shapes = local(WelcomeTile::Browser, rect.size());
        for shape in &mut shapes {
            shape.translate(rect.min.to_vec2());
        }
        assert!(!shapes.is_empty());
        let moved = shapes
            .iter()
            .map(egui::Shape::visual_bounding_rect)
            .reduce(egui::Rect::union)
            .expect("a drawing");
        assert!(
            moved.min.x >= rect.min.x - 1.0 && moved.max.x <= rect.max.x + 1.0,
            "the drawing is inside its band: {moved:?}"
        );
    }

    #[test]
    fn a_band_that_is_too_small_draws_the_drawing_smaller() {
        for (width, height) in [(ART_WIDTH, ART_HEIGHT), (150.0, ART_HEIGHT), (240.0, 46.0)] {
            let size = vec2(width, height);
            let shapes = local(WelcomeTile::Server, size);
            for shape in &shapes {
                let bounds = shape.visual_bounding_rect();
                assert!(
                    bounds.min.x >= -1.0 && bounds.max.x <= size.x + 1.0,
                    "a drawing of a band of {size:?} stays inside it: {bounds:?}"
                );
                assert!(
                    bounds.min.y >= -1.0 && bounds.max.y <= size.y + 1.0,
                    "a drawing of a band of {size:?} stays inside it: {bounds:?}"
                );
            }
        }
    }
}
