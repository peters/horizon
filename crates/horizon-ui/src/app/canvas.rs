use egui::{Align, Context, Id, Layout, Margin, Mesh, Order, Pos2, Rect, Shape, Stroke, Vec2};

use crate::theme;

use super::HorizonApp;
use super::root_chrome::effective_sidebar_width;
use super::util::{format_grid_position, paint_canvas_glow, rounded_i32, viewport_local_rect};

const GRID_SPACING: f32 = 22.0;
const GRID_DOT_DIAMETER: f32 = 2.3;
const MIN_GRID_SCREEN_SPACING: f32 = 14.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CanvasGridCacheKey {
    rect_min_x: u32,
    rect_min_y: u32,
    rect_width: u32,
    rect_height: u32,
    spacing: u32,
    dot_diameter: u32,
}

impl CanvasGridCacheKey {
    fn new(rect: Rect, layout: DotGridLayout) -> Self {
        Self {
            rect_min_x: rect.min.x.to_bits(),
            rect_min_y: rect.min.y.to_bits(),
            rect_width: rect.width().to_bits(),
            rect_height: rect.height().to_bits(),
            spacing: layout.spacing.to_bits(),
            dot_diameter: layout.dot_diameter.to_bits(),
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct CanvasGridCache {
    key: Option<CanvasGridCacheKey>,
    shape: Option<Shape>,
    offset: Vec2,
}

impl HorizonApp {
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
            effective_sidebar_width(viewport_local_rect(ctx).width()) + 16.0
        } else {
            16.0
        };
        egui::Area::new(Id::new("canvas_hud"))
            .anchor(egui::Align2::LEFT_BOTTOM, Vec2::new(hud_left, -16.0))
            .interactable(false)
            .order(Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(theme::alpha(theme::PANEL_BG(), 236))
                    .inner_margin(Margin::symmetric(12, 10))
                    .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_STRONG(), 210)))
                    .corner_radius(12)
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Canvas HUD").color(theme::FG()).size(11.5).strong());
                        ui.label(
                            egui::RichText::new(format!("view origin  {}", format_grid_position(view_origin)))
                                .monospace()
                                .color(theme::FG_SOFT())
                                .size(11.0),
                        );
                        ui.label(
                            egui::RichText::new(format!("zoom  {:.0}%", self.canvas_view.zoom * 100.0))
                                .monospace()
                                .color(theme::FG_SOFT())
                                .size(11.0),
                        );
                        ui.label(
                            egui::RichText::new(format!("focused term {focused_status}"))
                                .monospace()
                                .color(theme::FG_SOFT())
                                .size(11.0),
                        );
                    });
            });
    }

    #[profiling::function]
    pub(super) fn render_canvas(&mut self, ctx: &Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::BG()))
            .show(ctx, |ui| {
                paint_canvas_glow(ui);
                paint_dot_grid(ui, self.canvas_view, &mut self.canvas_grid_cache);
            });
    }

    pub(super) fn render_empty_state_card(&mut self, ctx: &Context) {
        if !should_show_empty_state_card(self.board.workspaces.len(), self.board.panels.len()) {
            return;
        }

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
        let card_size = Vec2::new(540.0, 228.0);
        let card_rect = Rect::from_center_size(self.canvas_rect(ctx).center(), card_size);

        egui::Area::new(Id::new("empty_state_card"))
            .fixed_pos(card_rect.min)
            .order(Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::alpha(theme::PANEL_BG(), 238))
                    .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE(), 210)))
                    .corner_radius(20)
                    .inner_margin(Margin::same(20))
                    .show(ui, |ui| {
                        ui.set_min_size(card_size);
                        ui.set_max_size(card_size);
                        ui.with_layout(Layout::top_down(Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new("Start with a workspace-first flow")
                                    .color(theme::FG())
                                    .size(18.0)
                                    .strong(),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new(
                                    "Create a workspace, launch a preset-driven terminal, jump with Quick Nav, then fit the active workspace when you want a clean overview.",
                                )
                                .color(theme::FG_SOFT())
                                .size(12.0),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new(
                                    "Rows / Cols / Grid live on each workspace header once panels are open, so you can stay structured without giving up the canvas.",
                                )
                                .color(theme::FG_DIM())
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
                                .color(theme::FG_SOFT())
                                .size(11.0),
                            );
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(
                                    "You can still Ctrl+double-click to create a workspace and Ctrl+double-click inside one to add a terminal.",
                                )
                                .color(theme::FG_DIM())
                                .size(10.5),
                            );
                        });
                    });
            });
    }
}

fn should_show_empty_state_card(workspace_count: usize, panel_count: usize) -> bool {
    workspace_count == 0 && panel_count == 0
}

fn paint_dot_grid(ui: &mut egui::Ui, canvas_view: horizon_core::CanvasViewState, cache: &mut CanvasGridCache) {
    let rect = ui.max_rect();
    let layout = dot_grid_layout(canvas_view);
    let key = CanvasGridCacheKey::new(rect, layout);
    let offset = dot_grid_offset(canvas_view, layout);

    if cache.key != Some(key) {
        cache.key = Some(key);
        cache.shape = Some(build_dot_grid_shape(rect, layout));
        cache.offset = Vec2::ZERO;
    }

    if let Some(shape) = &mut cache.shape {
        let delta = offset - cache.offset;
        if delta != Vec2::ZERO {
            shape.translate(delta);
            cache.offset = offset;
        }
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

fn dot_grid_offset(canvas_view: horizon_core::CanvasViewState, layout: DotGridLayout) -> Vec2 {
    Vec2::new(
        canvas_view.pan_offset[0].rem_euclid(layout.spacing),
        canvas_view.pan_offset[1].rem_euclid(layout.spacing),
    )
}

fn build_dot_grid_shape(rect: Rect, layout: DotGridLayout) -> Shape {
    let expanded_rect = rect.expand(layout.spacing);
    let columns = dot_grid_axis_count(expanded_rect.width(), layout.spacing);
    let rows = dot_grid_axis_count(expanded_rect.height(), layout.spacing);
    let dot_count = columns.saturating_mul(rows);

    let mut mesh = Mesh::default();
    mesh.reserve_vertices(dot_count.saturating_mul(4));
    mesh.reserve_triangles(dot_count.saturating_mul(2));

    let mut x = expanded_rect.min.x;
    while x <= expanded_rect.max.x {
        let mut y = expanded_rect.min.y;
        while y <= expanded_rect.max.y {
            let dot_rect = Rect::from_center_size(Pos2::new(x, y), Vec2::splat(layout.dot_diameter));
            mesh.add_colored_rect(dot_rect, theme::GRID_DOT());
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

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect, Vec2};
    use horizon_core::CanvasViewState;

    use super::{
        CanvasGridCacheKey, GRID_SPACING, MIN_GRID_SCREEN_SPACING, dot_grid_axis_count, dot_grid_layout,
        should_show_empty_state_card,
    };

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
        let base = CanvasGridCacheKey::new(rect, dot_grid_layout(CanvasViewState::new([24.0, -12.0], 1.0)));
        let zoomed = CanvasGridCacheKey::new(rect, dot_grid_layout(CanvasViewState::new([24.0, -12.0], 1.5)));

        assert_ne!(base, zoomed);
    }

    #[test]
    fn empty_state_card_only_shows_for_truly_empty_board() {
        assert!(should_show_empty_state_card(0, 0));
        assert!(!should_show_empty_state_card(1, 0));
        assert!(!should_show_empty_state_card(1, 1));
    }
}
