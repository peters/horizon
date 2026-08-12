use std::collections::HashMap;

use egui::{Color32, Context, CornerRadius, Id, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2};
use horizon_core::{PanelId, WorkspaceId};

use crate::theme;

use super::{HorizonApp, MINIMAP_MARGIN, MINIMAP_PAD, WS_BG_PAD, WS_EMPTY_SIZE, WS_TITLE_HEIGHT};

mod interaction;
mod labels;

use interaction::{minimap_panels_in_paint_order, render_scoped_minimap, scope_includes_workspace};
use labels::paint_minimap_workspace_labels;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MinimapScope {
    Attached,
    Workspace(WorkspaceId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MinimapHitTarget {
    Panel {
        panel_id: PanelId,
        workspace_id: WorkspaceId,
    },
    Workspace(WorkspaceId),
}

impl MinimapHitTarget {
    fn workspace_id(self) -> WorkspaceId {
        match self {
            Self::Panel { workspace_id, .. } | Self::Workspace(workspace_id) => workspace_id,
        }
    }
}

struct MinimapModel {
    content_min: [f32; 2],
    scale_x: f32,
    scale_y: f32,
    outer_size: Vec2,
    view_min: Pos2,
    view_max: Pos2,
}

impl HorizonApp {
    pub(super) fn render_minimap(
        &mut self,
        ctx: &Context,
        workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    ) -> f32 {
        render_scoped_minimap(
            self,
            ctx,
            workspace_bounds,
            self.canvas_rect(ctx),
            MinimapScope::Attached,
            Id::new("minimap_overlay"),
        )
    }

    pub(super) fn render_workspace_minimap(
        &mut self,
        ctx: &Context,
        workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
        workspace_id: WorkspaceId,
        canvas_rect: Rect,
        overlay_id: Id,
    ) -> f32 {
        render_scoped_minimap(
            self,
            ctx,
            workspace_bounds,
            canvas_rect,
            MinimapScope::Workspace(workspace_id),
            overlay_id,
        )
    }
}

fn minimap_model(
    app: &HorizonApp,
    canvas_rect: Rect,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    scope: MinimapScope,
) -> Option<MinimapModel> {
    let (content_min, content_max) = workspace_content_bounds(app, workspace_bounds, scope)?;
    let view_min = app.screen_to_canvas(canvas_rect, canvas_rect.min);
    let view_max = app.screen_to_canvas(canvas_rect, canvas_rect.max);

    let content_w = content_max[0] - content_min[0];
    let content_h = content_max[1] - content_min[1];
    if content_w < 1.0 || content_h < 1.0 {
        return None;
    }

    let overlays = &app.template_config.overlays;
    let map_w = overlays.minimap_width.max(120.0);
    let map_h = overlays.minimap_height.max(120.0);

    Some(MinimapModel {
        content_min,
        scale_x: map_w / content_w,
        scale_y: map_h / content_h,
        outer_size: Vec2::new(map_w + MINIMAP_PAD * 2.0, map_h + MINIMAP_PAD * 2.0),
        view_min,
        view_max,
    })
}

fn paint_minimap_contents(
    app: &HorizonApp,
    painter: &Painter,
    rect: Rect,
    model: &MinimapModel,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    scope: MinimapScope,
    hovered: Option<MinimapHitTarget>,
) {
    painter.rect_filled(rect, CornerRadius::same(8), theme::alpha(theme::BG_ELEVATED(), 220));
    painter.rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0_f32, theme::alpha(theme::BORDER_SUBTLE(), 180)),
        StrokeKind::Outside,
    );

    let origin = rect.min;
    let hovered_workspace = hovered.map(MinimapHitTarget::workspace_id);
    paint_minimap_workspaces(app, painter, origin, model, workspace_bounds, scope, hovered_workspace);
    paint_minimap_panels(app, painter, origin, model, scope);
    paint_minimap_workspace_labels(app, painter, origin, model, workspace_bounds, scope);
    paint_minimap_viewport(painter, origin, model);
}

fn paint_minimap_workspaces(
    app: &HorizonApp,
    painter: &Painter,
    origin: Pos2,
    model: &MinimapModel,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    scope: MinimapScope,
    hovered_workspace: Option<WorkspaceId>,
) {
    for workspace in &app.board.workspaces {
        if !scope_includes_workspace(app, scope, workspace.id) {
            continue;
        }
        let (r, g, b) = workspace.accent();
        let workspace_color = Color32::from_rgb(r, g, b);
        let is_active =
            app.board.active_workspace == Some(workspace.id) || scope == MinimapScope::Workspace(workspace.id);
        let is_hovered = hovered_workspace == Some(workspace.id);
        let workspace_rect =
            workspace_minimap_screen_rect(origin, model, workspace.id, workspace.position, workspace_bounds);

        let (fill_alpha, stroke_alpha) = workspace_style_alpha(is_active, is_hovered);
        painter.rect_filled(
            workspace_rect,
            CornerRadius::same(2),
            theme::alpha(workspace_color, fill_alpha),
        );
        painter.rect_stroke(
            workspace_rect,
            CornerRadius::same(2),
            Stroke::new(0.8_f32, theme::alpha(workspace_color, stroke_alpha)),
            StrokeKind::Outside,
        );

        if is_active && scope == MinimapScope::Attached {
            painter.rect_stroke(
                workspace_rect.expand(3.0),
                CornerRadius::same(4),
                Stroke::new(2.0_f32, theme::alpha(theme::ACCENT(), 160)),
                StrokeKind::Outside,
            );
        }
    }
}

fn workspace_style_alpha(is_active: bool, is_hovered: bool) -> (u8, u8) {
    match (is_active, is_hovered) {
        (true, true) => (78, 240),
        (true, false) => (60, 210),
        (false, true) => (34, 180),
        (false, false) => (22, 80),
    }
}

fn paint_minimap_panels(app: &HorizonApp, painter: &Painter, origin: Pos2, model: &MinimapModel, scope: MinimapScope) {
    for panel in minimap_panels_in_paint_order(app, scope) {
        let panel_rect = panel_minimap_screen_rect(origin, model, panel.layout.position, panel.layout.size);
        let workspace_color = app
            .board
            .workspace(panel.workspace_id)
            .map_or(theme::ACCENT(), |workspace| {
                let (r, g, b) = workspace.accent();
                Color32::from_rgb(r, g, b)
            });
        let is_focused = app.board.focused == Some(panel.id);

        painter.rect_filled(
            panel_rect,
            CornerRadius::same(1),
            theme::alpha(workspace_color, if is_focused { 120 } else { 70 }),
        );
        if is_focused {
            painter.rect_stroke(
                panel_rect,
                CornerRadius::same(1),
                Stroke::new(1.0_f32, theme::alpha(theme::FG(), 220)),
                StrokeKind::Outside,
            );
        }
    }
}

fn workspace_content_bounds(
    app: &HorizonApp,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    scope: MinimapScope,
) -> Option<([f32; 2], [f32; 2])> {
    let mut content_min = [f32::MAX, f32::MAX];
    let mut content_max = [f32::MIN, f32::MIN];
    let mut has_content = false;

    for workspace in &app.board.workspaces {
        if !scope_includes_workspace(app, scope, workspace.id) {
            continue;
        }
        let (workspace_min, workspace_max) =
            workspace_minimap_bounds(workspace.id, workspace_bounds).unwrap_or_else(|| {
                let pos = workspace.position;
                (pos, [pos[0] + WS_EMPTY_SIZE[0], pos[1] + WS_EMPTY_SIZE[1]])
            });
        content_min[0] = content_min[0].min(workspace_min[0]);
        content_min[1] = content_min[1].min(workspace_min[1]);
        content_max[0] = content_max[0].max(workspace_max[0]);
        content_max[1] = content_max[1].max(workspace_max[1]);
        has_content = true;
    }

    has_content.then_some((content_min, content_max))
}

fn workspace_minimap_bounds(
    workspace_id: WorkspaceId,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
) -> Option<([f32; 2], [f32; 2])> {
    workspace_bounds
        .get(&workspace_id)
        .copied()
        .map(|(workspace_min, workspace_max)| {
            (
                [
                    workspace_min[0] - WS_BG_PAD,
                    workspace_min[1] - WS_BG_PAD - WS_TITLE_HEIGHT,
                ],
                [workspace_max[0] + WS_BG_PAD, workspace_max[1] + WS_BG_PAD],
            )
        })
}

fn minimap_point(model: &MinimapModel, canvas_x: f32, canvas_y: f32) -> Pos2 {
    Pos2::new(
        MINIMAP_PAD + (canvas_x - model.content_min[0]) * model.scale_x,
        MINIMAP_PAD + (canvas_y - model.content_min[1]) * model.scale_y,
    )
}

fn workspace_minimap_screen_rect(
    origin: Pos2,
    model: &MinimapModel,
    workspace_id: WorkspaceId,
    workspace_position: [f32; 2],
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
) -> Rect {
    let (workspace_min, workspace_max) =
        workspace_minimap_bounds(workspace_id, workspace_bounds).unwrap_or_else(|| {
            (
                workspace_position,
                [
                    workspace_position[0] + WS_EMPTY_SIZE[0],
                    workspace_position[1] + WS_EMPTY_SIZE[1],
                ],
            )
        });
    Rect::from_min_max(
        origin + minimap_point(model, workspace_min[0], workspace_min[1]).to_vec2(),
        origin + minimap_point(model, workspace_max[0], workspace_max[1]).to_vec2(),
    )
}

fn panel_minimap_screen_rect(origin: Pos2, model: &MinimapModel, position: [f32; 2], size: [f32; 2]) -> Rect {
    Rect::from_min_max(
        origin + minimap_point(model, position[0], position[1]).to_vec2(),
        origin + minimap_point(model, position[0] + size[0], position[1] + size[1]).to_vec2(),
    )
}

fn paint_minimap_viewport(painter: &Painter, origin: Pos2, model: &MinimapModel) {
    let map_rect = Rect::from_min_max(
        origin + Vec2::splat(MINIMAP_PAD),
        origin + (model.outer_size - Vec2::splat(MINIMAP_PAD)),
    );
    let viewport_rect = Rect::from_min_max(
        origin + minimap_point(model, model.view_min.x, model.view_min.y).to_vec2(),
        origin + minimap_point(model, model.view_max.x, model.view_max.y).to_vec2(),
    )
    .intersect(map_rect);
    if !viewport_rect.is_positive() {
        return;
    }
    painter.rect_filled(viewport_rect, CornerRadius::same(1), theme::alpha(theme::FG(), 14));
    painter.rect_stroke(
        viewport_rect,
        CornerRadius::same(1),
        Stroke::new(1.0_f32, theme::alpha(theme::FG(), 90)),
        StrokeKind::Inside,
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use egui::{Pos2, Rect, Vec2};
    use horizon_core::WorkspaceId;

    use super::{
        MinimapModel, WS_EMPTY_SIZE, panel_minimap_screen_rect, workspace_minimap_screen_rect, workspace_style_alpha,
    };

    fn test_model() -> MinimapModel {
        MinimapModel {
            content_min: [100.0, 200.0],
            scale_x: 0.5,
            scale_y: 0.25,
            outer_size: Vec2::ZERO,
            view_min: Pos2::ZERO,
            view_max: Pos2::ZERO,
        }
    }

    #[test]
    fn active_workspace_brightens_while_hovered() {
        let active = workspace_style_alpha(true, false);
        let active_hovered = workspace_style_alpha(true, true);

        assert!(active_hovered.0 > active.0);
        assert!(active_hovered.1 > active.1);
    }

    #[test]
    fn panel_minimap_screen_rect_applies_model_scale_and_pad() {
        let rect = panel_minimap_screen_rect(Pos2::new(10.0, 20.0), &test_model(), [140.0, 240.0], [20.0, 40.0]);

        assert_eq!(rect, Rect::from_min_max(Pos2::new(36.0, 36.0), Pos2::new(46.0, 46.0)));
    }

    #[test]
    fn workspace_minimap_screen_rect_falls_back_to_empty_size() {
        let rect = workspace_minimap_screen_rect(
            Pos2::new(10.0, 20.0),
            &test_model(),
            WorkspaceId(7),
            [100.0, 200.0],
            &HashMap::new(),
        );

        assert_eq!(
            rect,
            Rect::from_min_max(
                Pos2::new(16.0, 26.0),
                Pos2::new(16.0 + WS_EMPTY_SIZE[0] * 0.5, 26.0 + WS_EMPTY_SIZE[1] * 0.25)
            )
        );
    }
}
