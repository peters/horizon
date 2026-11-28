use egui::{Context, Id, Margin, Mesh, Order, Pos2, Rect, Shape, Stroke, Vec2};

use crate::theme;

use super::util::{format_grid_position, rounded_i32};
use super::{HorizonApp, SIDEBAR_WIDTH, canvas_backdrop};

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
            SIDEBAR_WIDTH + 16.0
        } else {
            16.0
        };
        egui::Area::new(Id::unique("canvas_hud"))
            .anchor(egui::Align2::LEFT_BOTTOM, Vec2::new(hud_left, -16.0))
            .interactable(false)
            .order(Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(theme::alpha(theme::PANEL_BG(), 236))
                    .inner_margin(Margin::symmetric(12, 10))
                    .stroke(Stroke::new(1.0_f32, theme::alpha(theme::BORDER_STRONG(), 210)))
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
    pub(super) fn render_canvas(&mut self, ui: &mut egui::Ui, beside: Option<Rect>, canvas_top: f32) {
        // The canvas paints only in the rectangle that the Settings panel leaves,
        // whatever the panel's content asked for: the panel draws first and
        // answers its rectangle, and no painting of the canvas starts inside it.
        let rect = canvas_paint_rect(ui.available_rect_before_wrap(), beside, canvas_top);
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.shrink_clip_rect(rect);
            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(theme::BG()))
                .show(ui, |ui| {
                    canvas_backdrop::paint(ui, &mut self.canvas_backdrop_cache, rect);
                    paint_dot_grid(ui, self.canvas_view, &mut self.canvas_grid_cache);
                });
        });
    }
}

/// The rectangle that the canvas paints in: what `ui` has left under the toolbar,
/// and never the rectangle of the panel beside it.
///
/// The toolbar of the root window is an overlay, so the room that `ui` offers
/// starts at the top of the window and not under the toolbar. `canvas_top` ends
/// the canvas where the person sees it, so the backdrop of the canvas is a share
/// of the canvas that the person looks at.
fn canvas_paint_rect(available: Rect, beside: Option<Rect>, canvas_top: f32) -> Rect {
    let min = Pos2::new(available.min.x, available.min.y.max(canvas_top));
    match beside {
        Some(panel) => Rect::from_min_max(min, Pos2::new(available.max.x.min(panel.min.x), available.max.y)),
        None => Rect::from_min_max(min, available.max),
    }
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
        CanvasGridCacheKey, GRID_SPACING, MIN_GRID_SCREEN_SPACING, canvas_paint_rect, dot_grid_axis_count,
        dot_grid_layout,
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

    /// The canvas takes the rectangle that the panel beside it leaves, and a
    /// panel whose content asked for more than it reserved ends the canvas at
    /// the edge it reserved, not at the edge that it reported.
    #[test]
    fn the_canvas_paints_only_beside_the_settings_panel() {
        let available = Rect::from_min_max(Pos2::new(210.0, 46.0), Pos2::new(1480.0, 900.0));
        let canvas_top = available.min.y;
        let reserved = Rect::from_min_max(Pos2::new(1036.0, 0.0), Pos2::new(1480.0, 900.0));
        let overflowing = Rect::from_min_max(Pos2::new(1036.0, 0.0), Pos2::new(1536.9, 900.0));

        for panel in [reserved, overflowing] {
            let painted = canvas_paint_rect(available, Some(panel), canvas_top);
            assert!(
                painted.max.x <= panel.min.x,
                "the canvas {painted:?} ends at the left edge of {panel:?}"
            );
            assert!((painted.max.x - panel.min.x).abs() <= f32::EPSILON);
            assert!((painted.min.x - available.min.x).abs() <= f32::EPSILON);
        }
        assert_eq!(canvas_paint_rect(available, None, canvas_top), available);

        // The Server tab draws the QR plate of an invitation, and a plate is never
        // wider than the panel: the canvas ends at the same edge as on every tab.
        let server_tab_width = crate::app::settings::settings_panel_default_width(1480.0);
        let server_tab = Rect::from_min_max(
            Pos2::new(available.max.x - server_tab_width, 0.0),
            Pos2::new(available.max.x, available.max.y),
        );
        let painted = canvas_paint_rect(available, Some(server_tab), canvas_top);
        assert!(
            painted.max.x <= server_tab.min.x + f32::EPSILON,
            "the canvas {painted:?} ends at the Server tab at {server_tab:?}"
        );

        // A panel that is wider than the window leaves an empty canvas, and never
        // a rectangle that runs backwards.
        let wider_than_the_window = Rect::from_min_max(Pos2::new(-40.0, 0.0), Pos2::new(1600.0, 900.0));
        let empty = canvas_paint_rect(available, Some(wider_than_the_window), canvas_top);
        assert!(
            empty.max.x <= available.min.x,
            "a panel wider than the window leaves no canvas: {empty:?}"
        );
    }

    /// The toolbar of the root window is an overlay, so the room that `ui` offers
    /// starts at the top of the window. The canvas starts under the toolbar
    /// whatever that room says, because the backdrop of the canvas is a share of
    /// the canvas that the person sees.
    #[test]
    fn the_canvas_starts_under_the_toolbar() {
        let room_from_the_window_top = Rect::from_min_max(Pos2::new(210.0, 0.0), Pos2::new(1480.0, 900.0));
        let room_under_the_toolbar = Rect::from_min_max(Pos2::new(210.0, 46.0), Pos2::new(1480.0, 900.0));

        for available in [room_from_the_window_top, room_under_the_toolbar] {
            let painted = canvas_paint_rect(available, None, 46.0);
            assert!(
                (painted.min.y - 46.0).abs() <= f32::EPSILON,
                "the canvas {painted:?} starts under the toolbar of {available:?}"
            );
            assert!((painted.max.y - available.max.y).abs() <= f32::EPSILON);
        }
    }
}
