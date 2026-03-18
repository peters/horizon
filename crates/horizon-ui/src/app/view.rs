use egui::{Context, Pos2, Rect, Ui, Vec2, emath::TSTransform};

use super::{HorizonApp, WS_BG_PAD, WS_TITLE_HEIGHT};

impl HorizonApp {
    #[profiling::function]
    pub(super) fn reset_view(&mut self) {
        self.canvas_view = horizon_core::CanvasViewState::default();
        self.pan_target = None;
        self.mark_runtime_dirty();
    }

    #[profiling::function]
    pub(super) fn animate_pan(&mut self, ctx: &Context) {
        if let Some(target) = self.pan_target {
            let dt = ctx.input(|input| input.predicted_dt);
            let t = (20.0 * dt).min(1.0);
            let current = Vec2::new(self.canvas_view.pan_offset[0], self.canvas_view.pan_offset[1]);
            let pan_offset = current + (target - current) * t;
            self.canvas_view.set_pan_offset([pan_offset.x, pan_offset.y]);
            if (pan_offset - target).length_sq() < 1.0 {
                self.canvas_view.set_pan_offset([target.x, target.y]);
                self.pan_target = None;
            }
            self.mark_runtime_dirty();
        }
    }

    pub(super) fn pan_to_canvas_pos_aligned(
        &mut self,
        ctx: &Context,
        canvas_pos: Pos2,
        canvas_size: Vec2,
        left_align: bool,
    ) {
        let canvas_rect = Self::canvas_rect(ctx, self.sidebar_visible);
        let zoom = self.canvas_view.zoom;
        let pan_margin = 40.0;
        let x = if left_align {
            pan_margin - canvas_pos.x * zoom
        } else {
            canvas_rect.width() * 0.5 - (canvas_pos.x + canvas_size.x * 0.5) * zoom
        };
        let y = canvas_rect.height() * 0.5 - (canvas_pos.y + canvas_size.y * 0.5) * zoom;

        self.pan_target = Some(Vec2::new(x, y));
    }

    pub(super) fn canvas_to_screen(&self, canvas_rect: Rect, position: Pos2) -> Pos2 {
        let screen = self
            .canvas_view
            .canvas_to_screen(canvas_origin(canvas_rect), [position.x, position.y]);
        Pos2::new(screen[0], screen[1])
    }

    pub(super) fn screen_to_canvas(&self, canvas_rect: Rect, screen_pos: Pos2) -> Pos2 {
        let canvas = self
            .canvas_view
            .screen_to_canvas(canvas_origin(canvas_rect), [screen_pos.x, screen_pos.y]);
        Pos2::new(canvas[0], canvas[1])
    }

    pub(super) fn canvas_size_to_screen(&self, canvas_size: Vec2) -> Vec2 {
        let screen = self.canvas_view.canvas_size_to_screen([canvas_size.x, canvas_size.y]);
        Vec2::new(screen[0], screen[1])
    }

    pub(super) fn apply_canvas_layer_transform(&self, ui: &mut Ui, canvas_rect: Rect) {
        let transform = canvas_scene_transform(canvas_rect, self.canvas_view);
        ui.ctx().set_transform_layer(ui.layer_id(), transform);
        ui.set_clip_rect(transform.inverse() * canvas_rect);
    }

    pub(super) fn zoom_canvas_at(&mut self, canvas_rect: Rect, screen_anchor: Pos2, zoom: f32) -> bool {
        let current_zoom = self.canvas_view.zoom;
        let next_zoom = horizon_core::clamp_canvas_zoom(zoom);
        if (next_zoom - current_zoom).abs() <= f32::EPSILON {
            return false;
        }

        self.pan_target = None;
        self.canvas_view.zoom_about_screen_anchor(
            canvas_origin(canvas_rect),
            [screen_anchor.x, screen_anchor.y],
            next_zoom,
        );
        self.mark_runtime_dirty();
        true
    }

    pub(super) fn focus_workspace_bounds(&mut self, ctx: &Context, min: [f32; 2], max: [f32; 2], left_align: bool) {
        let pos = Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT);
        let size = Vec2::new(
            max[0] - min[0] + 2.0 * WS_BG_PAD,
            max[1] - min[1] + 2.0 * WS_BG_PAD + WS_TITLE_HEIGHT,
        );
        self.pan_to_canvas_pos_aligned(ctx, pos, size, left_align);
    }
}

#[must_use]
pub(super) fn canvas_scene_transform(canvas_rect: Rect, canvas_view: horizon_core::CanvasViewState) -> TSTransform {
    TSTransform::from_translation(
        canvas_rect.min.to_vec2() + Vec2::new(canvas_view.pan_offset[0], canvas_view.pan_offset[1]),
    ) * TSTransform::from_scaling(canvas_view.zoom)
}

#[must_use]
fn canvas_origin(canvas_rect: Rect) -> [f32; 2] {
    [canvas_rect.min.x, canvas_rect.min.y]
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect, Vec2};
    use horizon_core::CanvasViewState;

    use super::canvas_scene_transform;

    #[test]
    fn canvas_scene_transform_matches_canvas_view_mapping() {
        let rect = Rect::from_min_size(Pos2::new(210.0, 46.0), Vec2::new(1200.0, 800.0));
        let view = CanvasViewState::new([48.0, -16.0], 1.5);
        let point = Pos2::new(320.0, 180.0);

        let mapped = canvas_scene_transform(rect, view) * point;
        let expected = view.canvas_to_screen([rect.min.x, rect.min.y], [point.x, point.y]);

        assert!((mapped.x - expected[0]).abs() <= f32::EPSILON);
        assert!((mapped.y - expected[1]).abs() <= f32::EPSILON);
    }

    #[test]
    fn inverse_transform_round_trips_screen_points() {
        let rect = Rect::from_min_size(Pos2::new(210.0, 46.0), Vec2::new(1200.0, 800.0));
        let view = CanvasViewState::new([-72.0, 64.0], 2.0);
        let transform = canvas_scene_transform(rect, view);
        let point = Pos2::new(410.0, 220.0);

        let screen = transform * point;
        let round_trip = transform.inverse() * screen;

        assert!((round_trip.x - point.x).abs() <= f32::EPSILON);
        assert!((round_trip.y - point.y).abs() <= f32::EPSILON);
    }
}
