use std::cmp::Ordering;
use std::collections::HashMap;

use egui::containers::panel::PanelState;
use egui::{Context, Id, Pos2, Rect, Vec2};
use horizon_core::{PanelId, WorkspaceId};

use crate::app::attention_feed::estimated_outer_rect;
use crate::app::root_chrome::effective_sidebar_width;
use crate::app::settings::{SETTINGS_BAR_HEIGHT, SETTINGS_BAR_ID, SETTINGS_PANEL_ID, settings_panel_default_width};
use crate::app::util::{OverlayExclusion, viewport_local_rect};
use crate::app::{HorizonApp, MINIMAP_MARGIN, MINIMAP_PAD, TOOLBAR_HEIGHT};

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
        let settings_panel_rect = self.settings_panel_rect(ctx, viewport);
        let settings_bar_rect = self.settings_bar_rect(ctx, viewport);
        let sidebar_width = if self.sidebar_visible {
            effective_sidebar_width(viewport.width())
        } else {
            0.0
        };
        canvas_rect_for_layout(viewport, sidebar_width, settings_panel_rect, settings_bar_rect)
    }

    pub(in crate::app) fn fixed_overlays_visible(&self) -> bool {
        self.settings.is_none()
    }

    fn settings_panel_rect(&self, ctx: &Context, viewport: Rect) -> Option<Rect> {
        estimated_settings_panel_rect(
            viewport,
            self.settings.is_some(),
            PanelState::load(ctx, Id::new(SETTINGS_PANEL_ID)).map(|state| state.rect),
        )
    }

    fn settings_bar_rect(&self, ctx: &Context, viewport: Rect) -> Option<Rect> {
        estimated_settings_bar_rect(
            viewport,
            self.settings.is_some(),
            PanelState::load(ctx, Id::new(SETTINGS_BAR_ID)).map(|state| state.rect),
        )
    }

    /// Screen-space rectangles occupied by fixed overlay widgets. Compute this
    /// once per frame and pass to rendering code that positions canvas-space
    /// elements (e.g. workspace labels) so they stay clear.
    pub(in crate::app) fn overlay_exclusion_zones(&self, ctx: &Context) -> OverlayExclusion {
        let viewport = viewport_local_rect(ctx);
        let mut zones = Vec::new();
        let sidebar_width = if self.sidebar_visible {
            effective_sidebar_width(viewport.width())
        } else {
            0.0
        };

        if sidebar_width > 0.0 {
            zones.push(Rect::from_min_max(
                Pos2::new(viewport.min.x, viewport.min.y + TOOLBAR_HEIGHT),
                Pos2::new(viewport.min.x + sidebar_width, viewport.max.y),
            ));
        }

        if let Some(rect) = self.settings_panel_rect(ctx, viewport) {
            zones.push(rect);
        }
        if let Some(rect) = self.settings_bar_rect(ctx, viewport) {
            zones.push(rect);
        }

        let minimap_height =
            if self.fixed_overlays_visible() && self.minimap_visible && !self.board.workspaces.is_empty() {
                let overlays = &self.template_config.overlays;
                let width = overlays.minimap_width.max(120.0) + MINIMAP_PAD * 2.0;
                let height = overlays.minimap_height.max(120.0) + MINIMAP_PAD * 2.0;
                zones.push(Rect::from_min_size(
                    Pos2::new(
                        viewport.max.x - MINIMAP_MARGIN - width,
                        viewport.max.y - MINIMAP_MARGIN - height,
                    ),
                    Vec2::new(width, height),
                ));
                height
            } else {
                0.0
            };

        if self.fixed_overlays_visible()
            && self.template_config.features.attention_feed
            && let Some(rect) =
                estimated_outer_rect(viewport, minimap_height, &self.template_config.overlays, &self.board)
        {
            zones.push(rect);
        }

        OverlayExclusion::new(zones)
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

pub(super) fn canvas_rect_for_layout(
    viewport: Rect,
    sidebar_width: f32,
    settings_panel_rect: Option<Rect>,
    settings_bar_rect: Option<Rect>,
) -> Rect {
    let left = viewport.min.x + sidebar_width;
    let right = settings_panel_rect.map_or(viewport.max.x, |rect| rect.min.x);
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
