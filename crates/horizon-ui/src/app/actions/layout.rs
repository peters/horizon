use std::cmp::Ordering;
use std::collections::HashMap;

use egui::containers::panel::PanelState;
use egui::{Context, Id, Pos2, Rect, Vec2};
use horizon_core::{PanelId, WorkspaceId};

use crate::app::orchestrator_dock::leftmost_panel;
use crate::app::settings::{SETTINGS_BAR_HEIGHT, SETTINGS_BAR_ID, settings_panel_default_width};
use crate::app::util::{OverlayExclusion, viewport_local_rect};
use crate::app::{HorizonApp, MINIMAP_MARGIN, MINIMAP_PAD, SIDEBAR_WIDTH, TOOLBAR_HEIGHT};

pub(super) fn panel_focus_target_at_pointer_press(
    panel_order: &[PanelId],
    panel_rects: &HashMap<PanelId, Rect>,
    focused_panel: Option<PanelId>,
    pointer_pos: Pos2,
) -> Option<PanelId> {
    if focused_panel.is_some_and(|panel_id| {
        panel_rects
            .get(&panel_id)
            .is_some_and(|rect| rect.contains(pointer_pos))
    }) {
        return focused_panel;
    }

    panel_order
        .iter()
        .rev()
        .copied()
        .find(|panel_id| panel_rects.get(panel_id).is_some_and(|rect| rect.contains(pointer_pos)))
}

impl HorizonApp {
    pub(in crate::app) fn leftmost_workspace_id(&self) -> Option<WorkspaceId> {
        self.board
            .workspaces
            .iter()
            .filter(|workspace| !self.workspace_is_detached(workspace.id))
            .min_by(|left, right| {
                left.position[0]
                    .partial_cmp(&right.position[0])
                    .unwrap_or(Ordering::Equal)
            })
            .map(|workspace| workspace.id)
    }

    pub(in crate::app) fn canvas_rect(&self, ctx: &Context) -> Rect {
        let viewport = viewport_local_rect(ctx);
        let dock_rect = self.orchestrator_dock.frame_rect();
        let settings_panel_rect = self.settings_panel_rect_for_layout(viewport, dock_rect);
        let settings_bar_rect = self.settings_bar_rect(ctx, viewport);
        let sidebar_width = if self.sidebar_visible { SIDEBAR_WIDTH } else { 0.0 };
        canvas_rect_for_layout(
            viewport,
            sidebar_width,
            settings_panel_rect,
            settings_bar_rect,
            dock_rect,
        )
    }

    pub(in crate::app) fn fixed_overlays_visible(&self) -> bool {
        self.settings.is_none()
    }

    fn settings_panel_rect_for_layout(&self, viewport: Rect, dock_rect: Option<Rect>) -> Option<Rect> {
        // The rectangle that the panel took this frame comes from the panel
        // itself: a rect that egui remembered of the panel is not that rect,
        // because egui moves the left edge of a panel whose content overflows.
        let taken = self.settings.as_ref().and_then(|editor| editor.panel_rect);
        // The dock keeps the right edge of the window, so the estimate of a frame
        // before the panel drew its own rectangle names the room left of the dock.
        let viewport = match (taken, dock_rect) {
            (None, Some(dock)) => Rect::from_min_max(viewport.min, Pos2::new(dock.min.x, viewport.max.y)),
            _ => viewport,
        };
        estimated_settings_panel_rect(viewport, self.settings.is_some(), taken)
    }

    fn settings_bar_rect(&self, ctx: &Context, viewport: Rect) -> Option<Rect> {
        estimated_settings_bar_rect(
            viewport,
            self.settings.is_some(),
            PanelState::load(ctx, Id::unique(SETTINGS_BAR_ID)).map(|state| state.outer_rect),
        )
    }

    /// Screen-space rectangles occupied by fixed overlay widgets. Compute this
    /// once per frame and pass to rendering code that positions canvas-space
    /// elements (e.g. workspace labels) so they stay clear.
    pub(in crate::app) fn overlay_exclusion_zones(&self, ctx: &Context) -> OverlayExclusion {
        let viewport = viewport_local_rect(ctx);
        let mut zones: Vec<_> = self.preset_picker_rect(ctx).into_iter().collect();
        let sidebar_width = if self.sidebar_visible { SIDEBAR_WIDTH } else { 0.0 };

        if sidebar_width > 0.0 {
            zones.push(Rect::from_min_max(
                Pos2::new(viewport.min.x, viewport.min.y + TOOLBAR_HEIGHT),
                Pos2::new(viewport.min.x + sidebar_width, viewport.max.y),
            ));
        }

        if let Some(rect) = self.settings_panel_rect_for_layout(viewport, self.orchestrator_dock.frame_rect()) {
            zones.push(rect);
        }
        if let Some(rect) = self.settings_bar_rect(ctx, viewport) {
            zones.push(rect);
        }
        // The dock stands over the canvas as a panel does, so nothing of the canvas
        // goes under it.
        if let Some(rect) = self.orchestrator_dock.frame_rect() {
            zones.push(rect);
        }

        if let Some(rect) = self.minimap_overlay_rect(ctx) {
            zones.push(rect);
        }

        if let Some(rect) = self.work_resume_overlay_rect(ctx) {
            zones.push(rect);
        }
        OverlayExclusion::new(zones)
    }

    pub(in crate::app) fn minimap_overlay_rect(&self, ctx: &Context) -> Option<Rect> {
        if !self.fixed_overlays_visible() || !self.minimap_visible || !self.any_attached_workspace() {
            return None;
        }
        let canvas = self.canvas_rect(ctx);
        let overlays = &self.template_config.overlays;
        let size = Vec2::new(overlays.minimap_width.max(120.0), overlays.minimap_height.max(120.0))
            + Vec2::splat(MINIMAP_PAD * 2.0);
        // The minimap hangs on the bottom right corner of the canvas, so the dock
        // and an open Settings panel never stand on it.
        crate::app::minimap::minimap_fits(canvas, size)
            .then(|| Rect::from_min_size(canvas.max - Vec2::splat(MINIMAP_MARGIN) - size, size))
    }

    pub(in crate::app) fn sync_panel_focus_from_pointer_press(&mut self, ctx: &Context) {
        let Some(pointer_pos) = ctx.input(|input| {
            input.events.iter().rev().find_map(|event| match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                } => Some(*pos),
                _ => None,
            })
        }) else {
            return;
        };

        if self.overlay_exclusion_zones(ctx).contains(pointer_pos) {
            return;
        }

        let panel_geometry = self.visible_panel_geometry_for_canvas_view(self.canvas_rect(ctx), None);
        let panel_order: Vec<_> = panel_geometry.iter().map(|(panel_id, _)| *panel_id).collect();
        let panel_rects: HashMap<_, _> = panel_geometry
            .into_iter()
            .map(|(panel_id, geometry)| (panel_id, geometry.screen_rect))
            .collect();

        if let Some(panel_id) =
            panel_focus_target_at_pointer_press(&panel_order, &panel_rects, self.board.focused, pointer_pos)
        {
            self.board.focus(panel_id);
        }
    }
}

pub(in crate::app) fn canvas_rect_for_layout(
    viewport: Rect,
    sidebar_width: f32,
    settings_panel_rect: Option<Rect>,
    settings_bar_rect: Option<Rect>,
    dock_rect: Option<Rect>,
) -> Rect {
    let left = viewport.min.x + sidebar_width;
    let right = leftmost_panel([settings_panel_rect, dock_rect]).map_or(viewport.max.x, |rect| rect.min.x);
    let bottom = settings_bar_rect.map_or(viewport.max.y, |rect| rect.min.y);

    Rect::from_min_max(
        Pos2::new(left, viewport.min.y + TOOLBAR_HEIGHT),
        Pos2::new(right, bottom),
    )
}

pub(super) fn estimated_settings_panel_rect(
    viewport: Rect,
    settings_open: bool,
    remembered_rect: Option<Rect>,
) -> Option<Rect> {
    if !settings_open {
        return None;
    }

    remembered_rect.or_else(|| {
        let width = settings_panel_default_width(viewport.width());
        Some(Rect::from_min_max(
            Pos2::new(viewport.max.x - width, viewport.min.y + TOOLBAR_HEIGHT),
            Pos2::new(viewport.max.x, viewport.max.y - SETTINGS_BAR_HEIGHT),
        ))
    })
}

pub(super) fn estimated_settings_bar_rect(
    viewport: Rect,
    settings_open: bool,
    remembered_rect: Option<Rect>,
) -> Option<Rect> {
    if !settings_open {
        return None;
    }

    remembered_rect.or_else(|| {
        Some(Rect::from_min_max(
            Pos2::new(viewport.min.x, viewport.max.y - SETTINGS_BAR_HEIGHT),
            viewport.max,
        ))
    })
}
