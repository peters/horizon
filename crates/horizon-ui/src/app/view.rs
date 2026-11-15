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

    /// The workspace that carries `name`, or a new one with that name that is brought
    /// into the view.
    pub(super) fn ensure_workspace_of_name(&mut self, ctx: &Context, name: &str) -> WorkspaceId {
        if let Some(workspace) = self.board.workspaces.iter().find(|workspace| workspace.name == name) {
            return workspace.id;
        }
        self.create_workspace_visible(ctx, name)
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
    pub(super) fn zoom_reset(&mut self, canvas_rect: Rect, screen_anchor: Pos2) -> bool {
        self.zoom_canvas_at(canvas_rect, screen_anchor, horizon_core::DEFAULT_CANVAS_ZOOM)
    }

    /// A zoom that a shortcut asks for, at the middle of the canvas.
    ///
    /// A zoom at a pointer keeps the place of that pointer. A zoom of a shortcut
    /// has no pointer to keep, so it keeps the middle of the canvas: a panel that
    /// stood out of the view, or partly out of it, before the zoom is not a panel
    /// that the person reads, and it does not pull the view to itself.
    ///
    /// The one exception is a panel that the view held whole before the zoom: the
    /// person reads that panel, and a zoom that would push it out of the canvas
    /// keeps it whole by the rule of a jump to that panel.
    pub(super) fn zoom_by_shortcut(&mut self, ctx: &Context, zoom: f32) -> bool {
        let rect = self.canvas_rect(ctx);
        let held = self.focused_panel_whole(ctx, rect);
        let changed = self.zoom_canvas_at(rect, rect.center(), zoom);
        if changed && held {
            self.keep_focused_panel_whole(ctx, rect);
        }
        changed
    }

    /// Whether the panel with the keyboard focus is whole in `rect` at the view
    /// that the app holds now. No focus, or a panel that the board does not hold,
    /// is not a panel for the view to keep.
    fn focused_panel_whole(&self, ctx: &Context, rect: Rect) -> bool {
        let Some(panel) = self.board.focused.and_then(|panel_id| self.board.panel(panel_id)) else {
            return false;
        };
        let placed = Rect::from_min_size(panel.layout.position.into(), panel.layout.size.into());
        let placed = canvas_scene_transform(self.canvas_rect(ctx), self.canvas_view) * placed;
        rect.contains_rect(placed)
    }

    /// Moves the view so that the panel with the keyboard focus is whole in
    /// `rect` at the zoom of the view. The caller calls this only for a panel that
    /// the view held whole before the zoom, so a panel that is whole again stays
    /// as it is, and a panel that is larger than the view at this zoom keeps the
    /// place that the zoom gave it.
    fn keep_focused_panel_whole(&mut self, ctx: &Context, rect: Rect) {
        if self.focused_panel_whole(ctx, rect) {
            return;
        }
        let Some(panel_id) = self.board.focused else {
            return;
        };
        let Some((panel_frame, workspace_pos)) = self.reveal_frames(panel_id) else {
            return;
        };
        let Some(pan) = reveal_pan_at_zoom(rect, panel_frame, workspace_pos, self.canvas_view.zoom) else {
            return;
        };
        self.pan_target = None;
        self.canvas_view.set_pan_offset([pan.x, pan.y]);
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
        let target = aligned_pan_offset(canvas_rect, canvas_pos, canvas_size, self.canvas_view.zoom, left_align);
        let current = Vec2::new(self.canvas_view.pan_offset[0], self.canvas_view.pan_offset[1]);
        self.pan_target = ((target - current).length_sq() >= 1.0).then_some(target);
    }

    pub(super) fn align_initial_view_to_workspace(&mut self, ctx: &Context, workspace_id: WorkspaceId) -> bool {
        if self.workspace_is_detached(workspace_id) {
            return false;
        }

        let Some((pos, _size)) = self.workspace_focus_frame(workspace_id) else {
            return false;
        };
        let canvas_rect = self.canvas_rect(ctx);
        self.canvas_view.align_canvas_point_to_screen(
            [canvas_rect.min.x, canvas_rect.min.y],
            [pos.x, pos.y],
            [canvas_rect.min.x + 40.0, canvas_rect.center().y],
        );
        self.pan_target = None;
        true
    }

    pub(super) fn startup_workspace_view_anchor(&self, ctx: &Context) -> Option<(WorkspaceId, Pos2)> {
        let preferred_workspace = self
            .board
            .focused
            .and_then(|panel_id| self.board.panel_workspace_id(panel_id))
            .or(self.board.active_workspace);
        preferred_workspace
            .filter(|workspace_id| !self.workspace_is_detached(*workspace_id))
            .and_then(|workspace_id| {
                self.visible_workspace_screen_center(ctx, workspace_id)
                    .map(|screen_center| (workspace_id, screen_center))
            })
            .or_else(|| {
                self.board
                    .workspaces
                    .iter()
                    .filter(|workspace| !self.workspace_is_detached(workspace.id))
                    .find_map(|workspace| {
                        self.visible_workspace_screen_center(ctx, workspace.id)
                            .map(|screen_center| (workspace.id, screen_center))
                    })
            })
    }

    pub(super) fn restore_startup_workspace_view_anchor(
        &mut self,
        ctx: &Context,
        workspace_id: WorkspaceId,
        screen_anchor: Pos2,
    ) {
        if self.visible_workspace_screen_center(ctx, workspace_id).is_some() {
            return;
        }

        let Some((pos, size)) = self.workspace_focus_frame(workspace_id) else {
            return;
        };
        let current_center = self.canvas_to_screen(self.canvas_rect(ctx), pos + size * 0.5);
        let delta = screen_anchor - current_center;
        self.canvas_view.set_pan_offset([
            self.canvas_view.pan_offset[0] + delta.x,
            self.canvas_view.pan_offset[1] + delta.y,
        ]);
        self.pan_target = None;
    }

    fn visible_workspace_screen_center(&self, ctx: &Context, workspace_id: WorkspaceId) -> Option<Pos2> {
        let (pos, size) = self.workspace_focus_frame(workspace_id)?;
        let canvas_rect = self.canvas_rect(ctx);
        let screen_min = self.canvas_to_screen(canvas_rect, pos);
        let screen_size = self.canvas_size_to_screen(size);
        let screen_rect = Rect::from_min_size(screen_min, screen_size);
        canvas_rect
            .contains(screen_rect.center())
            .then_some(screen_rect.center())
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

    /// Focus a panel and move the view so the panel is visible: its workspace
    /// is left-aligned next to the canvas edge (close to the sidebar), the
    /// view zooms out just enough for the panel to fit (never zooms in), or
    /// bottom-aligns the panel when it stays larger than the viewport even at
    /// the minimum zoom.
    pub(super) fn reveal_panel_visible(&mut self, ctx: &Context, panel_id: PanelId) {
        self.reveal_panel_in_rect(panel_id, self.reveal_rect(ctx));
    }

    /// The rectangle that a jump reveals a panel into.
    ///
    /// The canvas leaves the sidebar, the settings panel and the settings bar
    /// out, so a revealed panel never stands under them. The minimap sits over
    /// a corner of the canvas, and a revealed panel keeps clear of that corner
    /// while the rest of the canvas still holds the panel with the margin of a
    /// jump.
    pub(in crate::app) fn reveal_rect(&self, ctx: &Context) -> Rect {
        let canvas = self.canvas_rect(ctx);
        let Some(minimap) = self.minimap_overlay_rect(ctx) else {
            return canvas;
        };
        // The minimap is held in the corner that it stands in, so the reveal
        // takes the corner that holds the overlay rather than the whole canvas.
        let right = if minimap.max.x > canvas.center().x {
            minimap.min.x
        } else {
            canvas.max.x
        };
        let bottom = if minimap.max.y > canvas.center().y {
            minimap.min.y
        } else {
            canvas.max.y
        };
        let clear = Rect::from_min_max(canvas.min, Pos2::new(right.max(canvas.min.x), bottom.max(canvas.min.y)));
        // A canvas that the minimap leaves no room in keeps the whole of it.
        if fits_in_canvas(clear.size(), Vec2::ZERO, Vec2::splat(REVEAL_MARGIN), 1.0) {
            clear
        } else {
            canvas
        }
    }

    pub(super) fn reveal_panel_in_rect(&mut self, panel_id: PanelId, canvas_rect: Rect) {
        let Some((panel_frame, workspace_pos)) = self.reveal_frames(panel_id) else {
            return;
        };

        let (zoom, pan_offset) = reveal_view_state(
            canvas_rect,
            panel_frame,
            workspace_pos,
            self.canvas_view.zoom,
            self.template_config.canvas.fit_floor(),
        );
        self.board.focus(panel_id);
        self.pan_target = None;
        self.canvas_view.set_zoom(zoom);
        self.canvas_view.set_pan_offset([pan_offset.x, pan_offset.y]);
        self.mark_runtime_dirty();
    }

    /// Recovery must bring the device itself into view even when its workspace
    /// is too wide to fit at the minimum zoom. It must not move keyboard focus.
    pub(super) fn reveal_device_in_rect(&mut self, panel_id: PanelId, canvas_rect: Rect) {
        let Some(panel) = self.board.panel(panel_id) else {
            return;
        };
        let position = self.arranged_panel_position(panel_id, panel.workspace_id, panel.layout.position.into());
        let (position, size) = panel_focus_frame(position.into(), panel.layout.size);
        let zoom = self
            .canvas_view
            .zoom
            .min(fit_zoom_for_frame(canvas_rect.size(), size, Vec2::splat(REVEAL_MARGIN)));
        let pan = aligned_pan_offset(canvas_rect, position, size, zoom, false);
        self.pan_target = None;
        self.canvas_view.set_zoom(zoom);
        self.canvas_view.set_pan_offset(pan.into());
        self.mark_runtime_dirty();
    }

    /// The revealed panel's frame plus the left edge of the workspace it
    /// belongs to (falling back to the panel's own left edge when the
    /// workspace cannot be resolved).
    fn reveal_frames(&self, panel_id: PanelId) -> Option<((Pos2, Vec2), Pos2)> {
        let panel = self.board.panel(panel_id)?;
        let panel_frame = panel_focus_frame(panel.layout.position, panel.layout.size);
        let workspace_pos = self
            .board
            .panel_workspace_id(panel_id)
            .and_then(|workspace_id| self.workspace_focus_frame(workspace_id))
            .map_or(panel_frame.0, |(pos, _size)| pos);
        Some((panel_frame, workspace_pos))
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

    pub(super) fn focus_workspace_in_rect(&mut self, workspace_id: WorkspaceId, canvas_rect: Rect) -> bool {
        if !self.center_workspace_in_rect(workspace_id, canvas_rect) {
            return false;
        }
        self.board.focus_workspace(workspace_id);
        true
    }

    /// Puts `workspace_id` in the middle of `canvas_rect` with the zoom the person chose, and
    /// leaves focus where it is: a panel they type in keeps the keyboard.
    pub(super) fn center_workspace_in_rect(&mut self, workspace_id: WorkspaceId, canvas_rect: Rect) -> bool {
        let Some((pos, size)) = self.workspace_focus_frame(workspace_id) else {
            return false;
        };

        let pan_offset = aligned_pan_offset(canvas_rect, pos, size, self.canvas_view.zoom, false);
        self.pan_target = None;
        self.canvas_view.set_pan_offset([pan_offset.x, pan_offset.y]);
        self.mark_runtime_dirty();
        true
    }

    pub(super) fn fit_workspace_visible(&mut self, ctx: &Context, workspace_id: WorkspaceId) -> bool {
        if self.workspace_is_detached(workspace_id) {
            return false;
        }

        self.fit_workspace_in_rect(workspace_id, self.canvas_rect(ctx))
    }

    pub(super) fn fit_workspace_in_rect(&mut self, workspace_id: WorkspaceId, canvas_rect: Rect) -> bool {
        let Some((pos, size)) = self.workspace_focus_frame(workspace_id) else {
            return false;
        };

        let margin = Vec2::splat(FIT_MARGIN);
        let zoom = fit_workspace_zoom(
            canvas_rect.size(),
            size,
            margin,
            self.template_config.canvas.fit_floor(),
        );
        // A workspace that does not fit at the floor of the zoom is not made any
        // smaller: it is shown from its top left corner, and the minimap shows
        // where the rest of it is.
        let pan_offset = if fits_in_canvas(canvas_rect.size(), size, margin, zoom) {
            aligned_pan_offset(canvas_rect, pos, size, zoom, false)
        } else {
            top_left_pan_offset(pos, zoom)
        };

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

    pub(super) fn workspace_focus_frame(&self, workspace_id: WorkspaceId) -> Option<(Pos2, Vec2)> {
        let bounds = self.board.workspace_bounds_map();
        if let Some(&(min, max)) = bounds.get(&workspace_id) {
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
                Self::empty_workspace_size(),
            )
        })
    }

    /// The frame an empty workspace is drawn at.
    pub(super) fn empty_workspace_size() -> Vec2 {
        Vec2::new(super::WS_EMPTY_SIZE[0], super::WS_EMPTY_SIZE[1])
    }
}

fn panel_focus_frame(position: [f32; 2], size: [f32; 2]) -> (Pos2, Vec2) {
    (
        Pos2::new(position[0] - PANEL_PADDING, position[1] - PANEL_PADDING),
        Vec2::new(size[0] + PANEL_PADDING * 2.0, size[1] + PANEL_PADDING * 2.0),
    )
}

/// Inset (px) kept between the revealed panel and the viewport edge.
const REVEAL_MARGIN: f32 = 40.0;

/// Inset (px) kept between a fitting workspace and the canvas edge.
const FIT_MARGIN: f32 = 64.0;

/// Inset (px) kept between a placed workspace and the canvas edge.
const PAN_MARGIN: f32 = 40.0;

/// View state that keeps a revealed panel visible: the panel's workspace is
/// left-aligned next to the canvas edge (close to the sidebar), the current
/// zoom is kept while the panel already fits from there, otherwise the view
/// zooms out just enough for the panel to fit (never zooming in).
fn reveal_view_state(
    canvas_rect: Rect,
    panel: (Pos2, Vec2),
    workspace_pos: Pos2,
    current_zoom: f32,
    floor: f32,
) -> (f32, Vec2) {
    let (panel_pos, panel_size) = panel;
    // The horizontal fit spans the workspace's left edge to the panel's right
    // edge because the view left-aligns the workspace, not the panel.
    let fit_width = (panel_pos.x + panel_size.x) - workspace_pos.x;
    let frame = Vec2::new(fit_width, panel_size.y);
    let margin = Vec2::splat(REVEAL_MARGIN);
    let fit = fit_zoom_for_frame(canvas_rect.size(), frame, margin);
    let zoom = horizon_core::reveal_zoom(current_zoom, fit, floor);
    // The workspace is left-aligned while its left edge and the panel fit
    // together. A workspace that is too wide at this zoom still shows its panel
    // whole, at the panel's own left edge. A panel that is larger than the view
    // even at the floor of a Fit is shown from its top left, so the person reads
    // the start of it.
    let pan = reveal_pan_at_zoom(canvas_rect, panel, workspace_pos, zoom)
        .unwrap_or_else(|| top_left_pan_offset(panel_pos, zoom));
    (zoom, pan)
}

/// The pan of a jump to a panel at `zoom`, or `None` when the panel is larger
/// than the view at that zoom.
///
/// The workspace's left edge comes first, next to the canvas edge, while it and
/// the panel fit together. A workspace that is too wide at this zoom still shows
/// its panel whole at the panel's own left edge.
fn reveal_pan_at_zoom(canvas_rect: Rect, panel: (Pos2, Vec2), workspace_pos: Pos2, zoom: f32) -> Option<Vec2> {
    let (panel_pos, panel_size) = panel;
    let margin = Vec2::splat(REVEAL_MARGIN);
    let fit_width = (panel_pos.x + panel_size.x) - workspace_pos.x;
    if fits_in_canvas(canvas_rect.size(), Vec2::new(fit_width, panel_size.y), margin, zoom) {
        Some(reveal_pan_offset(
            canvas_rect,
            panel_pos,
            panel_size,
            workspace_pos,
            zoom,
        ))
    } else if fits_in_canvas(canvas_rect.size(), panel_size, margin, zoom) {
        Some(reveal_pan_offset(canvas_rect, panel_pos, panel_size, panel_pos, zoom))
    } else {
        None
    }
}

fn reveal_pan_offset(canvas_rect: Rect, panel_pos: Pos2, panel_size: Vec2, workspace_pos: Pos2, zoom: f32) -> Vec2 {
    // The workspace's left edge is pinned next to the canvas edge (close to the
    // sidebar) instead of the panel being centered, so the workspace reads from
    // the left. When the panel is taller than the viewport (oversized panel at
    // the minimum zoom) it is bottom-aligned so the panel's bottom edge — where
    // a terminal's active prompt lives — stays visible.
    let x = REVEAL_MARGIN - workspace_pos.x * zoom;
    let y = if panel_size.y * zoom + REVEAL_MARGIN * 2.0 <= canvas_rect.height() {
        canvas_rect.height() * 0.5 - (panel_pos.y + panel_size.y * 0.5) * zoom
    } else {
        canvas_rect.height() - REVEAL_MARGIN - (panel_pos.y + panel_size.y) * zoom
    };
    Vec2::new(x, y)
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

fn fit_workspace_zoom(canvas_size: Vec2, frame_size: Vec2, margin: Vec2, floor: f32) -> f32 {
    fit_zoom_for_frame(canvas_size, frame_size, margin)
        .max(floor)
        .min(horizon_core::DEFAULT_CANVAS_ZOOM)
}

/// Whether a frame of `zoom` keeps the margin of a Fit on every side of the
/// canvas.
fn fits_in_canvas(canvas_size: Vec2, frame_size: Vec2, margin: Vec2, zoom: f32) -> bool {
    frame_size.x * zoom <= canvas_size.x - margin.x * 2.0 && frame_size.y * zoom <= canvas_size.y - margin.y * 2.0
}

/// The pan offset that puts the top left corner of a workspace at the top left
/// margin of the canvas, for a workspace that is larger than the canvas even at
/// the floor of the zoom.
fn top_left_pan_offset(canvas_pos: Pos2, zoom: f32) -> Vec2 {
    Vec2::new(PAN_MARGIN - canvas_pos.x * zoom, PAN_MARGIN - canvas_pos.y * zoom)
}

fn aligned_pan_offset(canvas_rect: Rect, canvas_pos: Pos2, canvas_size: Vec2, zoom: f32, left_align: bool) -> Vec2 {
    let pan_margin = PAN_MARGIN;
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
    use egui::{Context, Pos2, Rect, Vec2};
    use horizon_core::{CanvasViewState, MAX_CANVAS_ZOOM, MIN_CANVAS_ZOOM, RuntimeState, StartupDecision};

    use super::{
        FIT_MARGIN, PAN_MARGIN, aligned_pan_offset, canvas_origin, canvas_scene_transform, fit_workspace_zoom,
        fit_zoom_for_frame, fits_in_canvas, panel_focus_frame, reveal_view_state, top_left_pan_offset,
    };
    use horizon_core::DEFAULT_FIT_FLOOR_ZOOM;

    use tempfile::TempDir;

    use crate::app::HorizonApp;
    use crate::app::test_support::{
        editor_panel_state, editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup,
    };

    use super::REVEAL_MARGIN;

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
        let oversized = Vec2::new(600.0, 400.0) / MIN_CANVAS_ZOOM;
        let zoomed_out = fit_zoom_for_frame(Vec2::new(600.0, 400.0), oversized, Vec2::splat(64.0));
        let zoomed_in = fit_zoom_for_frame(Vec2::new(1600.0, 1000.0), Vec2::new(100.0, 80.0), Vec2::splat(64.0));

        assert!((zoomed_out - MIN_CANVAS_ZOOM).abs() <= f32::EPSILON);
        assert!((zoomed_in - MAX_CANVAS_ZOOM).abs() <= f32::EPSILON);
    }

    #[test]
    fn workspace_fit_zooms_out_but_never_upscales_content() {
        let floor = horizon_core::DEFAULT_FIT_FLOOR_ZOOM;
        let zoomed_out = fit_workspace_zoom(
            Vec2::new(600.0, 400.0),
            Vec2::new(900.0, 600.0),
            Vec2::splat(64.0),
            floor,
        );
        let native_scale = fit_workspace_zoom(
            Vec2::new(1600.0, 1000.0),
            Vec2::new(100.0, 80.0),
            Vec2::splat(64.0),
            floor,
        );

        assert!(zoomed_out < horizon_core::DEFAULT_CANVAS_ZOOM);
        assert!((native_scale - horizon_core::DEFAULT_CANVAS_ZOOM).abs() <= f32::EPSILON);
    }

    #[test]
    fn a_workspace_fit_stops_at_the_floor_of_the_zoom() {
        let floor = horizon_core::DEFAULT_FIT_FLOOR_ZOOM;
        // The fit alone would be 0.26, which draws card text that nobody reads.
        let zoom = fit_workspace_zoom(
            Vec2::new(600.0, 400.0),
            Vec2::new(1800.0, 1200.0),
            Vec2::splat(64.0),
            floor,
        );

        assert!(
            (zoom - floor).abs() <= f32::EPSILON,
            "{zoom} is the floor of the setting"
        );
    }

    #[test]
    fn a_workspace_that_does_not_fit_at_the_floor_starts_at_its_top_left() {
        let canvas = Rect::from_min_size(Pos2::new(210.0, 46.0), Vec2::new(1200.0, 800.0));
        let canvas_size = canvas.size();
        let margin = Vec2::splat(FIT_MARGIN);
        let pos = Pos2::new(320.0, 180.0);
        let size = Vec2::new(1800.0, 1200.0);
        let zoom = fit_workspace_zoom(canvas_size, size, margin, horizon_core::DEFAULT_FIT_FLOOR_ZOOM);

        assert!(
            !fits_in_canvas(canvas_size, size, margin, zoom),
            "the workspace is larger than the canvas at the floor"
        );
        let offset = top_left_pan_offset(pos, zoom);
        assert!((offset.x + pos.x * zoom - PAN_MARGIN).abs() <= f32::EPSILON);
        assert!((offset.y + pos.y * zoom - PAN_MARGIN).abs() <= f32::EPSILON);
    }

    #[test]
    fn a_workspace_that_fits_at_the_floor_is_centred() {
        let canvas_size = Vec2::new(1200.0, 800.0);
        let margin = Vec2::splat(FIT_MARGIN);
        let size = Vec2::new(900.0, 600.0);
        let zoom = fit_workspace_zoom(canvas_size, size, margin, horizon_core::DEFAULT_FIT_FLOOR_ZOOM);

        assert!(
            fits_in_canvas(canvas_size, size, margin, zoom),
            "a workspace of the size of the canvas fits at the fit zoom"
        );
    }

    #[test]
    fn reveal_view_state_left_aligns_workspace_for_fitting_panel() {
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0));
        let panel = (Pos2::new(500.0, 300.0), Vec2::new(400.0, 300.0));
        let workspace_pos = Pos2::new(484.0, 254.0);

        let (zoom, pan) = reveal_view_state(canvas, panel, workspace_pos, 1.0, DEFAULT_FIT_FLOOR_ZOOM);

        assert!((zoom - 1.0).abs() <= f32::EPSILON);
        // x: workspace left edge at the margin: 40 - 484 ; y centered on the panel.
        assert!((pan.x - (-444.0)).abs() < 0.001);
        assert!((pan.y - (-50.0)).abs() < 0.001);
    }

    /// A panel that would fit at a zoom under the floor keeps the floor, so its
    /// text stays readable, and the view shows its top left.
    #[test]
    fn reveal_view_state_holds_the_floor_for_a_panel_that_asks_for_less() {
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0));
        // Span workspace-left -> panel-right is 1300 - 84 = 1216 wide; available
        // (1200-80) x (800-80) -> min(1120/1216, 720/900) = 0.8, under the floor.
        let panel = (Pos2::new(100.0, 50.0), Vec2::new(1200.0, 900.0));
        let workspace_pos = Pos2::new(84.0, -4.0);

        let (zoom, pan) = reveal_view_state(canvas, panel, workspace_pos, 1.0, DEFAULT_FIT_FLOOR_ZOOM);

        assert!((zoom - DEFAULT_FIT_FLOOR_ZOOM).abs() < 0.001);
        assert!(
            (pan.x - (PAN_MARGIN - panel.0.x * zoom)).abs() < 0.001,
            "the panel starts at the top-left margin"
        );
        assert!((pan.y - (PAN_MARGIN - panel.0.y * zoom)).abs() < 0.001);
    }

    #[test]
    fn reveal_view_state_never_zooms_in_past_the_default_zoom() {
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0));
        let panel = (Pos2::new(0.0, 0.0), Vec2::new(100.0, 80.0));
        let workspace_pos = Pos2::new(-16.0, -44.0);

        let (zoom, pan) = reveal_view_state(canvas, panel, workspace_pos, 1.5, DEFAULT_FIT_FLOOR_ZOOM);

        assert!((zoom - horizon_core::DEFAULT_CANVAS_ZOOM).abs() <= f32::EPSILON);
        // x: the left edge of the workspace at the margin, at the default zoom.
        assert!((pan.x - (40.0 - workspace_pos.x * zoom)).abs() < 0.001);
    }

    /// A panel taller than the view is shown from its top left at the floor: the
    /// jump reads the start of the panel instead of shrinking it to nothing.
    #[test]
    fn reveal_view_state_shows_a_panel_taller_than_the_view_from_its_top_left() {
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0));
        let extent = 1000.0 / MIN_CANVAS_ZOOM;
        let panel = (Pos2::new(100.0, 0.0), Vec2::splat(extent));
        let workspace_pos = Pos2::new(84.0, -44.0);

        let (zoom, pan) = reveal_view_state(canvas, panel, workspace_pos, 1.0, DEFAULT_FIT_FLOOR_ZOOM);

        assert!((zoom - DEFAULT_FIT_FLOOR_ZOOM).abs() <= f32::EPSILON);
        assert!((pan.x - (PAN_MARGIN - panel.0.x * zoom)).abs() < 0.001);
        assert!((pan.y - (PAN_MARGIN - panel.0.y * zoom)).abs() < 0.001);
    }

    /// A panel far to the right of its workspace shows the whole workspace width
    /// at the floor, so the left edge of that workspace stays next to the sidebar.
    #[test]
    fn reveal_view_state_keeps_off_left_panel_visible() {
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0));
        // Panel sits far right inside its workspace: the fit span runs from the
        // workspace's left edge (0) to the panel's right edge (1400) -> 1120/1400 = 0.8,
        // under the floor, so the view holds the floor and starts at the top left.
        let panel = (Pos2::new(1000.0, 0.0), Vec2::new(400.0, 300.0));
        let workspace_pos = Pos2::new(0.0, -54.0);

        let (zoom, pan) = reveal_view_state(canvas, panel, workspace_pos, 1.0, DEFAULT_FIT_FLOOR_ZOOM);

        assert!((zoom - DEFAULT_FIT_FLOOR_ZOOM).abs() < 0.001);
        // The workspace is too wide for the view at this zoom, so the panel is
        // shown whole at its own left edge and its right edge keeps the margin.
        assert!((pan.x - (40.0 - panel.0.x * zoom)).abs() < 0.001);
        let right_edge = panel.0.x * zoom + pan.x + panel.1.x * zoom;
        assert!(
            right_edge <= 1200.0 - 40.0 + 0.001,
            "the panel stays whole: {right_edge}"
        );
    }

    const VIEWPORT: [f32; 2] = [1400.0, 900.0];

    /// An app with one workspace that holds a panel at `position`.
    fn app_with_panel_at(position: [f32; 2]) -> (TempDir, Context, HorizonApp) {
        let mut workspace = editor_workspace_state("fixture", [40.0, 40.0]);
        workspace.panels = vec![editor_panel_state("notes", position)];
        let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState {
                workspaces: vec![workspace],
                ..RuntimeState::default()
            }),
        });
        for _ in 0..3 {
            run_app_frame_with_input(&ctx, &mut app, raw_input(VIEWPORT, None));
        }
        (temp, ctx, app)
    }

    /// The screen rectangle of the panel `local_id`, through the view of the app.
    fn panel_screen_rect(app: &HorizonApp, ctx: &Context, local_id: &str) -> Rect {
        let panel_id = app
            .board
            .panel_id_by_local_id(local_id)
            .expect("the panel of the fixture");
        let panel = app.board.panel(panel_id).expect("the panel of the fixture");
        let rect = Rect::from_min_size(panel.layout.position.into(), panel.layout.size.into());
        canvas_scene_transform(app.canvas_rect(ctx), app.canvas_view) * rect
    }

    /// A zoom of a shortcut keeps the zoom that it asked for and the panel of the
    /// keyboard focus whole in the canvas, while that panel was whole before the
    /// zoom and the zoom would push it past the left edge.
    #[test]
    fn a_zoom_of_a_shortcut_keeps_a_whole_focused_panel_whole() {
        let (_temp, ctx, mut app) = app_with_panel_at([40.0, 40.0]);
        let panel = app.board.panel_id_by_local_id("notes").expect("the fixture panel");
        app.board.focus(panel);
        app.canvas_view.set_zoom(1.0);
        app.canvas_view.set_pan_offset([-38.0, -38.0]);
        let canvas = app.canvas_rect(&ctx);
        assert!(
            canvas.contains_rect(panel_screen_rect(&app, &ctx, "notes")),
            "the panel starts whole at the left edge of the canvas: {:?} in {canvas:?}",
            panel_screen_rect(&app, &ctx, "notes")
        );
        let mut plain = app.canvas_view;
        plain.zoom_about_screen_anchor(canvas_origin(canvas), [canvas.center().x, canvas.center().y], 1.2);
        let placed = {
            let panel = app.board.panel(panel).expect("the fixture panel");
            Rect::from_min_size(panel.layout.position.into(), panel.layout.size.into())
        };
        let pushed = canvas_scene_transform(canvas, plain) * placed;
        assert!(
            !canvas.contains_rect(pushed),
            "a plain zoom pushes the panel past the left edge: {pushed:?} in {canvas:?}"
        );

        assert!(app.zoom_by_shortcut(&ctx, 1.2), "the shortcut changes the zoom");
        assert!(
            (app.canvas_view.zoom - 1.2).abs() < 1.0e-4,
            "the view keeps the zoom that the shortcut asked for: {}",
            app.canvas_view.zoom
        );
        let after = panel_screen_rect(&app, &ctx, "notes");
        let canvas = app.canvas_rect(&ctx);
        assert!(
            canvas.contains_rect(after),
            "the panel of the keyboard focus stays whole in the canvas: {after:?} in {canvas:?}"
        );
        assert!(
            after.min.x >= canvas.min.x + REVEAL_MARGIN - 0.5 && after.min.x <= canvas.min.x + REVEAL_MARGIN + 40.0,
            "the panel starts at the left edge that a jump gives it: {} against {}",
            after.min.x,
            canvas.min.x + REVEAL_MARGIN
        );
    }

    /// A zoom of a shortcut leaves the view at the middle of the canvas when the
    /// panel of the keyboard focus stood out of the view before it: a panel that
    /// nobody reads does not pull the view to itself.
    #[test]
    fn a_zoom_of_a_shortcut_leaves_a_panel_out_of_the_view_alone() {
        let (_temp, ctx, mut app) = app_with_panel_at([40.0, 40.0]);
        let panel = app.board.panel_id_by_local_id("notes").expect("the fixture panel");
        app.board.focus(panel);
        app.canvas_view.set_zoom(2.0);
        app.canvas_view.set_pan_offset([-1500.0, -200.0]);
        let before = panel_screen_rect(&app, &ctx, "notes");
        let canvas = app.canvas_rect(&ctx);
        assert!(
            !canvas.intersects(before),
            "the panel starts fully out of the canvas: {before:?} in {canvas:?}"
        );
        let mut expected = app.canvas_view;
        expected.zoom_about_screen_anchor(canvas_origin(canvas), [canvas.center().x, canvas.center().y], 2.2);

        assert!(app.zoom_by_shortcut(&ctx, 2.2), "the shortcut changes the zoom");
        assert!(
            (app.canvas_view.pan_offset[0] - expected.pan_offset[0]).abs() < 0.01
                && (app.canvas_view.pan_offset[1] - expected.pan_offset[1]).abs() < 0.01,
            "the zoom keeps the pan of a plain zoom at the middle: {:?} against {:?}",
            app.canvas_view.pan_offset,
            expected.pan_offset
        );
        assert!(
            !app.canvas_rect(&ctx).intersects(panel_screen_rect(&app, &ctx, "notes")),
            "the panel that nobody read stays out of the view: the view did not move to it"
        );
    }

    /// A zoom of a shortcut leaves the view at the middle of the canvas when the
    /// panel of the keyboard focus stood partly out of the view before it.
    #[test]
    fn a_zoom_of_a_shortcut_leaves_a_partly_visible_panel_alone() {
        let (_temp, ctx, mut app) = app_with_panel_at([40.0, 40.0]);
        let panel = app.board.panel_id_by_local_id("notes").expect("the fixture panel");
        app.board.focus(panel);
        app.canvas_view.set_zoom(1.0);
        app.canvas_view.set_pan_offset([-80.0, -10.0]);
        let before = panel_screen_rect(&app, &ctx, "notes");
        let canvas = app.canvas_rect(&ctx);
        assert!(
            !canvas.contains_rect(before) && canvas.intersects(before),
            "the panel starts partly out of the canvas: {before:?} in {canvas:?}"
        );
        let mut expected = app.canvas_view;
        expected.zoom_about_screen_anchor(canvas_origin(canvas), [canvas.center().x, canvas.center().y], 1.3);

        assert!(app.zoom_by_shortcut(&ctx, 1.3), "the shortcut changes the zoom");
        assert!(
            (app.canvas_view.pan_offset[0] - expected.pan_offset[0]).abs() < 0.01
                && (app.canvas_view.pan_offset[1] - expected.pan_offset[1]).abs() < 0.01,
            "the zoom keeps the pan of a plain zoom at the middle: {:?} against {:?}",
            app.canvas_view.pan_offset,
            expected.pan_offset
        );
        let after = panel_screen_rect(&app, &ctx, "notes");
        assert!(
            after.min.x < canvas.min.x && !canvas.contains_rect(after),
            "the panel keeps the edge that the zoom gave it instead of a jump: {after:?}"
        );
    }

    /// A zoom of a shortcut without a panel of the keyboard focus keeps the middle
    /// of the canvas.
    #[test]
    fn a_zoom_of_a_shortcut_without_focus_keeps_the_middle_of_the_canvas() {
        let (_temp, ctx, mut app) = app_with_panel_at([40.0, 40.0]);
        app.board.focused = None;
        app.canvas_view.set_zoom(1.4);
        app.canvas_view.set_pan_offset([-600.0, -300.0]);
        let canvas = app.canvas_rect(&ctx);
        let mut expected = app.canvas_view;
        expected.zoom_about_screen_anchor(canvas_origin(canvas), [canvas.center().x, canvas.center().y], 0.8);

        assert!(app.zoom_by_shortcut(&ctx, 0.8), "the shortcut changes the zoom");
        assert!(
            (app.canvas_view.pan_offset[0] - expected.pan_offset[0]).abs() < 0.01
                && (app.canvas_view.pan_offset[1] - expected.pan_offset[1]).abs() < 0.01,
            "the zoom keeps the pan of a plain zoom at the middle: {:?} against {:?}",
            app.canvas_view.pan_offset,
            expected.pan_offset
        );
    }

    /// A zoom of a shortcut that already holds the panel whole leaves the view where
    /// the zoom about the middle of the canvas put it: the panel decides only when it
    /// would leave the canvas.
    #[test]
    fn a_zoom_of_a_shortcut_that_keeps_the_panel_whole_leaves_the_view_alone() {
        let (_temp, ctx, mut app) = app_with_panel_at([480.0, 240.0]);
        let panel = app.board.panel_id_by_local_id("notes").expect("the fixture panel");
        app.board.focus(panel);
        app.canvas_view.set_zoom(1.0);
        app.canvas_view.set_pan_offset([0.0, 0.0]);
        let canvas = app.canvas_rect(&ctx);
        assert!(
            canvas.contains_rect(panel_screen_rect(&app, &ctx, "notes")),
            "the fixture panel starts whole in the canvas"
        );
        let mut expected = app.canvas_view;
        expected.zoom_about_screen_anchor(canvas_origin(canvas), [canvas.center().x, canvas.center().y], 0.9);

        assert!(app.zoom_by_shortcut(&ctx, 0.9), "the shortcut changes the zoom");
        assert!(
            (app.canvas_view.pan_offset[0] - expected.pan_offset[0]).abs() < 0.01
                && (app.canvas_view.pan_offset[1] - expected.pan_offset[1]).abs() < 0.01,
            "the panel keeps the view where the zoom put it: {:?} against {:?}",
            app.canvas_view.pan_offset,
            expected.pan_offset
        );
    }

    /// A zoom at a pointer keeps the canvas point under the pointer, and it does not
    /// move the panel of the keyboard focus: the pointer decides, not the panel.
    #[test]
    fn a_zoom_at_the_pointer_keeps_the_place_of_the_pointer() {
        let (_temp, ctx, mut app) = app_with_panel_at([40.0, 40.0]);
        let panel = app.board.panel_id_by_local_id("notes").expect("the fixture panel");
        app.board.focus(panel);
        app.canvas_view.set_zoom(1.0);
        app.canvas_view.set_pan_offset([-20.0, -20.0]);
        let canvas = app.canvas_rect(&ctx);
        let anchor = canvas.center();
        let under_pointer = app.screen_to_canvas(canvas, anchor);
        let before = panel_screen_rect(&app, &ctx, "notes");

        assert!(app.zoom_canvas_at(canvas, anchor, 1.4), "the zoom changes");
        let after = panel_screen_rect(&app, &ctx, "notes");
        let moved = app.canvas_to_screen(canvas, under_pointer);
        assert!(
            moved.distance(anchor) < 0.01,
            "the canvas point under the pointer keeps its place: {moved:?} against {anchor:?}"
        );
        assert!(
            (after.center().distance(before.center()) - 0.0).abs() > 1.0,
            "a zoom at the pointer moves the panel with the canvas: {before:?} then {after:?}"
        );
        assert!(
            (app.canvas_view.pan_offset[0] + 300.0).abs() > 1.0,
            "the pan of the zoom at the pointer is the pan of that pointer"
        );
    }

    /// A jump puts a panel clear of the sidebar, and the canvas of the reveal leaves
    /// the sidebar out of its rectangle whether it is open or closed.
    #[test]
    fn a_jump_keeps_the_panel_clear_of_an_open_sidebar() {
        let (_temp, ctx, mut app) = app_with_panel_at([40.0, 40.0]);
        app.minimap_visible = false;

        let mut left_edges = Vec::new();
        for visible in [true, false] {
            app.sidebar_visible = visible;
            let canvas = app.canvas_rect(&ctx);
            if visible {
                assert!(
                    canvas.min.x > 0.0,
                    "an open sidebar leaves the canvas to its right: {canvas:?}"
                );
            } else {
                assert!(
                    canvas.min.x.abs() < f32::EPSILON,
                    "a closed sidebar leaves the canvas at the edge: {canvas:?}"
                );
            }
            app.reveal_panel_visible(
                &ctx,
                app.board.panel_id_by_local_id("notes").expect("the fixture panel"),
            );
            let revealed = panel_screen_rect(&app, &ctx, "notes");
            let reveal = app.reveal_rect(&ctx);
            assert!(
                reveal.contains_rect(revealed),
                "the revealed panel is whole in the rectangle of the reveal: {revealed:?} in {reveal:?}"
            );
            assert!(
                revealed.min.x >= reveal.min.x + REVEAL_MARGIN - 0.5,
                "the revealed panel starts clear of the sidebar: {} against {}",
                revealed.min.x,
                reveal.min.x + REVEAL_MARGIN
            );
            left_edges.push(revealed.min.x);
        }
        assert!(
            left_edges[1] < left_edges[0],
            "the same jump stands further left without the sidebar: {left_edges:?}"
        );
    }

    /// The minimap covers a corner of the canvas, and the reveal keeps a panel out of
    /// that corner.
    #[test]
    fn a_jump_keeps_the_panel_clear_of_the_minimap() {
        let (_temp, ctx, mut app) = app_with_panel_at([40.0, 40.0]);
        app.sidebar_visible = false;
        app.minimap_visible = true;
        let canvas = app.canvas_rect(&ctx);
        let minimap = app.minimap_overlay_rect(&ctx).expect("the minimap is on the canvas");
        assert!(canvas.contains_rect(minimap), "the minimap stands over the canvas");
        let reveal = app.reveal_rect(&ctx);
        assert!(
            reveal.max.y <= minimap.min.y + 0.5 || reveal.max.x <= minimap.min.x + 0.5,
            "the rectangle of a jump leaves the corner of the minimap out: {reveal:?} against {minimap:?}"
        );

        app.board
            .focus(app.board.panel_id_by_local_id("notes").expect("the fixture panel"));
        app.reveal_panel_visible(
            &ctx,
            app.board.panel_id_by_local_id("notes").expect("the fixture panel"),
        );
        let revealed = panel_screen_rect(&app, &ctx, "notes");
        assert!(
            !revealed.intersects(minimap),
            "the revealed panel stays out of the minimap: {revealed:?} against {minimap:?}"
        );
        assert!(
            reveal.contains_rect(revealed),
            "the revealed panel is whole in the rectangle of the reveal: {revealed:?} in {reveal:?}"
        );
        let _ = canvas_scene_transform(canvas, app.canvas_view);
    }
}
