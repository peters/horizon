//! Connector lines linking an agent panel to the browser panels it drives.
//!
//! The link data comes from each browser panel's manifest: while an agent's
//! MCP adapter drives a browser it heartbeats an owner whose name is the
//! agent panel's actor identity. A line is drawn only while that owner
//! heartbeat is fresh and both panels are visible in the same viewport, so a
//! handoff or an idle agent removes the line automatically.
//!
//! Lines are painted on the canvas layer after the workspace backgrounds and
//! before the panels, so panel bodies cover the line wherever they overlap.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::{Color32, Context, LayerId, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use horizon_core::browser::manifest;
use horizon_core::{Panel, PanelId, PanelKind, WorkspaceId, browser_actor};

use super::{HorizonApp, theme};

/// How often a browser panel's manifest is re-read for its live owner. The
/// owner TTL is 10 s, so a one-second cadence keeps line lifetimes honest
/// without touching disk more than once a second per browser panel.
const OWNER_REFRESH_INTERVAL: Duration = Duration::from_millis(1_000);
const LINE_WIDTH: f32 = 1.5;
const LINE_ALPHA: u8 = 150;
const DOT_RADIUS: f32 = 3.0;
const ARROW_LENGTH: f32 = 8.0;
const ARROW_HALF_WIDTH: f32 = 3.5;
const CURVE_SAMPLES: i32 = 24;
const MIN_CURVE_PULL: f32 = 40.0;
const MAX_CURVE_PULL: f32 = 260.0;
/// Below this anchor distance the curve degenerates for touching panels, so
/// no line is drawn: panel adjacency already shows the relationship and the
/// browser chrome chip carries the owner.
const MIN_ANCHOR_GAP: f32 = 48.0;

impl HorizonApp {
    /// Draw one line per browser panel whose live owner is an agent panel
    /// visible in the same viewport. Must run after the workspace
    /// backgrounds and before the panels so the panels cover the lines.
    pub(super) fn render_browser_connector_lines(
        &mut self,
        ctx: &Context,
        canvas_rect: Rect,
        visible_workspace: Option<WorkspaceId>,
    ) {
        let geometries = self.visible_panel_geometry_for_canvas_view(canvas_rect, visible_workspace);
        if geometries.is_empty() {
            return;
        }
        let rect_of: HashMap<PanelId, Rect> = geometries
            .iter()
            .map(|(panel_id, geometry)| (*panel_id, geometry.screen_rect))
            .collect();
        let now = Instant::now();
        let mut links: Vec<(Rect, Rect)> = Vec::new();
        let browsers: Vec<(PanelId, String)> = self
            .board
            .panels
            .iter()
            .filter(|panel| {
                panel.kind == PanelKind::Browser
                    && panel.visible
                    && self.connector_panel_in_scope(panel, visible_workspace)
            })
            .map(|panel| (panel.id, panel.local_id.clone()))
            .collect();
        for (browser_id, browser_local_id) in browsers {
            let Some(browser_rect) = rect_of.get(&browser_id).copied() else {
                continue;
            };
            let Some(actor) = self.owner_actor_for(&browser_local_id, now) else {
                continue;
            };
            let Some(agent_rect) = self
                .board
                .panels
                .iter()
                .filter(|agent| {
                    agent.kind.is_agent()
                        && agent.visible
                        && self.connector_panel_in_scope(agent, visible_workspace)
                        && browser_actor(&agent.local_id) == actor
                })
                .find_map(|agent| rect_of.get(&agent.id).copied())
            else {
                continue;
            };
            links.push((agent_rect, browser_rect));
        }
        if links.is_empty() {
            return;
        }
        let painter = ctx.layer_painter(LayerId::background());
        for (agent_rect, browser_rect) in links {
            paint_connector(&painter, agent_rect, browser_rect, theme::PALETTE_CYAN());
        }
    }

    fn connector_panel_in_scope(&self, panel: &Panel, visible_workspace: Option<WorkspaceId>) -> bool {
        match visible_workspace {
            Some(workspace_id) => panel.workspace_id == workspace_id,
            None => !self.workspace_is_detached(panel.workspace_id),
        }
    }

    /// The browser panel's live owner actor, refreshed at most once per
    /// [`OWNER_REFRESH_INTERVAL`] per panel so the manifest is not re-read
    /// every frame.
    fn owner_actor_for(&mut self, browser_local_id: &str, now: Instant) -> Option<String> {
        let cached = self.browser_owner_links.get(browser_local_id);
        let stale = cached.is_none_or(|(_, fetched_at)| now.duration_since(*fetched_at) >= OWNER_REFRESH_INTERVAL);
        if stale {
            let actor = manifest::read(browser_local_id)
                .and_then(|manifest| {
                    manifest
                        .live_owner(manifest::now_millis())
                        .map(|owner| owner.name.clone())
                })
                .filter(|actor| !actor.is_empty());
            self.browser_owner_links
                .insert(browser_local_id.to_string(), (actor.clone(), now));
            return actor;
        }
        cached.and_then(|(actor, _)| actor.clone())
    }
}

/// Cubic-bezier anchors: each panel uses the edge that faces the other, and
/// the control points pull outward along that edge's normal so the curve
/// leaves and arrives perpendicular to the panel borders. `None` when the
/// panels touch and the curve would be a hidden sliver.
fn connector_anchors(agent_rect: Rect, browser_rect: Rect) -> Option<(Pos2, Pos2, Pos2, Pos2)> {
    let pull = ((browser_rect.center() - agent_rect.center()).length() * 0.45).clamp(MIN_CURVE_PULL, MAX_CURVE_PULL);
    let (start, c1, c2, end) = if (browser_rect.center().x - agent_rect.center().x).abs()
        >= (browser_rect.center().y - agent_rect.center().y).abs()
    {
        if browser_rect.center().x >= agent_rect.center().x {
            let start = Pos2::new(agent_rect.max.x, agent_rect.center().y);
            let end = Pos2::new(browser_rect.min.x, browser_rect.center().y);
            (start, start + Vec2::new(pull, 0.0), end + Vec2::new(-pull, 0.0), end)
        } else {
            let start = Pos2::new(agent_rect.min.x, agent_rect.center().y);
            let end = Pos2::new(browser_rect.max.x, browser_rect.center().y);
            (start, start + Vec2::new(-pull, 0.0), end + Vec2::new(pull, 0.0), end)
        }
    } else if browser_rect.center().y >= agent_rect.center().y {
        let start = Pos2::new(agent_rect.center().x, agent_rect.max.y);
        let end = Pos2::new(browser_rect.center().x, browser_rect.min.y);
        (start, start + Vec2::new(0.0, pull), end + Vec2::new(0.0, -pull), end)
    } else {
        let start = Pos2::new(agent_rect.center().x, agent_rect.min.y);
        let end = Pos2::new(browser_rect.center().x, browser_rect.max.y);
        (start, start + Vec2::new(0.0, -pull), end + Vec2::new(0.0, pull), end)
    };
    let gap = (start - end).length();
    (gap >= MIN_ANCHOR_GAP).then_some((start, c1, c2, end))
}

#[expect(
    clippy::cast_precision_loss,
    reason = "bezier sample indices are bounded by CURVE_SAMPLES (24)"
)]
fn cubic_bezier_points(p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, samples: i32) -> Vec<Pos2> {
    (0..=samples)
        .map(|i| {
            let t = i as f32 / samples as f32;
            let u = 1.0 - t;
            let point = p0.to_vec2() * (u * u * u)
                + p1.to_vec2() * (3.0 * u * u * t)
                + p2.to_vec2() * (3.0 * u * t * t)
                + p3.to_vec2() * (t * t * t);
            Pos2::new(point.x, point.y)
        })
        .collect()
}

fn paint_connector(painter: &Painter, agent_rect: Rect, browser_rect: Rect, color: Color32) {
    let Some((start, c1, c2, end)) = connector_anchors(agent_rect, browser_rect) else {
        return;
    };
    let points = cubic_bezier_points(start, c1, c2, end, CURVE_SAMPLES);
    let stroke = Stroke::new(LINE_WIDTH, theme::alpha(color, LINE_ALPHA));
    painter.add(Shape::line(points, stroke));
    // Source dot on the agent panel edge.
    painter.circle_filled(start, DOT_RADIUS, color);
    // Arrowhead at the browser panel edge, along the curve's end tangent.
    let raw_tangent = end - c2;
    let tangent = if raw_tangent.length() > f32::EPSILON {
        raw_tangent / raw_tangent.length()
    } else {
        Vec2::ZERO
    };
    if tangent != Vec2::ZERO {
        let back = tangent * ARROW_LENGTH;
        let side = tangent.rot90() * ARROW_HALF_WIDTH;
        painter.add(Shape::convex_polygon(
            [end, end - back + side, end - back - side].to_vec(),
            color,
            stroke,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
    }

    #[test]
    fn anchors_pick_the_facing_edges_horizontally() {
        let (start, _, _, end) =
            connector_anchors(rect(0.0, 0.0, 100.0, 80.0), rect(300.0, 20.0, 100.0, 80.0)).expect("non-degenerate");
        assert_eq!(start, Pos2::new(100.0, 40.0), "agent right edge, mid height");
        assert_eq!(end, Pos2::new(300.0, 60.0), "browser left edge, mid height");

        let (start, _, _, end) =
            connector_anchors(rect(300.0, 0.0, 100.0, 80.0), rect(0.0, 20.0, 100.0, 80.0)).expect("non-degenerate");
        assert_eq!(
            start,
            Pos2::new(300.0, 40.0),
            "agent left edge facing right-hand browser"
        );
        assert_eq!(end, Pos2::new(100.0, 60.0));
    }

    #[test]
    fn anchors_pick_the_facing_edges_vertically() {
        let (start, _, _, end) =
            connector_anchors(rect(0.0, 0.0, 100.0, 80.0), rect(10.0, 300.0, 100.0, 80.0)).expect("non-degenerate");
        assert_eq!(start, Pos2::new(50.0, 80.0), "agent bottom edge, mid width");
        assert_eq!(end, Pos2::new(60.0, 300.0), "browser top edge, mid width");

        let (start, _, _, end) =
            connector_anchors(rect(0.0, 300.0, 100.0, 80.0), rect(10.0, 0.0, 100.0, 80.0)).expect("non-degenerate");
        assert_eq!(start, Pos2::new(50.0, 300.0), "agent top edge facing browser above");
        assert_eq!(end, Pos2::new(60.0, 80.0));
    }

    #[test]
    fn bezier_samples_end_at_the_anchors() {
        let points = cubic_bezier_points(
            Pos2::ZERO,
            Pos2::new(40.0, 0.0),
            Pos2::new(40.0, 10.0),
            Pos2::new(80.0, 10.0),
            8,
        );
        assert_eq!(points.len(), 9);
        assert_eq!(points.first(), Some(&Pos2::ZERO));
        assert_eq!(points.last(), Some(&Pos2::new(80.0, 10.0)));
    }
}

#[cfg(test)]
mod degenerate_tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
    }

    #[test]
    fn touching_panels_skip_the_connector() {
        assert!(
            connector_anchors(rect(0.0, 330.0, 700.0, 420.0), rect(0.0, 0.0, 700.0, 330.0)).is_none(),
            "zero-gap panels have a degenerate curve"
        );
        assert!(
            connector_anchors(rect(0.0, 0.0, 700.0, 400.0), rect(0.0, 380.0, 700.0, 400.0)).is_none(),
            "a 20 px gap is below the minimum anchor distance"
        );
        assert!(
            connector_anchors(rect(0.0, 0.0, 700.0, 400.0), rect(0.0, 460.0, 700.0, 400.0)).is_some(),
            "a 60 px gap still draws"
        );
    }
}
