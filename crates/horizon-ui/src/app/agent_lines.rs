//! The threads between an agent panel and the panels that it controls.
//!
//! One thin curve runs from the edge of an agent panel to the edge of every panel
//! whose owner or opener that agent is: a card it created, a page it drives, a
//! terminal or a preset panel it opened for the person.
//! [`Board::agent_links`](horizon_core::Board::agent_links) answers the pairs from
//! the board alone.
//!
//! A curve takes the workspace accent of its agent, so two agents of one
//! workspace differ. It is 1.5 points wide at every zoom, with a filled dot at the
//! agent end and a ring at the other end. The lines of the agent in focus are
//! clear, the lines of every other agent are faint, and at a zoom where panel
//! text is no longer readable all of them take one middle strength.
//!
//! The lines are painted on their own `Order::Background` layer, created after the
//! canvas and the workspace frames, so they cover the dot grid; the panels live in
//! `Order::Middle`, so a line that passes behind a panel is hidden by it.
//!
//! The shapes are built only when the pairs, the places of the panels, the view,
//! the agent in focus or the theme change. An unchanged frame reuses them, and
//! nothing here asks for a frame of its own.

use egui::{Color32, Context, Id, LayerId, Order, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use horizon_core::{AgentLink, Board, CanvasViewState, PanelId};

use super::HorizonApp;
use crate::theme;

/// The width of a line, in points of the screen at every zoom.
const LINE_WIDTH: f32 = 1.5;
/// The radius of the filled dot at the agent end and of the ring at the other
/// end, in points of the screen.
const END_MARK_RADIUS: f32 = 3.0;
/// The strength of the lines of the agent in focus, about 0.9.
const CLEAR_ALPHA: u8 = 230;
/// The strength of the lines of every other agent, about 0.18.
const FAINT_ALPHA: u8 = 46;
/// The strength of every line at a zoom of [`OVERVIEW_ZOOM`] or below, about 0.6.
const OVERVIEW_ALPHA: u8 = 153;
/// The zoom at and below which every line takes [`OVERVIEW_ALPHA`].
///
/// It sits below the fit floor of the settings, which is 0.85 by default and
/// keeps the smallest text of a card at about 9 points: at this zoom that text is
/// under 7 points, so a person reads the shape of the board and not its text.
const OVERVIEW_ZOOM: f32 = 0.6;
/// How far a control point pulls away from its panel, as a share of the distance
/// between the two ends.
const CURVE_PULL_SHARE: f32 = 0.45;
const MIN_CURVE_PULL: f32 = 40.0;
const MAX_CURVE_PULL: f32 = 260.0;
/// The share of the gap between the two facing edges that one control point may
/// cross.
///
/// A control point that pulls further than half of that gap passes the other
/// panel, and the curve then folds back under the two panels and draws a curl
/// beside the agent. A short link stays a plain curve in the gap instead.
const CURVE_PULL_GAP_SHARE: f32 = 0.5;
/// The closest that the two facing edges of a pair may stand, in points.
///
/// The dot and the ring of a shorter line would merge into one mark, and two
/// panels that stand that close already say that one belongs to the other.
/// The rule reads the two panels of the pair, never the two starts: a start
/// follows the number of lines that leave one side and the place of the other
/// card, so a card beside its agent would draw its line in one frame and lose it
/// in the next.
const MIN_LINK_GAP: f32 = 8.0;
/// How many points a curve is drawn with.
const CURVE_SAMPLES: usize = 24;

/// The edge of a panel where a line starts or ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    /// The side of a panel that faces the other panel of the pair.
    fn facing(agent: Rect, target: Rect) -> Self {
        let delta = target.center() - agent.center();
        if delta.x.abs() >= delta.y.abs() {
            if delta.x >= 0.0 { Self::Right } else { Self::Left }
        } else if delta.y >= 0.0 {
            Self::Bottom
        } else {
            Self::Top
        }
    }

    const fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }

    /// The direction that leaves a panel of that side, which a control point
    /// follows so the curve leaves the panel at a right angle.
    const fn outward(self) -> Vec2 {
        match self {
            Self::Left => Vec2::new(-1.0, 0.0),
            Self::Right => Vec2::new(1.0, 0.0),
            Self::Top => Vec2::new(0.0, -1.0),
            Self::Bottom => Vec2::new(0.0, 1.0),
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Right => 1,
            Self::Top => 2,
            Self::Bottom => 3,
        }
    }

    /// Whether the spread of the starts of this side runs along `y`.
    const fn runs_down(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    /// The middle of that side of `rect`.
    fn middle(self, rect: Rect) -> Pos2 {
        match self {
            Self::Left => Pos2::new(rect.min.x, rect.center().y),
            Self::Right => Pos2::new(rect.max.x, rect.center().y),
            Self::Top => Pos2::new(rect.center().x, rect.min.y),
            Self::Bottom => Pos2::new(rect.center().x, rect.max.y),
        }
    }

    /// The `slot` of `count` places along that side of `rect`, so the lines of one
    /// agent leave it spread out instead of on one point.
    #[expect(
        clippy::cast_precision_loss,
        reason = "a slot and a count of the lines of one agent are small"
    )]
    fn spread(self, rect: Rect, slot: usize, count: usize) -> Pos2 {
        let share = (slot + 1) as f32 / (count + 1) as f32;
        match self {
            Self::Left => Pos2::new(rect.min.x, rect.min.y + rect.height() * share),
            Self::Right => Pos2::new(rect.max.x, rect.min.y + rect.height() * share),
            Self::Top => Pos2::new(rect.min.x + rect.width() * share, rect.min.y),
            Self::Bottom => Pos2::new(rect.min.x + rect.width() * share, rect.max.y),
        }
    }
}

/// The geometry of one line: two ends and the two control points of a cubic
/// Bezier that leaves each panel at a right angle, in points of the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LinePath {
    pub(super) start: Pos2,
    pub(super) control_start: Pos2,
    pub(super) control_end: Pos2,
    pub(super) end: Pos2,
    pub(super) color: Color32,
}

impl LinePath {
    /// Whether this line can show on a canvas of `canvas`.
    ///
    /// A cubic Bezier stays inside the box of its four points, so this box decides
    /// that a line which runs off the canvas builds no shape at all.
    fn touches(self, canvas: Rect) -> bool {
        Rect::from_points(&[self.start, self.control_start, self.control_end, self.end]).intersects(canvas)
    }

    /// The points of the curve of this line.
    fn points(self) -> Vec<Pos2> {
        let mut points = Vec::with_capacity(CURVE_SAMPLES + 1);
        for index in 0..=CURVE_SAMPLES {
            points.push(cubic_bezier_point(self, index));
        }
        points
    }
}

/// The point at `step` of [`CURVE_SAMPLES`] of the curve of `path`.
#[expect(
    clippy::cast_precision_loss,
    reason = "a step of the curve is bounded by CURVE_SAMPLES (24)"
)]
fn cubic_bezier_point(path: LinePath, step: usize) -> Pos2 {
    let count = CURVE_SAMPLES as f32;
    let step = step as f32;
    let t = step / count;
    let u = 1.0 - t;
    let point = path.start.to_vec2() * (u * u * u)
        + path.control_start.to_vec2() * (3.0 * u * u * t)
        + path.control_end.to_vec2() * (3.0 * u * t * t)
        + path.end.to_vec2() * (t * t * t);
    Pos2::new(point.x, point.y)
}

/// The paths of this frame, from the places of the panels that the pairs name.
///
/// A pair that this canvas does not draw, two panels that overlap, and two panels
/// whose facing edges stand closer than [`MIN_LINK_GAP`] draw no path: a start
/// must lie on the edge of the agent panel and outside the other one.
pub(super) fn line_paths(
    links: &[AgentLink],
    places: &[(PanelId, Option<Rect>)],
    canvas: Rect,
    focus: Option<PanelId>,
    zoom: f32,
    theme: theme::ResolvedTheme,
) -> Vec<LinePath> {
    let mut paths = Vec::new();
    let mut index = 0;
    while index < links.len() {
        let agent = links[index].agent;
        let count = links[index..].iter().take_while(|link| link.agent == agent).count();
        paths.extend(agent_paths(
            &links[index..index + count],
            places,
            canvas,
            line_color(links[index].accent, agent, focus, zoom, theme),
        ));
        index += count;
    }
    paths
}

/// The colour of the lines of one agent, at the strength that this frame gives
/// them.
fn line_color(
    accent: usize,
    agent: PanelId,
    focus: Option<PanelId>,
    zoom: f32,
    theme: theme::ResolvedTheme,
) -> Color32 {
    theme::agent_line_color(theme, accent, line_alpha(agent, focus, zoom))
}

/// The strength of the line of one pair, in alpha.
///
/// A zoom at or below [`OVERVIEW_ZOOM`] gives every line the middle strength, so
/// the shape of the whole board reads before the agents differ. Above it the lines
/// of the agent in focus are clear and every other line is faint.
pub(super) fn line_alpha(agent: PanelId, focus: Option<PanelId>, zoom: f32) -> u8 {
    if !zoom.is_finite() || zoom <= OVERVIEW_ZOOM {
        return OVERVIEW_ALPHA;
    }
    if Some(agent) == focus { CLEAR_ALPHA } else { FAINT_ALPHA }
}

/// The paths of the pairs of one agent, with the starts spread along the side
/// that the lines leave in the order of their ends.
fn agent_paths(links: &[AgentLink], places: &[(PanelId, Option<Rect>)], canvas: Rect, color: Color32) -> Vec<LinePath> {
    let mut paths = Vec::new();
    let mut by_side: [Vec<(Rect, Rect)>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for link in links {
        let (Some(agent), Some(target)) = (place_of(places, link.agent), place_of(places, link.target)) else {
            continue;
        };
        if overlaps(agent, target) {
            continue;
        }
        let side = Side::facing(agent, target);
        by_side[side.index()].push((agent, target));
    }
    for (index, mut group) in by_side.into_iter().enumerate() {
        let side = match index {
            0 => Side::Left,
            1 => Side::Right,
            2 => Side::Top,
            _ => Side::Bottom,
        };
        // The end that sits first along the side takes the first place, so the
        // lines of one agent do not cross each other near it.
        group.sort_by(|(_, first), (_, second)| {
            let (first, second) = if side.runs_down() {
                (first.center().y, second.center().y)
            } else {
                (first.center().x, second.center().x)
            };
            first.total_cmp(&second)
        });
        let count = group.len();
        for (slot, (agent, target)) in group.into_iter().enumerate() {
            let start = side.spread(agent, slot, count);
            let end = side.opposite().middle(target);
            if let Some(path) = curve(start, end, side, agent, target, color)
                && path.touches(canvas)
            {
                paths.push(path);
            }
        }
    }
    paths
}

/// The curve from `start` on `side` to `end`, or `None` when the two panels of the
/// pair stand closer than [`MIN_LINK_GAP`].
fn curve(start: Pos2, end: Pos2, side: Side, agent: Rect, target: Rect, color: Color32) -> Option<LinePath> {
    if separation(agent, target) < MIN_LINK_GAP {
        return None;
    }
    let distance = (end - start).length();
    let pull = (distance * CURVE_PULL_SHARE)
        .clamp(MIN_CURVE_PULL, MAX_CURVE_PULL)
        .min(facing_gap(agent, target, side).max(0.0) * CURVE_PULL_GAP_SHARE);
    let outward = side.outward();
    Some(LinePath {
        start,
        control_start: start + outward * pull,
        control_end: end - outward * pull,
        end,
        color,
    })
}

/// The place of `panel_id` in this frame, when this canvas draws that panel.
fn place_of(places: &[(PanelId, Option<Rect>)], panel_id: PanelId) -> Option<Rect> {
    places
        .iter()
        .find(|(id, _)| *id == panel_id)
        .and_then(|(_, place)| *place)
}

/// Whether two panels cover a common area, so no curve can run between them.
fn overlaps(first: Rect, second: Rect) -> bool {
    let overlap = Rect::from_min_max(first.min.max(second.min), first.max.min(second.max));
    overlap.width() > 0.0 && overlap.height() > 0.0
}

/// The distance between the two facing edges of a pair, along the axis of those
/// sides, in points. A negative value says that the two panels overlap on that
/// axis, so the side that faces the other panel is the other axis.
fn facing_gap(agent: Rect, target: Rect, side: Side) -> f32 {
    match side {
        Side::Left => agent.min.x - target.max.x,
        Side::Right => target.min.x - agent.max.x,
        Side::Top => agent.min.y - target.max.y,
        Side::Bottom => target.min.y - agent.max.y,
    }
}

/// The distance between the edges of two panels, in points: zero when the two
/// touch or cover a common area.
///
/// A pair is measured on the two panels and not on the two ends of its line, so a
/// line does not come and go with the places that the starts take along one side.
fn separation(first: Rect, second: Rect) -> f32 {
    let apart = Vec2::new(
        (second.min.x - first.max.x).max(first.min.x - second.max.x).max(0.0),
        (second.min.y - first.max.y).max(first.min.y - second.max.y).max(0.0),
    );
    apart.length()
}

/// The agent that a frame shows in focus.
///
/// The panel under the pointer names its agent, and that agent wins while the
/// pointer rests there. Without a pointer over the panel of an agent, the panel
/// that holds the keyboard names the agent: the agent panel itself, or the agent
/// that controls it. A frame with neither keeps every line faint.
pub(super) fn focus_agent(board: &Board, links: &[AgentLink], hovered: Option<PanelId>) -> Option<PanelId> {
    hovered
        .and_then(|panel_id| agent_of(board, links, panel_id))
        .or_else(|| board.focused.and_then(|panel_id| agent_of(board, links, panel_id)))
}

/// The agent that a panel stands for: the panel itself when it is an agent panel,
/// or the agent that controls it.
fn agent_of(board: &Board, links: &[AgentLink], panel_id: PanelId) -> Option<PanelId> {
    if board.panel(panel_id).is_some_and(|panel| panel.kind.is_agent()) {
        return Some(panel_id);
    }
    links.iter().find(|link| link.target == panel_id).map(|link| link.agent)
}

/// The bits of a rectangle, so a cache key compares places with no float compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PlaceKey {
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
}

impl PlaceKey {
    fn of(place: Option<Rect>) -> Option<Self> {
        place.map(|rect| Self {
            min_x: rect.min.x.to_bits(),
            min_y: rect.min.y.to_bits(),
            max_x: rect.max.x.to_bits(),
            max_y: rect.max.y.to_bits(),
        })
    }
}

/// What one frame's shapes were built for.
struct Built {
    links: Vec<AgentLink>,
    places: Vec<(PanelId, Option<PlaceKey>)>,
    canvas: Option<PlaceKey>,
    zoom: u32,
    pan: [u32; 2],
    focus: Option<PanelId>,
    theme: theme::ResolvedTheme,
    shapes: Vec<Shape>,
}

/// The shapes of the lines, and the places that this frame read for them.
///
/// The vectors keep their room for the next frame, so a board whose pairs do not
/// grow makes no allocation for them. A frame where nothing that the key holds
/// changed builds no shape at all.
#[derive(Default)]
pub(super) struct AgentLines {
    /// The pairs of this frame.
    links: Vec<AgentLink>,
    /// The place of the two ends of every pair of this frame.
    places: Vec<(PanelId, Option<Rect>)>,
    built: Option<Built>,
    /// How many times this cache built its shapes. A test reads it to prove that
    /// an unchanged frame builds none.
    builds: u32,
}

impl AgentLines {
    /// The shapes of this frame, built only when the key of the shapes changed.
    pub(super) fn shapes(
        &mut self,
        canvas: Rect,
        view: CanvasViewState,
        focus: Option<PanelId>,
        theme: theme::ResolvedTheme,
        places: &[(PanelId, Option<Rect>)],
    ) -> &[Shape] {
        let unchanged = self
            .built
            .as_ref()
            .is_some_and(|built| built.matches(&self.links, canvas, view, focus, theme, places));
        if unchanged {
            return self.built.as_ref().map_or(&[], |built| &built.shapes);
        }
        let paths = line_paths(&self.links, places, canvas, focus, view.zoom, theme);
        self.builds = self.builds.saturating_add(1);
        let built = self.built.get_or_insert_with(|| Built {
            links: Vec::new(),
            places: Vec::new(),
            canvas: None,
            zoom: 0,
            pan: [0, 0],
            focus: None,
            theme,
            shapes: Vec::new(),
        });
        built.links.clear();
        built.links.extend_from_slice(&self.links);
        built.places.clear();
        built
            .places
            .extend(places.iter().map(|(id, place)| (*id, PlaceKey::of(*place))));
        built.canvas = PlaceKey::of(Some(canvas));
        built.zoom = view.zoom.to_bits();
        built.pan = [view.pan_offset[0].to_bits(), view.pan_offset[1].to_bits()];
        built.focus = focus;
        built.theme = theme;
        built.shapes.clear();
        for path in paths {
            built
                .shapes
                .push(Shape::line(path.points(), Stroke::new(LINE_WIDTH, path.color)));
            built
                .shapes
                .push(Shape::circle_filled(path.start, END_MARK_RADIUS, path.color));
            built.shapes.push(Shape::circle_stroke(
                path.end,
                END_MARK_RADIUS,
                Stroke::new(LINE_WIDTH, path.color),
            ));
        }
        &built.shapes
    }

    /// Drop the shapes and the pairs, for a frame that draws no line.
    pub(super) fn forget(&mut self) {
        self.links.clear();
        self.places.clear();
        self.built = None;
    }
}

impl Built {
    /// Whether the shapes of this entry are the shapes of that frame.
    fn matches(
        &self,
        links: &[AgentLink],
        canvas: Rect,
        view: CanvasViewState,
        focus: Option<PanelId>,
        theme: theme::ResolvedTheme,
        places: &[(PanelId, Option<Rect>)],
    ) -> bool {
        self.links == links
            && self.canvas == PlaceKey::of(Some(canvas))
            && self.zoom == view.zoom.to_bits()
            && self.pan == [view.pan_offset[0].to_bits(), view.pan_offset[1].to_bits()]
            && self.focus == focus
            && self.theme == theme
            && self.places.len() == places.len()
            && self
                .places
                .iter()
                .zip(places)
                .all(|((id, key), (place_id, place))| id == place_id && *key == PlaceKey::of(*place))
    }
}

impl HorizonApp {
    /// Draw the lines between the agents and the panels that they control on the
    /// canvas of the root window.
    ///
    /// It runs after the canvas and the workspace frames and before the panels. No
    /// line reaches a panel of a detached workspace, a hidden panel, a panel behind
    /// a fullscreen one or a panel that this canvas does not draw.
    pub(super) fn render_agent_lines(&mut self, ctx: &Context, canvas_rect: Rect) {
        if !self.template_config.canvas.agent_lines || !self.board.has_agent_links() {
            self.agent_lines.forget();
            return;
        }
        self.board.fill_agent_links(&mut self.agent_lines.links);
        let mut places = std::mem::take(&mut self.agent_lines.places);
        places.clear();
        for link in &self.agent_lines.links {
            for panel_id in [link.agent, link.target] {
                let place = self
                    .board
                    .panel(panel_id)
                    .filter(|panel| panel.visible && !self.workspace_is_detached(panel.workspace_id))
                    .map(|panel| self.panel_screen_place(panel, canvas_rect));
                places.push((panel_id, place));
            }
        }
        let focus = self.agent_line_focus(ctx);
        let view = self.canvas_view;
        let theme = theme::current_theme();
        // A workspace frame is a layer of its own in `Order::Background`, and the
        // layer of a workspace that a person creates in this run comes after this
        // one. The last place of that order keeps the lines above every workspace
        // frame and below the panels of `Order::Middle`.
        let layer = LayerId::new(Order::Background, Id::unique("agent_lines"));
        ctx.move_to_top(layer);
        let painter = ctx.layer_painter(layer).with_clip_rect(canvas_rect);
        for shape in self.agent_lines.shapes(canvas_rect, view, focus, theme, &places) {
            paint(&painter, shape);
        }
        self.agent_lines.places = places;
    }

    /// The agent that this frame shows in focus, from what the frame already
    /// knows: the panel under the pointer wins while the pointer rests on one,
    /// then the panel that holds the keyboard.
    fn agent_line_focus(&self, ctx: &Context) -> Option<PanelId> {
        let hovered = ctx.input(|input| input.pointer.hover_pos()).and_then(|position| {
            self.panel_screen_order.iter().rev().copied().find(|panel_id| {
                self.panel_screen_rects
                    .get(panel_id)
                    .is_some_and(|rect| rect.contains(position))
            })
        });
        focus_agent(&self.board, &self.agent_lines.links, hovered)
    }
}

/// Paint one shape of the cache. The painter takes the shape by value, so a frame
/// that reuses the cache hands it a copy of a shape and builds nothing.
fn paint(painter: &Painter, shape: &Shape) {
    painter.add(shape.clone());
}

#[cfg(test)]
mod tests;
