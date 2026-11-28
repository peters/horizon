//! The backdrop of the canvas: the sky and the ground, the glow that lies on the
//! horizon, and the horizon line.
//!
//! The numbers are the approved design "Dusk" that
//! [`docs/design/guidelines.md`](../../../../docs/design/guidelines.md) states.
//! Every measure is a share of the canvas rectangle of the window, so the horizon
//! stands at the same share of the canvas of the root window, of a detached
//! workspace window and of the canvas behind the first start page. The backdrop
//! is fixed to the screen: panning and zooming move the dot grid and nothing
//! here.

use egui::{Color32, Mesh, Pos2, Rect, Rgba, Shape, Vec2};

use crate::theme::{self, ResolvedTheme};

/// The share of the canvas height where the sky meets the ground, and where the
/// third stop of the sky stands.
pub(super) const HORIZON_SHARE: f32 = 0.72;
/// The share of the canvas height where the second stop of the sky stands.
const SKY_MID_SHARE: f32 = 0.46;

/// The share of the canvas width that the line holds at its peak, the share
/// where it starts to fade out again, and its thickness.
const LINE_PEAK_SHARE: f32 = 0.20;
const LINE_FADE_SHARE: f32 = 0.80;
const LINE_WIDTH: f32 = 1.0;

/// The radii of the glow, as a share of the canvas width and height, the share
/// of the radius where the warm colour meets the cool one, and the share from
/// which the glow has no alpha.
const GLOW_RADIUS_X: f32 = 1.30;
const GLOW_RADIUS_Y: f32 = 0.26;
const GLOW_COOL_SHARE: f32 = 0.46;
const GLOW_EDGE_SHARE: f32 = 0.76;

/// The strips of the sky, one row of the gradient each, and the rings and the
/// segments of the glow. The counts fit in a `u16` and the loops count in one,
/// so a share of them is a plain `f32::from`.
const SKY_STRIPS: u16 = 96;
const GLOW_RINGS: u16 = 24;
const GLOW_SEGMENTS: u16 = 96;
/// The columns of the horizon line, and the spans between them.
const LINE_COLUMNS: usize = 4;
const LINE_SPANS: u32 = 3;

/// The colours of the backdrop of one theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BackdropColours {
    pub(super) sky_top: Color32,
    pub(super) sky_mid: Color32,
    pub(super) sky_horizon: Color32,
    pub(super) ground: Color32,
    pub(super) glow_warm: Color32,
    pub(super) glow_cool: Color32,
    pub(super) horizon_line: Color32,
}

impl BackdropColours {
    /// The colours of the active theme, which the caller resolves.
    fn active() -> Self {
        Self {
            sky_top: theme::CANVAS_SKY_TOP(),
            sky_mid: theme::CANVAS_SKY_MID(),
            sky_horizon: theme::CANVAS_SKY_HORIZON(),
            ground: theme::CANVAS_GROUND(),
            glow_warm: theme::CANVAS_GLOW_WARM(),
            glow_cool: theme::CANVAS_GLOW_COOL(),
            horizon_line: theme::CANVAS_HORIZON_LINE(),
        }
    }
}

/// The stops of the sky, as a share of the canvas height.
const SKY_STOPS: [f32; 4] = [0.0, SKY_MID_SHARE, HORIZON_SHARE, 1.0];

/// The colour of the sky at `share` of the canvas height.
pub(super) fn sky_colour_at(share: f32, colours: &BackdropColours) -> Color32 {
    let stops = [colours.sky_top, colours.sky_mid, colours.sky_horizon, colours.ground];
    if share <= SKY_STOPS[0] {
        return stops[0];
    }
    for index in 1..SKY_STOPS.len() {
        if share <= SKY_STOPS[index] {
            let span = SKY_STOPS[index] - SKY_STOPS[index - 1];
            let into = (share - SKY_STOPS[index - 1]) / span;
            return mix(stops[index - 1], stops[index], into);
        }
    }
    stops[stops.len() - 1]
}

/// The screen height of the horizon line inside `rect`.
pub(super) fn horizon_y(rect: Rect) -> f32 {
    rect.top() + rect.height() * HORIZON_SHARE
}

/// The alpha of the horizon line at `share` of the canvas width, as a share of
/// its peak: zero at both edges, and the peak from [`LINE_PEAK_SHARE`] to
/// [`LINE_FADE_SHARE`].
pub(super) fn horizon_line_alpha_share(share: f32) -> f32 {
    let share = share.clamp(0.0, 1.0);
    if share <= LINE_PEAK_SHARE {
        share / LINE_PEAK_SHARE
    } else if share >= LINE_FADE_SHARE {
        (1.0 - share) / (1.0 - LINE_FADE_SHARE)
    } else {
        1.0
    }
}

/// The colour of the glow at `distance` of its radius, from the centre outward:
/// the warm colour at the centre, the cool colour at [`GLOW_COOL_SHARE`], and no
/// alpha from [`GLOW_EDGE_SHARE`] outward.
pub(super) fn glow_colour_at(distance: f32, colours: &BackdropColours) -> Color32 {
    if distance <= 0.0 {
        return colours.glow_warm;
    }
    if distance <= GLOW_COOL_SHARE {
        return mix(colours.glow_warm, colours.glow_cool, distance / GLOW_COOL_SHARE);
    }
    if distance <= GLOW_EDGE_SHARE {
        let into = (distance - GLOW_COOL_SHARE) / (GLOW_EDGE_SHARE - GLOW_COOL_SHARE);
        return with_alpha_share(colours.glow_cool, 1.0 - into);
    }
    with_alpha_share(colours.glow_cool, 0.0)
}

/// The three shapes of the backdrop, from the back to the front.
pub(super) fn build(rect: Rect, colours: &BackdropColours) -> Vec<Shape> {
    vec![
        sky_and_ground(rect, colours),
        glow(rect, colours),
        horizon_line(rect, colours),
    ]
}

/// The sky and the ground: one vertical gradient over the whole of `rect`.
fn sky_and_ground(rect: Rect, colours: &BackdropColours) -> Shape {
    let mut mesh = Mesh::default();
    mesh.reserve_vertices(2 * usize::from(SKY_STRIPS + 1));
    mesh.reserve_triangles(2 * usize::from(SKY_STRIPS));

    for strip in 0..=SKY_STRIPS {
        let share = f32::from(strip) / f32::from(SKY_STRIPS);
        let y = rect.top() + rect.height() * share;
        let colour = sky_colour_at(share, colours);
        mesh.colored_vertex(Pos2::new(rect.left(), y), colour);
        mesh.colored_vertex(Pos2::new(rect.right(), y), colour);
    }

    for strip in 0..SKY_STRIPS {
        let top = 2 * u32::from(strip);
        let foot = top + 2;
        mesh.add_triangle(top, top + 1, foot);
        mesh.add_triangle(foot, top + 1, foot + 1);
    }

    Shape::mesh(mesh)
}

/// The glow: an ellipse of rings around the middle of the horizon line, with no
/// visible edge. The rings interpolate the alpha, so the shape needs no stroke.
fn glow(rect: Rect, colours: &BackdropColours) -> Shape {
    let centre = Pos2::new(rect.center().x, horizon_y(rect));
    let radius = Vec2::new(rect.width() * GLOW_RADIUS_X, rect.height() * GLOW_RADIUS_Y);

    let mut mesh = Mesh::default();
    mesh.reserve_vertices(usize::from(GLOW_RINGS + 1) * usize::from(GLOW_SEGMENTS + 1));
    mesh.reserve_triangles(2 * usize::from(GLOW_RINGS) * usize::from(GLOW_SEGMENTS));

    for ring in 0..=GLOW_RINGS {
        let distance = f32::from(ring) / f32::from(GLOW_RINGS);
        let colour = glow_colour_at(distance, colours);
        for segment in 0..=GLOW_SEGMENTS {
            let angle = std::f32::consts::TAU * f32::from(segment) / f32::from(GLOW_SEGMENTS);
            let offset = Vec2::new(angle.cos() * distance * radius.x, angle.sin() * distance * radius.y);
            mesh.colored_vertex(centre + offset, colour);
        }
    }

    let stride = u32::from(GLOW_SEGMENTS) + 1;
    for ring in 0..u32::from(GLOW_RINGS) {
        for segment in 0..u32::from(GLOW_SEGMENTS) {
            let inner = ring * stride + segment;
            let outer = inner + stride;
            mesh.add_triangle(inner, inner + 1, outer);
            mesh.add_triangle(outer, inner + 1, outer + 1);
        }
    }

    Shape::mesh(mesh)
}

/// The horizon line: one point thick, and its alpha falls to zero at both edges
/// of the canvas.
fn horizon_line(rect: Rect, colours: &BackdropColours) -> Shape {
    let y = horizon_y(rect);
    let columns: [f32; LINE_COLUMNS] = [0.0, LINE_PEAK_SHARE, LINE_FADE_SHARE, 1.0];

    let mut mesh = Mesh::default();
    mesh.reserve_vertices(2 * LINE_COLUMNS);
    mesh.reserve_triangles(2 * LINE_COLUMNS);

    for share in columns {
        let x = rect.left() + rect.width() * share;
        let colour = with_alpha_share(colours.horizon_line, horizon_line_alpha_share(share));
        mesh.colored_vertex(Pos2::new(x, y - LINE_WIDTH * 0.5), colour);
        mesh.colored_vertex(Pos2::new(x, y + LINE_WIDTH * 0.5), colour);
    }

    for span in 0..LINE_SPANS {
        let left = 2 * span;
        let right = left + 2;
        mesh.add_triangle(left, left + 1, right);
        mesh.add_triangle(right, left + 1, right + 1);
    }

    Shape::mesh(mesh)
}

/// Paint the backdrop of `rect`, and nothing outside it.
pub(super) fn paint(ui: &egui::Ui, cache: &mut BackdropCache, rect: Rect) {
    let theme = theme::current_theme();
    let pixels_per_point = ui.ctx().pixels_per_point();
    let painter = ui.painter().with_clip_rect(rect);
    for shape in cache.shapes(rect, pixels_per_point, theme) {
        painter.add(shape.clone());
    }
}

/// What the shapes of the backdrop were built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BackdropKey {
    rect_min_x: u32,
    rect_min_y: u32,
    rect_width: u32,
    rect_height: u32,
    pixels_per_point: u32,
    theme: ResolvedTheme,
}

impl BackdropKey {
    fn new(rect: Rect, pixels_per_point: f32, theme: ResolvedTheme) -> Self {
        Self {
            rect_min_x: rect.min.x.to_bits(),
            rect_min_y: rect.min.y.to_bits(),
            rect_width: rect.width().to_bits(),
            rect_height: rect.height().to_bits(),
            pixels_per_point: pixels_per_point.to_bits(),
            theme,
        }
    }
}

/// The shapes of the backdrop, and the rectangle, the pixels per point and the
/// theme that they were built for.
///
/// The shapes depend on nothing else: the pan offset and the zoom of the canvas
/// do not move them, so a frame where none of the three changed paints the shapes
/// of the frame before and builds nothing. The cache follows
/// [`super::agent_lines::AgentLines`], which keeps its shapes the same way.
#[derive(Default)]
pub(super) struct BackdropCache {
    key: Option<BackdropKey>,
    shapes: Vec<Shape>,
    /// How many times this cache built its shapes. A test reads it to prove that
    /// an unchanged canvas builds none.
    builds: u32,
}

impl BackdropCache {
    /// The shapes of the backdrop of this frame, built only when the key of the
    /// shapes changed. `theme` keys the cache, and the colours follow the active
    /// palette of that theme, which the caller resolves.
    pub(super) fn shapes(&mut self, rect: Rect, pixels_per_point: f32, theme: ResolvedTheme) -> &[Shape] {
        let key = BackdropKey::new(rect, pixels_per_point, theme);
        if self.key != Some(key) {
            self.key = Some(key);
            self.shapes = build(rect, &BackdropColours::active());
            self.builds = self.builds.saturating_add(1);
        }
        &self.shapes
    }

    /// Drops the shapes, because a theme change gives every shape new colours.
    pub(super) fn forget(&mut self) {
        self.key = None;
        self.shapes.clear();
    }
}

/// `from` and `to`, mixed by `share` at each of the four channels, on the
/// premultiplied values that a mesh interpolates between two of its vertices. A
/// share of 0 or 1 holds the colour of its end exactly.
fn mix(from: Color32, to: Color32, share: f32) -> Color32 {
    if share <= 0.0 {
        return from;
    }
    if share >= 1.0 {
        return to;
    }
    let from = Rgba::from(from);
    let to = Rgba::from(to);
    let at = |from: f32, to: f32| from + (to - from) * share;
    let mixed = Rgba::from_rgba_premultiplied(
        at(from.r(), to.r()),
        at(from.g(), to.g()),
        at(from.b(), to.b()),
        at(from.a(), to.a()),
    );
    let [red, green, blue, alpha] = mixed.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(red, green, blue, alpha)
}

/// `colour` with its alpha scaled by `share`. A premultiplied colour keeps the
/// colour that the person sees when the share scales each of its four channels.
fn with_alpha_share(colour: Color32, share: f32) -> Color32 {
    if share <= 0.0 {
        return Color32::TRANSPARENT;
    }
    if share >= 1.0 {
        return colour;
    }
    let colour = Rgba::from(colour);
    let scaled = Rgba::from_rgba_premultiplied(
        colour.r() * share,
        colour.g() * share,
        colour.b() * share,
        colour.a() * share,
    );
    let [red, green, blue, alpha] = scaled.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(red, green, blue, alpha)
}

#[cfg(test)]
mod tests;
