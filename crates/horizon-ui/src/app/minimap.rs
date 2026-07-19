use std::{collections::HashMap, time::SystemTime};

use egui::{Context, CornerRadius, Id, Order, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use horizon_core::WorkspaceId;

use crate::theme;

use super::{HorizonApp, MINIMAP_MARGIN, MINIMAP_PAD, WS_BG_PAD, WS_EMPTY_SIZE, WS_TITLE_HEIGHT};

mod labels;
mod spotlight;

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

struct MinimapPaintGeometry<'a> {
    rect: Rect,
    model: &'a MinimapModel,
    workspace_bounds: &'a HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    scope: MinimapScope,
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
    let spotlight = spotlight::MinimapSpotlight::collect(app, scope, SystemTime::now());

    let response = egui::Area::new(overlay_id)
        .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-MINIMAP_MARGIN, -MINIMAP_MARGIN))
        .order(Order::Foreground)
        .show(ctx, |ui| {
            let (response, painter) = ui.allocate_painter(model.outer_size, Sense::click_and_drag());
            let geometry = MinimapPaintGeometry {
                rect: response.rect,
                model: &model,
                workspace_bounds,
                scope,
            };
            let paint_result = paint_minimap_contents(app, ui, &painter, &geometry, &spotlight);
            (response, paint_result)
        });

    let (inner, paint_result) = response.inner;
    if let Some(navigation) = paint_result.navigation {
        apply_minimap_navigation(app, navigation, canvas_rect);
    } else if (inner.clicked() || inner.dragged())
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

fn apply_minimap_navigation(app: &mut HorizonApp, navigation: spotlight::MinimapNavigationTarget, canvas_rect: Rect) {
    match navigation {
        spotlight::MinimapNavigationTarget::Panel { workspace_id, panel_id } => {
            if app
                .board
                .panel(panel_id)
                .is_some_and(|panel| panel.workspace_id == workspace_id)
            {
                app.focus_panel_in_rect(panel_id, canvas_rect);
            } else {
                let _ = app.focus_workspace_in_rect(workspace_id, canvas_rect);
            }
        }
        spotlight::MinimapNavigationTarget::Workspace(workspace_id) => {
            let _ = app.focus_workspace_in_rect(workspace_id, canvas_rect);
        }
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
    ui: &mut Ui,
    painter: &Painter,
    geometry: &MinimapPaintGeometry<'_>,
    spotlight: &spotlight::MinimapSpotlight,
) -> spotlight::MinimapPaintResult {
    painter.rect_filled(
        geometry.rect,
        CornerRadius::same(8),
        theme::alpha(theme::BG_ELEVATED(), 220),
    );
    painter.rect_stroke(
        geometry.rect,
        CornerRadius::same(8),
        Stroke::new(1.0_f32, theme::alpha(theme::BORDER_SUBTLE(), 180)),
        StrokeKind::Outside,
    );

    spotlight::paint_minimap_scene(app, painter, geometry);
    let mut cue_layout = spotlight::MinimapCueLayout::collect_active_tabs(app, geometry);
    let label_rects = labels::paint_minimap_workspace_labels(
        app,
        painter,
        geometry.rect.min,
        geometry.model,
        geometry.workspace_bounds,
        geometry.scope,
        cue_layout.exclusions(),
    );
    cue_layout.place_attention(app, geometry, spotlight, &label_rects);
    paint_minimap_viewport(painter, geometry.rect.min, geometry.model);
    spotlight::paint_minimap_cues(app, ui, painter, spotlight, &cue_layout)
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

fn workspace_minimap_rect(
    workspace_id: WorkspaceId,
    workspace_position: [f32; 2],
    origin: Pos2,
    model: &MinimapModel,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
) -> Rect {
    let (workspace_min, workspace_max) = workspace_minimap_bounds(workspace_id, workspace_bounds).unwrap_or((
        workspace_position,
        [
            workspace_position[0] + WS_EMPTY_SIZE[0],
            workspace_position[1] + WS_EMPTY_SIZE[1],
        ],
    ));
    Rect::from_min_max(
        origin + minimap_point(model, workspace_min[0], workspace_min[1]).to_vec2(),
        origin + minimap_point(model, workspace_max[0], workspace_max[1]).to_vec2(),
    )
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
    painter.rect_filled(viewport_rect, CornerRadius::same(1), theme::alpha(theme::FG(), 14));
    painter.rect_stroke(
        viewport_rect,
        CornerRadius::same(1),
        Stroke::new(1.0_f32, theme::alpha(theme::FG(), 90)),
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
    scope_includes_workspace_state(scope, workspace_id, app.workspace_is_detached(workspace_id))
}

fn scope_includes_workspace_state(scope: MinimapScope, workspace_id: WorkspaceId, is_detached: bool) -> bool {
    match scope {
        MinimapScope::Attached => !is_detached,
        MinimapScope::Workspace(target_workspace_id) => target_workspace_id == workspace_id,
    }
}
