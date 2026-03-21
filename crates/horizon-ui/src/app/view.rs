use egui::{Context, Pos2, Rect, Ui, Vec2, emath::TSTransform};
use horizon_core::{PanelId, WorkspaceId};

use super::{HorizonApp, PANEL_PADDING, WS_BG_PAD, WS_TITLE_HEIGHT};

impl HorizonApp {
    pub(super) fn ensure_workspace_visible(&mut self, ctx: &Context) -> WorkspaceId {
        let workspace_count_before = self.board.workspaces.len();
        let workspace_id = self.board.ensure_workspace();
        self.reveal_initial_workspace(ctx, workspace_id, workspace_count_before);
        workspace_id
    }

    pub(super) fn focus_active_workspace(&mut self, ctx: &Context, left_align: bool) -> bool {
        self.active_attached_workspace_id()
            .is_some_and(|workspace_id| self.focus_workspace_visible(ctx, workspace_id, left_align))
    }

    pub(super) fn fit_active_workspace(&mut self, ctx: &Context) -> bool {
        self.active_attached_workspace_id()
            .is_some_and(|workspace_id| self.fit_workspace_visible(ctx, workspace_id))
    }

    pub(super) fn create_workspace_visible(&mut self, ctx: &Context, name: &str) -> WorkspaceId {
        let workspace_count_before = self.board.workspaces.len();
        let workspace_id = self.board.create_workspace(name);
        self.reveal_initial_workspace(ctx, workspace_id, workspace_count_before);
        workspace_id
    }

    pub(super) fn create_workspace_at_visible(&mut self, ctx: &Context, name: &str, position: [f32; 2]) -> WorkspaceId {
        let workspace_count_before = self.board.workspaces.len();
        let workspace_id = self.board.create_workspace_at(name, position);
        self.reveal_initial_workspace(ctx, workspace_id, workspace_count_before);
        workspace_id
    }

    #[profiling::function]
    pub(super) fn reset_view(&mut self, ctx: &Context) {
        self.canvas_view.set_zoom(horizon_core::DEFAULT_CANVAS_ZOOM);
        self.pan_target = None;

        if let Some(workspace_id) = self
            .board
            .active_workspace
            .filter(|workspace_id| !self.workspace_is_detached(*workspace_id))
            .or_else(|| self.leftmost_workspace_id())
            && let Some((pos, size)) = self.workspace_focus_frame(workspace_id)
        {
            self.board.focus_workspace(workspace_id);
            let canvas_rect = self.canvas_rect(ctx);
            let pan_offset = aligned_pan_offset(canvas_rect, pos, size, self.canvas_view.zoom, true);
            self.canvas_view.set_pan_offset([pan_offset.x, pan_offset.y]);
        } else {
            self.canvas_view = horizon_core::CanvasViewState::default();
        }

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
        let canvas_rect = self.canvas_rect(ctx);
        self.pan_target = Some(aligned_pan_offset(
            canvas_rect,
            canvas_pos,
            canvas_size,
            self.canvas_view.zoom,
            left_align,
        ));
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

    pub(super) fn focus_panel_visible(&mut self, ctx: &Context, panel_id: PanelId, left_align: bool) {
        self.board.focus(panel_id);
        if let Some((pos, size)) = self.panel_focus_frame(panel_id) {
            self.pan_to_canvas_pos_aligned(ctx, pos, size, left_align);
        }
    }

    pub(super) fn focus_workspace_bounds(&mut self, ctx: &Context, min: [f32; 2], max: [f32; 2], left_align: bool) {
        let pos = Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT);
        let size = Vec2::new(
            max[0] - min[0] + 2.0 * WS_BG_PAD,
            max[1] - min[1] + 2.0 * WS_BG_PAD + WS_TITLE_HEIGHT,
        );
        self.pan_to_canvas_pos_aligned(ctx, pos, size, left_align);
    }

    pub(super) fn focus_workspace_visible(
        &mut self,
        ctx: &Context,
        workspace_id: WorkspaceId,
        left_align: bool,
    ) -> bool {
        if self.workspace_is_detached(workspace_id) {
            return false;
        }

        let Some((pos, size)) = self.workspace_focus_frame(workspace_id) else {
            return false;
        };

        self.board.focus_workspace(workspace_id);
        self.pan_to_canvas_pos_aligned(ctx, pos, size, left_align);
        true
    }

    pub(super) fn fit_workspace_visible(&mut self, ctx: &Context, workspace_id: WorkspaceId) -> bool {
        if self.workspace_is_detached(workspace_id) {
            return false;
        }

        let Some((pos, size)) = self.workspace_focus_frame(workspace_id) else {
            return false;
        };

        let canvas_rect = self.canvas_rect(ctx);
        let zoom = fit_zoom_for_frame(canvas_rect.size(), size, Vec2::splat(64.0));
        let pan_offset = aligned_pan_offset(canvas_rect, pos, size, zoom, false);

        self.board.focus_workspace(workspace_id);
        self.pan_target = None;
        self.canvas_view.set_zoom(zoom);
        self.canvas_view.set_pan_offset([pan_offset.x, pan_offset.y]);
        self.mark_runtime_dirty();
        true
    }

    fn reveal_initial_workspace(&mut self, ctx: &Context, workspace_id: WorkspaceId, workspace_count_before: usize) {
        if workspace_count_before != 0 {
            return;
        }

        self.board.focus_workspace(workspace_id);
        if let Some((pos, size)) = self.workspace_focus_frame(workspace_id) {
            self.pan_to_canvas_pos_aligned(ctx, pos, size, true);
        }
    }

    fn active_attached_workspace_id(&self) -> Option<WorkspaceId> {
        self.board
            .active_workspace
            .filter(|workspace_id| !self.workspace_is_detached(*workspace_id))
            .or_else(|| self.leftmost_workspace_id())
    }

    fn workspace_focus_frame(&self, workspace_id: WorkspaceId) -> Option<(Pos2, Vec2)> {
        if let Some((min, max)) = self.board.workspace_bounds(workspace_id) {
            return Some((
                Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT),
                Vec2::new(
                    max[0] - min[0] + 2.0 * WS_BG_PAD,
                    max[1] - min[1] + 2.0 * WS_BG_PAD + WS_TITLE_HEIGHT,
                ),
            ));
        }

        self.board.workspace(workspace_id).map(|workspace| {
            (
                Pos2::new(workspace.position[0], workspace.position[1]),
                Vec2::new(super::WS_EMPTY_SIZE[0], super::WS_EMPTY_SIZE[1]),
            )
        })
    }

    fn panel_focus_frame(&self, panel_id: PanelId) -> Option<(Pos2, Vec2)> {
        self.board
            .panel(panel_id)
            .map(|panel| panel_focus_frame(panel.layout.position, panel.layout.size))
    }
}

fn panel_focus_frame(position: [f32; 2], size: [f32; 2]) -> (Pos2, Vec2) {
    (
        Pos2::new(position[0] - PANEL_PADDING, position[1] - PANEL_PADDING),
        Vec2::new(size[0] + PANEL_PADDING * 2.0, size[1] + PANEL_PADDING * 2.0),
    )
}

fn fit_zoom_for_frame(canvas_size: Vec2, frame_size: Vec2, margin: Vec2) -> f32 {
    if frame_size.x <= f32::EPSILON || frame_size.y <= f32::EPSILON {
        return horizon_core::DEFAULT_CANVAS_ZOOM;
    }

    let available_size = Vec2::new(
        (canvas_size.x - margin.x * 2.0).max(1.0),
        (canvas_size.y - margin.y * 2.0).max(1.0),
    );
    horizon_core::clamp_canvas_zoom((available_size.x / frame_size.x).min(available_size.y / frame_size.y))
}

fn aligned_pan_offset(canvas_rect: Rect, canvas_pos: Pos2, canvas_size: Vec2, zoom: f32, left_align: bool) -> Vec2 {
    let pan_margin = 40.0;
    let x = if left_align {
        pan_margin - canvas_pos.x * zoom
    } else {
        canvas_rect.width() * 0.5 - (canvas_pos.x + canvas_size.x * 0.5) * zoom
    };
    let y = canvas_rect.height() * 0.5 - (canvas_pos.y + canvas_size.y * 0.5) * zoom;

    Vec2::new(x, y)
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
    use horizon_core::{CanvasViewState, MAX_CANVAS_ZOOM, MIN_CANVAS_ZOOM};

    use super::{aligned_pan_offset, canvas_scene_transform, fit_zoom_for_frame, panel_focus_frame};

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

    #[test]
    fn aligned_pan_offset_left_aligns_canvas_point() {
        let rect = Rect::from_min_size(Pos2::new(210.0, 46.0), Vec2::new(1200.0, 800.0));
        let offset = aligned_pan_offset(rect, Pos2::new(320.0, 180.0), Vec2::new(420.0, 260.0), 1.0, true);

        assert!((offset.x + 280.0).abs() <= f32::EPSILON);
        assert!((offset.y - 90.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn panel_focus_frame_adds_padding_around_panel_bounds() {
        let (pos, size) = panel_focus_frame([240.0, 120.0], [520.0, 340.0]);

        assert_eq!(pos, Pos2::new(232.0, 112.0));
        assert_eq!(size, Vec2::new(536.0, 356.0));
    }

    #[test]
    fn fit_zoom_for_frame_uses_most_constrained_axis() {
        let zoom = fit_zoom_for_frame(Vec2::new(1200.0, 800.0), Vec2::new(600.0, 300.0), Vec2::splat(64.0));

        assert!((zoom - 1.786_666_6).abs() < 0.000_1);
    }

    #[test]
    fn fit_zoom_for_frame_clamps_large_and_small_values() {
        let zoomed_out = fit_zoom_for_frame(Vec2::new(600.0, 400.0), Vec2::new(4000.0, 3000.0), Vec2::splat(64.0));
        let zoomed_in = fit_zoom_for_frame(Vec2::new(1600.0, 1000.0), Vec2::new(100.0, 80.0), Vec2::splat(64.0));

        assert!((zoomed_out - MIN_CANVAS_ZOOM).abs() <= f32::EPSILON);
        assert!((zoomed_in - MAX_CANVAS_ZOOM).abs() <= f32::EPSILON);
    }
}
