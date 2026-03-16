use egui::{Color32, Context, CornerRadius, Id, Margin, Order, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::theme;

use super::util::{draw_dot_grid, format_grid_position, paint_canvas_glow, paint_empty_state, rounded_i32};
use super::{
    HorizonApp, MINIMAP_MARGIN, MINIMAP_MAX_H, MINIMAP_MAX_W, MINIMAP_PAD, SIDEBAR_WIDTH, WS_BG_PAD, WS_EMPTY_SIZE,
    WS_TITLE_HEIGHT,
};

struct MinimapModel {
    content_min: [f32; 2],
    scale: f32,
    outer_size: Vec2,
    view_min: Pos2,
    view_max: Pos2,
}

impl HorizonApp {
    pub(super) fn render_minimap(&mut self, ctx: &Context) {
        if !self.minimap_visible || self.board.workspaces.is_empty() {
            return;
        }

        let canvas_rect = Self::canvas_rect(ctx, self.sidebar_visible);
        let Some(model) = self.minimap_model(canvas_rect) else {
            return;
        };

        let response = egui::Area::new(Id::new("minimap_overlay"))
            .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-MINIMAP_MARGIN, -MINIMAP_MARGIN))
            .order(Order::Foreground)
            .show(ctx, |ui| {
                let (response, painter) = ui.allocate_painter(model.outer_size, Sense::click_and_drag());
                self.paint_minimap_contents(&painter, response.rect, &model);
                response
            });

        let inner = response.inner;
        if (inner.clicked() || inner.dragged())
            && let Some(pointer) = ctx.input(|input| input.pointer.interact_pos())
        {
            let local = pointer - inner.rect.min;
            let canvas_x = model.content_min[0] + (local.x - MINIMAP_PAD) / model.scale;
            let canvas_y = model.content_min[1] + (local.y - MINIMAP_PAD) / model.scale;

            self.pan_target = None;
            self.pan_offset = Vec2::new(
                canvas_rect.width() * 0.5 - canvas_x,
                canvas_rect.height() * 0.5 - canvas_y,
            );
            self.mark_runtime_dirty();
        }
    }

    fn minimap_model(&self, canvas_rect: Rect) -> Option<MinimapModel> {
        let (mut content_min, mut content_max) = workspace_content_bounds(self)?;
        let view_min = self.screen_to_canvas(canvas_rect, canvas_rect.min);
        let view_max = self.screen_to_canvas(canvas_rect, canvas_rect.max);
        let view_w = view_max.x - view_min.x;
        let view_h = view_max.y - view_min.y;
        content_min[0] -= view_w * 0.5;
        content_min[1] -= view_h * 0.5;
        content_max[0] += view_w * 0.5;
        content_max[1] += view_h * 0.5;

        let content_w = content_max[0] - content_min[0];
        let content_h = content_max[1] - content_min[1];
        if content_w < 1.0 || content_h < 1.0 {
            return None;
        }

        let aspect = content_w / content_h;
        let (map_w, map_h) = if aspect > MINIMAP_MAX_W / MINIMAP_MAX_H {
            (MINIMAP_MAX_W, MINIMAP_MAX_W / aspect)
        } else {
            (MINIMAP_MAX_H * aspect, MINIMAP_MAX_H)
        };

        Some(MinimapModel {
            content_min,
            scale: map_w / content_w,
            outer_size: Vec2::new(map_w + MINIMAP_PAD * 2.0, map_h + MINIMAP_PAD * 2.0),
            view_min,
            view_max,
        })
    }

    fn paint_minimap_contents(&self, painter: &Painter, rect: Rect, model: &MinimapModel) {
        painter.rect_filled(rect, CornerRadius::same(8), theme::alpha(theme::BG_ELEVATED, 220));
        painter.rect_stroke(
            rect,
            CornerRadius::same(8),
            Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 180)),
            StrokeKind::Outside,
        );

        let origin = rect.min;
        self.paint_minimap_workspaces(painter, origin, model);
        self.paint_minimap_panels(painter, origin, model);
        paint_minimap_viewport(painter, origin, model);
    }

    fn paint_minimap_workspaces(&self, painter: &Painter, origin: Pos2, model: &MinimapModel) {
        for workspace in &self.board.workspaces {
            let (r, g, b) = workspace.accent();
            let workspace_color = Color32::from_rgb(r, g, b);
            let is_active = self.board.active_workspace == Some(workspace.id);
            let (workspace_min, workspace_max) = workspace_minimap_bounds(self, workspace.id).unwrap_or_else(|| {
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

    fn paint_minimap_panels(&self, painter: &Painter, origin: Pos2, model: &MinimapModel) {
        for panel in &self.board.panels {
            let pos = panel.layout.position;
            let size = panel.layout.size;
            let panel_rect = Rect::from_min_max(
                origin + minimap_point(model, pos[0], pos[1]).to_vec2(),
                origin + minimap_point(model, pos[0] + size[0], pos[1] + size[1]).to_vec2(),
            );
            let workspace_color = self
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
                    if self.board.focused == Some(panel.id) { 120 } else { 70 },
                ),
            );
        }
    }

    pub(super) fn render_canvas_hud(&self, ctx: &Context) {
        if !self.hud_visible {
            return;
        }

        let view_origin = Pos2::new(-self.pan_offset.x, -self.pan_offset.y);
        let focused_status = self
            .board
            .focused
            .and_then(|panel_id| self.board.panel(panel_id))
            .map_or_else(
                || "none".to_string(),
                |panel| {
                    format!(
                        "{}  {} x {}",
                        format_grid_position(Pos2::new(panel.layout.position[0], panel.layout.position[1])),
                        rounded_i32(panel.layout.size[0]),
                        rounded_i32(panel.layout.size[1]),
                    )
                },
            );

        let hud_left = if self.sidebar_visible {
            SIDEBAR_WIDTH + 16.0
        } else {
            16.0
        };
        egui::Area::new(Id::new("canvas_hud"))
            .anchor(egui::Align2::LEFT_BOTTOM, Vec2::new(hud_left, -16.0))
            .interactable(false)
            .order(Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(theme::alpha(theme::PANEL_BG, 236))
                    .inner_margin(Margin::symmetric(12, 10))
                    .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_STRONG, 210)))
                    .corner_radius(12)
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Canvas HUD").color(theme::FG).size(11.5).strong());
                        ui.label(
                            egui::RichText::new(format!("view origin  {}", format_grid_position(view_origin)))
                                .monospace()
                                .color(theme::FG_SOFT)
                                .size(11.0),
                        );
                        ui.label(
                            egui::RichText::new(format!("focused term {focused_status}"))
                                .monospace()
                                .color(theme::FG_SOFT)
                                .size(11.0),
                        );
                    });
            });
    }

    pub(super) fn render_canvas(&self, ctx: &Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::BG))
            .show(ctx, |ui| {
                paint_canvas_glow(ui);
                draw_dot_grid(ui, self.pan_offset);
                if self.board.panels.is_empty() {
                    paint_empty_state(ui);
                }
            });
    }
}

fn workspace_content_bounds(app: &HorizonApp) -> Option<([f32; 2], [f32; 2])> {
    let mut content_min = [f32::MAX, f32::MAX];
    let mut content_max = [f32::MIN, f32::MIN];
    let mut has_content = false;

    for workspace in &app.board.workspaces {
        let (workspace_min, workspace_max) = workspace_minimap_bounds(app, workspace.id).unwrap_or_else(|| {
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

fn workspace_minimap_bounds(app: &HorizonApp, workspace_id: horizon_core::WorkspaceId) -> Option<([f32; 2], [f32; 2])> {
    app.board
        .workspace_bounds(workspace_id)
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
        MINIMAP_PAD + (canvas_x - model.content_min[0]) * model.scale,
        MINIMAP_PAD + (canvas_y - model.content_min[1]) * model.scale,
    )
}

fn paint_minimap_viewport(painter: &Painter, origin: Pos2, model: &MinimapModel) {
    let viewport_rect = Rect::from_min_max(
        origin + minimap_point(model, model.view_min.x, model.view_min.y).to_vec2(),
        origin + minimap_point(model, model.view_max.x, model.view_max.y).to_vec2(),
    );
    painter.rect_filled(viewport_rect, CornerRadius::same(1), theme::alpha(theme::FG, 14));
    painter.rect_stroke(
        viewport_rect,
        CornerRadius::same(1),
        Stroke::new(1.0, theme::alpha(theme::FG, 90)),
        StrokeKind::Inside,
    );
}
