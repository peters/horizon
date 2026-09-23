//! Render-time visibility and end-of-pass navigation context; never changes the view.
use super::HorizonApp;
use egui::{Context, Rect};
use horizon_core::{
    Panel, PanelId, WorkspaceId,
    browser::manifest::device::{HostCanvas, HostExclusion, HostViewport},
};

impl HorizonApp {
    pub(super) fn record_applied_device_reveal(&mut self, id: PanelId, canvas: Rect) {
        let Some(panel) = self.board.panel(id) else { return };
        let viewport = if self.workspace_is_detached(panel.workspace_id) {
            HostViewport::Detached
        } else {
            HostViewport::Root
        };
        let canvas = self.device_host_canvas(canvas);
        if let Some(state) = self.panel_render_caches.device_ui_state.get_mut(&id) {
            state.host.applied(viewport, canvas);
        }
    }

    pub(super) fn capture_root_device_presentation(&mut self, canvas: Rect) {
        for panel in &self.board.panels {
            if panel.device().is_none() || self.workspace_is_detached(panel.workspace_id) {
                continue;
            }
            let exclusion = self.device_render_exclusion(panel, canvas, HostViewport::Root);
            let observation = self.device_host_canvas(canvas);
            if let Some(state) = self.panel_render_caches.device_ui_state.get_mut(&panel.id) {
                state.host.record(HostViewport::Root, Some(observation), exclusion);
            }
        }
    }

    pub(super) fn record_root_device_presentation(&mut self, ctx: &Context) {
        let after = self.device_host_canvas(self.canvas_rect(ctx));
        for panel in &self.board.panels {
            if panel.device().is_none() {
                continue;
            }
            let detached = self.workspace_is_detached(panel.workspace_id);
            let fallback = if detached {
                HostExclusion::DetachedViewportNotRendered
            } else if self.shutdown_progress.is_some()
                || self.pending_session_switch.is_some()
                || self.startup_receiver.is_some()
                || self.startup_bootstrap_failure.is_some()
                || self.startup_chooser.is_some()
            {
                HostExclusion::HostOverlay
            } else if !panel.visible {
                HostExclusion::Hidden
            } else if self.fullscreen_panel.is_some_and(|id| id != panel.id) {
                HostExclusion::OtherPanelFullscreen
            } else {
                HostExclusion::Unclassified
            };
            let Some(state) = self.panel_render_caches.device_ui_state.get_mut(&panel.id) else {
                continue;
            };
            if detached && state.host.observed_this_frame() {
                continue;
            }
            let viewport = if detached {
                HostViewport::Detached
            } else {
                HostViewport::Root
            };
            if !state.host.observed_this_frame() {
                // No render hook ran: do not report root or stale geometry as a
                // detached/fullscreen/overlay rendering camera.
                state.host.record(viewport, None, Some(fallback));
            }
            state.host.finish(
                (!detached).then_some(after),
                state.was_rendered(),
                ctx.cumulative_pass_nr(),
                ctx.will_discard(),
            );
        }
    }

    pub(super) fn record_detached_device_presentation(&mut self, workspace: WorkspaceId, canvas: Rect) {
        for panel in &self.board.panels {
            if panel.workspace_id != workspace || panel.device().is_none() {
                continue;
            }
            let exclusion = self.device_render_exclusion(panel, canvas, HostViewport::Detached);
            let observation = self.device_host_canvas(canvas);
            if let Some(state) = self.panel_render_caches.device_ui_state.get_mut(&panel.id) {
                state.host.record(HostViewport::Detached, Some(observation), exclusion);
            }
        }
    }

    pub(super) fn finish_detached_device_presentation(&mut self, ctx: &Context, workspace: WorkspaceId, canvas: Rect) {
        let after = self.device_host_canvas(canvas);
        for panel in &self.board.panels {
            if panel.workspace_id != workspace || panel.device().is_none() {
                continue;
            }
            if let Some(state) = self.panel_render_caches.device_ui_state.get_mut(&panel.id) {
                state.host.finish(
                    Some(after),
                    state.was_rendered(),
                    ctx.cumulative_pass_nr(),
                    ctx.will_discard(),
                );
            }
        }
    }

    fn device_host_canvas(&self, canvas: Rect) -> HostCanvas {
        HostCanvas {
            pan_offset: self.canvas_view.pan_offset,
            zoom: self.canvas_view.zoom,
            rect: [canvas.left(), canvas.top(), canvas.right(), canvas.bottom()],
        }
    }

    fn device_render_exclusion(&self, panel: &Panel, canvas: Rect, viewport: HostViewport) -> Option<HostExclusion> {
        if !panel.visible {
            return Some(HostExclusion::Hidden);
        }
        if viewport == HostViewport::Root {
            #[cfg(feature = "cloud-workspaces")]
            if !self.cloud_panel_is_in_view(&panel.local_id) {
                return Some(HostExclusion::OtherCloudFullscreen);
            }
        }
        self.panel_screen_geometry(panel, canvas)
            .is_none()
            .then_some(HostExclusion::OutsideCanvas)
    }
}
