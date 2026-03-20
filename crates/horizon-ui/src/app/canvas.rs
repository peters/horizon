use std::collections::HashMap;

use egui::{
    Align, Color32, Context, CornerRadius, Id, Layout, Margin, Mesh, Order, Painter, Pos2, Rect, Sense, Shape, Stroke,
    StrokeKind, UiBuilder, Vec2,
};
use horizon_core::WorkspaceId;

use crate::theme;

use super::util::{format_grid_position, paint_canvas_glow, rounded_i32};
use super::{HorizonApp, MINIMAP_MARGIN, MINIMAP_PAD, SIDEBAR_WIDTH, WS_BG_PAD, WS_EMPTY_SIZE, WS_TITLE_HEIGHT};

const GRID_SPACING: f32 = 22.0;
const GRID_DOT_DIAMETER: f32 = 2.3;
const MIN_GRID_SCREEN_SPACING: f32 = 14.0;

struct MinimapModel {
    content_min: [f32; 2],
    scale_x: f32,
    scale_y: f32,
    outer_size: Vec2,
    view_min: Pos2,
    view_max: Pos2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CanvasGridCacheKey {
    rect_min_x: u32,
    rect_min_y: u32,
    rect_width: u32,
    rect_height: u32,
    spacing: u32,
    dot_diameter: u32,
    offset_x: u32,
    offset_y: u32,
}

impl CanvasGridCacheKey {
    fn new(rect: Rect, canvas_view: horizon_core::CanvasViewState) -> Self {
        let layout = dot_grid_layout(canvas_view);
        Self {
            rect_min_x: rect.min.x.to_bits(),
            rect_min_y: rect.min.y.to_bits(),
            rect_width: rect.width().to_bits(),
            rect_height: rect.height().to_bits(),
            spacing: layout.spacing.to_bits(),
            dot_diameter: layout.dot_diameter.to_bits(),
            offset_x: canvas_view.pan_offset[0].rem_euclid(layout.spacing).to_bits(),
            offset_y: canvas_view.pan_offset[1].rem_euclid(layout.spacing).to_bits(),
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct CanvasGridCache {
    key: Option<CanvasGridCacheKey>,
    shape: Option<Shape>,
}

impl HorizonApp {
    pub(super) fn render_minimap(
        &mut self,
        ctx: &Context,
        workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    ) -> f32 {
        if !self.fixed_overlays_visible() || !self.minimap_visible || self.board.workspaces.is_empty() {
            return 0.0;
        }

        let canvas_rect = self.canvas_rect(ctx);
        let Some(model) = self.minimap_model(canvas_rect, workspace_bounds) else {
            return 0.0;
        };
        let minimap_height = model.outer_size.y;

        let response = egui::Area::new(Id::new("minimap_overlay"))
            .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-MINIMAP_MARGIN, -MINIMAP_MARGIN))
            .order(Order::Foreground)
            .show(ctx, |ui| {
                let (response, painter) = ui.allocate_painter(model.outer_size, Sense::click_and_drag());
                self.paint_minimap_contents(&painter, response.rect, &model, workspace_bounds);
                response
            });

        let inner = response.inner;
        if (inner.clicked() || inner.dragged())
            && let Some(pointer) = ctx.input(|input| input.pointer.interact_pos())
        {
            let local = pointer - inner.rect.min;
            let canvas_x = model.content_min[0] + (local.x - MINIMAP_PAD) / model.scale_x;
            let canvas_y = model.content_min[1] + (local.y - MINIMAP_PAD) / model.scale_y;

            self.pan_target = None;
            self.canvas_view.align_canvas_point_to_screen(
                [canvas_rect.min.x, canvas_rect.min.y],
                [canvas_x, canvas_y],
                [canvas_rect.center().x, canvas_rect.center().y],
            );
            self.mark_runtime_dirty();
        }

        minimap_height
    }

    fn minimap_model(
        &self,
        canvas_rect: Rect,
        workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    ) -> Option<MinimapModel> {
        let (content_min, content_max) = workspace_content_bounds(self, workspace_bounds)?;
        let view_min = self.screen_to_canvas(canvas_rect, canvas_rect.min);
        let view_max = self.screen_to_canvas(canvas_rect, canvas_rect.max);

        let content_w = content_max[0] - content_min[0];
        let content_h = content_max[1] - content_min[1];
        if content_w < 1.0 || content_h < 1.0 {
            return None;
        }

        let overlays = &self.template_config.overlays;
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
        &self,
        painter: &Painter,
        rect: Rect,
        model: &MinimapModel,
        workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    ) {
        painter.rect_filled(rect, CornerRadius::same(8), theme::alpha(theme::BG_ELEVATED, 220));
        painter.rect_stroke(
            rect,
            CornerRadius::same(8),
            Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 180)),
            StrokeKind::Outside,
        );

        let origin = rect.min;
        self.paint_minimap_workspaces(painter, origin, model, workspace_bounds);
        self.paint_minimap_panels(painter, origin, model);
        paint_minimap_viewport(painter, origin, model);
    }

    fn paint_minimap_workspaces(
        &self,
        painter: &Painter,
        origin: Pos2,
        model: &MinimapModel,
        workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    ) {
        for workspace in &self.board.workspaces {
            if self.workspace_is_detached(workspace.id) {
                continue;
            }
            let (r, g, b) = workspace.accent();
            let workspace_color = Color32::from_rgb(r, g, b);
            let is_active = self.board.active_workspace == Some(workspace.id);
            let (workspace_min, workspace_max) = workspace_minimap_bounds(workspace.id, workspace_bounds)
                .unwrap_or_else(|| {
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
            if self.workspace_is_detached(panel.workspace_id) {
                continue;
            }
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

        let canvas_rect = self.canvas_rect(ctx);
        let view_origin = self.screen_to_canvas(canvas_rect, canvas_rect.min);
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
                            egui::RichText::new(format!("zoom  {:.0}%", self.canvas_view.zoom * 100.0))
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

    #[profiling::function]
    pub(super) fn render_canvas(&mut self, ctx: &Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::BG))
            .show(ctx, |ui| {
                paint_canvas_glow(ui);
                paint_dot_grid(ui, self.canvas_view, &mut self.canvas_grid_cache);
                if self.board.panels.is_empty() {
                    self.render_empty_state_card(ui, ctx);
                }
            });
    }

    fn render_empty_state_card(&mut self, ui: &mut egui::Ui, ctx: &Context) {
        let quick_nav_shortcut = self
            .shortcuts
            .command_palette
            .display_label(super::util::primary_shortcut_label());
        let fit_shortcut = self
            .shortcuts
            .fit_active_workspace
            .display_label(super::util::primary_shortcut_label());
        let has_attached_workspace = self
            .board
            .workspaces
            .iter()
            .any(|workspace| !self.workspace_is_detached(workspace.id));
        let card_rect = Rect::from_center_size(ui.max_rect().center(), Vec2::new(540.0, 228.0));

        ui.scope_builder(
            UiBuilder::new()
                .max_rect(card_rect)
                .layout(Layout::top_down(Align::Center)),
            |ui| {
                egui::Frame::new()
                    .fill(theme::alpha(theme::PANEL_BG, 238))
                    .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 210)))
                    .corner_radius(20)
                    .inner_margin(Margin::same(20))
                    .show(ui, |ui| {
                        ui.set_min_size(card_rect.size());
                        ui.vertical_centered(|ui| {
                            ui.label(
                                egui::RichText::new("Start with a workspace-first flow")
                                    .color(theme::FG)
                                    .size(18.0)
                                    .strong(),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new(
                                    "Create a workspace, launch a preset-driven terminal, jump with Quick Nav, then fit the active workspace when you want a clean overview.",
                                )
                                .color(theme::FG_SOFT)
                                .size(12.0),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new(
                                    "Rows / Cols / Grid live on each workspace header once panels are open, so you can stay structured without giving up the canvas.",
                                )
                                .color(theme::FG_DIM)
                                .size(11.0),
                            );
                            ui.add_space(18.0);
                            ui.horizontal_wrapped(|ui| {
                                if ui.add(super::util::primary_button("New Workspace")).clicked() {
                                    let name = format!("Workspace {}", self.board.workspaces.len() + 1);
                                    self.create_workspace_visible(ctx, &name);
                                }
                                if ui.add(super::util::chrome_button("New Terminal")).clicked() {
                                    if let Some(preset) = self.presets.first().cloned() {
                                        let workspace_id = self.ensure_workspace_visible(ctx);
                                        self.add_panel_to_workspace(workspace_id, preset, None);
                                    } else {
                                        self.create_panel(ctx);
                                    }
                                }
                                if ui.add(super::util::chrome_button("Quick Nav")).clicked() {
                                    self.open_command_palette();
                                }
                                if ui
                                    .add_enabled(has_attached_workspace, super::util::chrome_button("Fit Workspace"))
                                    .clicked()
                                {
                                    let _ = self.fit_active_workspace(ctx);
                                }
                            });
                            ui.add_space(14.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "Quick Nav: {quick_nav_shortcut}    Fit Workspace: {fit_shortcut}"
                                ))
                                .monospace()
                                .color(theme::FG_SOFT)
                                .size(11.0),
                            );
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(
                                    "You can still Ctrl+double-click to create a workspace and Ctrl+double-click inside one to add a terminal.",
                                )
                                .color(theme::FG_DIM)
                                .size(10.5),
                            );
                        });
                    });
            },
        );
    }
}

fn workspace_content_bounds(
    app: &HorizonApp,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
) -> Option<([f32; 2], [f32; 2])> {
    let mut content_min = [f32::MAX, f32::MAX];
    let mut content_max = [f32::MIN, f32::MIN];
    let mut has_content = false;

    for workspace in &app.board.workspaces {
        if app.workspace_is_detached(workspace.id) {
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

fn paint_dot_grid(ui: &mut egui::Ui, canvas_view: horizon_core::CanvasViewState, cache: &mut CanvasGridCache) {
    let rect = ui.max_rect();
    let key = CanvasGridCacheKey::new(rect, canvas_view);

    if cache.key != Some(key) {
        cache.key = Some(key);
        cache.shape = Some(build_dot_grid_shape(rect, canvas_view));
    }

    if let Some(shape) = &cache.shape {
        ui.painter().add(shape.clone());
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct DotGridLayout {
    spacing: f32,
    dot_diameter: f32,
}

fn dot_grid_layout(canvas_view: horizon_core::CanvasViewState) -> DotGridLayout {
    let mut spacing = GRID_SPACING * canvas_view.zoom;
    while spacing < MIN_GRID_SCREEN_SPACING {
        spacing *= 2.0;
    }

    DotGridLayout {
        spacing,
        dot_diameter: (GRID_DOT_DIAMETER * canvas_view.zoom).clamp(1.0, 5.0),
    }
}

fn build_dot_grid_shape(rect: Rect, canvas_view: horizon_core::CanvasViewState) -> Shape {
    let layout = dot_grid_layout(canvas_view);
    let offset_x = canvas_view.pan_offset[0].rem_euclid(layout.spacing);
    let offset_y = canvas_view.pan_offset[1].rem_euclid(layout.spacing);
    let columns = dot_grid_axis_count(rect.width(), layout.spacing);
    let rows = dot_grid_axis_count(rect.height(), layout.spacing);
    let dot_count = columns.saturating_mul(rows);

    let mut mesh = Mesh::default();
    mesh.reserve_vertices(dot_count.saturating_mul(4));
    mesh.reserve_triangles(dot_count.saturating_mul(2));

    let mut x = rect.min.x + offset_x;
    while x <= rect.max.x {
        let mut y = rect.min.y + offset_y;
        while y <= rect.max.y {
            let dot_rect = Rect::from_center_size(Pos2::new(x, y), Vec2::splat(layout.dot_diameter));
            mesh.add_colored_rect(dot_rect, theme::GRID_DOT);
            y += layout.spacing;
        }
        x += layout.spacing;
    }

    Shape::mesh(mesh)
}

fn dot_grid_axis_count(length: f32, spacing: f32) -> usize {
    if !length.is_finite() || length < 0.0 || !spacing.is_finite() || spacing <= 0.0 {
        return 1;
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = ((length + spacing) / spacing).ceil() as usize;
    steps.saturating_add(1)
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

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect, Vec2};
    use horizon_core::CanvasViewState;

    use super::{CanvasGridCacheKey, GRID_SPACING, MIN_GRID_SCREEN_SPACING, dot_grid_axis_count, dot_grid_layout};

    #[test]
    fn dot_grid_axis_count_handles_invalid_spacing() {
        assert_eq!(dot_grid_axis_count(640.0, 0.0), 1);
        assert_eq!(dot_grid_axis_count(640.0, f32::NAN), 1);
        assert_eq!(dot_grid_axis_count(f32::NAN, GRID_SPACING), 1);
    }

    #[test]
    fn dot_grid_axis_count_grows_when_zooming_out() {
        let zoomed_in = dot_grid_axis_count(880.0, GRID_SPACING * 2.0);
        let zoomed_out = dot_grid_axis_count(880.0, GRID_SPACING * 0.5);

        assert!(zoomed_out > zoomed_in);
    }

    #[test]
    fn dot_grid_layout_bounds_zoomed_out_spacing() {
        let layout = dot_grid_layout(CanvasViewState::new([0.0, 0.0], 0.25));

        assert!(layout.spacing >= MIN_GRID_SCREEN_SPACING);
        assert!((layout.spacing - GRID_SPACING * 0.25 * 4.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn dot_grid_layout_preserves_base_spacing_when_already_sparse_enough() {
        let layout = dot_grid_layout(CanvasViewState::new([0.0, 0.0], 1.0));

        assert!((layout.spacing - GRID_SPACING).abs() <= f32::EPSILON);
    }

    #[test]
    fn dot_grid_layout_caps_zoomed_out_density() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0));
        let layout = dot_grid_layout(CanvasViewState::new([0.0, 0.0], 0.25));
        let dot_count =
            dot_grid_axis_count(rect.width(), layout.spacing) * dot_grid_axis_count(rect.height(), layout.spacing);

        assert!(dot_count < 10_000);
    }

    #[test]
    fn canvas_grid_cache_key_tracks_zoom_changes() {
        let rect = Rect::from_min_size(Pos2::new(210.0, 46.0), Vec2::new(1200.0, 800.0));
        let base = CanvasGridCacheKey::new(rect, CanvasViewState::new([24.0, -12.0], 1.0));
        let zoomed = CanvasGridCacheKey::new(rect, CanvasViewState::new([24.0, -12.0], 1.5));

        assert_ne!(base, zoomed);
    }
}
