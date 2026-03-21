use std::collections::HashMap;

use egui::{Color32, Context, CornerRadius, Id, Order, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use horizon_core::WorkspaceId;

use crate::theme;

use super::{HorizonApp, MINIMAP_MARGIN, MINIMAP_PAD, WS_BG_PAD, WS_EMPTY_SIZE, WS_TITLE_HEIGHT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MinimapScope {
    Attached,
    Workspace(WorkspaceId),
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

fn render_scoped_minimap(
    app: &mut HorizonApp,
    ctx: &Context,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    canvas_rect: Rect,
    scope: MinimapScope,
    overlay_id: Id,
) -> f32 {
    if !app.fixed_overlays_visible() || !app.minimap_visible || !scope_has_content(app, scope) {
        return 0.0;
    }

    let Some(model) = minimap_model(app, canvas_rect, workspace_bounds, scope) else {
        return 0.0;
    };
    let minimap_height = model.outer_size.y;

    let response = egui::Area::new(overlay_id)
        .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-MINIMAP_MARGIN, -MINIMAP_MARGIN))
        .order(Order::Foreground)
        .show(ctx, |ui| {
            let (response, painter) = ui.allocate_painter(model.outer_size, Sense::click_and_drag());
            paint_minimap_contents(app, &painter, response.rect, &model, workspace_bounds, scope);
            response
        });

    let inner = response.inner;
    if (inner.clicked() || inner.dragged())
        && let Some(pointer) = ctx.input(|input| input.pointer.interact_pos())
    {
        let local = pointer - inner.rect.min;
        let canvas_x = model.content_min[0] + (local.x - MINIMAP_PAD) / model.scale_x;
        let canvas_y = model.content_min[1] + (local.y - MINIMAP_PAD) / model.scale_y;

        app.pan_target = None;
        app.canvas_view.align_canvas_point_to_screen(
            [canvas_rect.min.x, canvas_rect.min.y],
            [canvas_x, canvas_y],
            [canvas_rect.center().x, canvas_rect.center().y],
        );
        app.mark_runtime_dirty();
    }

    minimap_height
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
) {
    painter.rect_filled(rect, CornerRadius::same(8), theme::alpha(theme::BG_ELEVATED, 220));
    painter.rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 180)),
        StrokeKind::Outside,
    );

    let origin = rect.min;
    paint_minimap_workspaces(app, painter, origin, model, workspace_bounds, scope);
    paint_minimap_panels(app, painter, origin, model, scope);
    paint_minimap_viewport(painter, origin, model);
}

fn paint_minimap_workspaces(
    app: &HorizonApp,
    painter: &Painter,
    origin: Pos2,
    model: &MinimapModel,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    scope: MinimapScope,
) {
    for workspace in &app.board.workspaces {
        if !scope_includes_workspace(app, scope, workspace.id) {
            continue;
        }
        let (r, g, b) = workspace.accent();
        let workspace_color = Color32::from_rgb(r, g, b);
        let is_active =
            app.board.active_workspace == Some(workspace.id) || scope == MinimapScope::Workspace(workspace.id);
        let (workspace_min, workspace_max) =
            workspace_minimap_bounds(workspace.id, workspace_bounds).unwrap_or_else(|| {
                let pos = workspace.position;
                (pos, [pos[0] + WS_EMPTY_SIZE[0], pos[1] + WS_EMPTY_SIZE[1]])
            });
        let workspace_rect = Rect::from_min_max(
            origin + minimap_point(model, workspace_min[0], workspace_min[1]).to_vec2(),
            origin + minimap_point(model, workspace_max[0], workspace_max[1]).to_vec2(),
        );

        painter.rect_filled(
            workspace_rect,
            CornerRadius::same(2),
            theme::alpha(workspace_color, if is_active { 40 } else { 22 }),
        );
        painter.rect_stroke(
            workspace_rect,
            CornerRadius::same(2),
            Stroke::new(0.8, theme::alpha(workspace_color, if is_active { 140 } else { 80 })),
            StrokeKind::Outside,
        );
    }
}

fn paint_minimap_panels(app: &HorizonApp, painter: &Painter, origin: Pos2, model: &MinimapModel, scope: MinimapScope) {
    for panel in &app.board.panels {
        if !scope_includes_workspace(app, scope, panel.workspace_id) {
            continue;
        }
        let pos = panel.layout.position;
        let size = panel.layout.size;
        let panel_rect = Rect::from_min_max(
            origin + minimap_point(model, pos[0], pos[1]).to_vec2(),
            origin + minimap_point(model, pos[0] + size[0], pos[1] + size[1]).to_vec2(),
        );
        let workspace_color = app
            .board
            .workspace(panel.workspace_id)
            .map_or(theme::ACCENT, |workspace| {
                let (r, g, b) = workspace.accent();
                Color32::from_rgb(r, g, b)
            });

        painter.rect_filled(
            panel_rect,
            CornerRadius::same(1),
            theme::alpha(
                workspace_color,
                if app.board.focused == Some(panel.id) { 120 } else { 70 },
            ),
        );
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
    painter.rect_filled(viewport_rect, CornerRadius::same(1), theme::alpha(theme::FG, 14));
    painter.rect_stroke(
        viewport_rect,
        CornerRadius::same(1),
        Stroke::new(1.0, theme::alpha(theme::FG, 90)),
        StrokeKind::Inside,
    );
}

fn scope_has_content(app: &HorizonApp, scope: MinimapScope) -> bool {
    match scope {
        MinimapScope::Attached => app
            .board
            .workspaces
            .iter()
            .any(|workspace| !app.workspace_is_detached(workspace.id)),
        MinimapScope::Workspace(workspace_id) => app.board.workspace(workspace_id).is_some(),
    }
}

fn scope_includes_workspace(app: &HorizonApp, scope: MinimapScope, workspace_id: WorkspaceId) -> bool {
    match scope {
        MinimapScope::Attached => !app.workspace_is_detached(workspace_id),
        MinimapScope::Workspace(target_workspace_id) => target_workspace_id == workspace_id,
    }
}
