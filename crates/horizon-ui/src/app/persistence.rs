use std::time::{Duration, Instant};

use horizon_core::{DetachedWorkspaceState, RuntimeState};

use super::HorizonApp;

impl HorizonApp {
    pub(super) fn mark_runtime_dirty(&mut self) {
        self.runtime_dirty_since.get_or_insert_with(Instant::now);
    }

    pub(super) fn flush_runtime_if_dirty(&mut self) {
        const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
        if let Some(since) = self.runtime_dirty_since
            && since.elapsed() >= SAVE_DEBOUNCE
        {
            self.runtime_dirty_since = None;
            self.auto_save_runtime_state();
        }
    }

    pub(super) fn auto_save_runtime_state(&self) {
        let Some(active_session) = self.active_session.as_ref().filter(|session| session.persistent) else {
            return;
        };

        let detached_workspaces = self
            .detached_workspaces
            .iter()
            .filter(|(workspace_local_id, _)| !self.pending_detached_reattach.contains(*workspace_local_id))
            .map(|(workspace_local_id, window)| DetachedWorkspaceState {
                workspace_local_id: workspace_local_id.clone(),
                window: window.clone(),
            })
            .collect();

        let runtime_state = RuntimeState::from_board_with_detached_workspaces(
            &self.board,
            self.window_config.clone(),
            self.canvas_view,
            detached_workspaces,
        );
        if let Err(error) = self
            .session_store
            .save_runtime_state(&active_session.session_id, &runtime_state)
        {
            tracing::error!("failed to auto-save runtime state: {error}");
        }
    }

    pub(super) fn sync_window_config(&mut self, ctx: &egui::Context) {
        ctx.input(|input| {
            if let Some(rect) = input.viewport().inner_rect {
                let new_w = rect.width();
                let new_h = rect.height();
                if (new_w - self.window_config.width).abs() > 1.0 || (new_h - self.window_config.height).abs() > 1.0 {
                    self.window_config.width = new_w;
                    self.window_config.height = new_h;
                    self.mark_runtime_dirty();
                }
            }
            if let Some(pos) = input.viewport().outer_rect {
                let new_x = pos.min.x;
                let new_y = pos.min.y;
                let changed = self.window_config.x.is_none()
                    || self.window_config.x.is_some_and(|x| (x - new_x).abs() > 1.0)
                    || self.window_config.y.is_none()
                    || self.window_config.y.is_some_and(|y| (y - new_y).abs() > 1.0);
                if changed {
                    self.window_config.x = Some(new_x);
                    self.window_config.y = Some(new_y);
                    self.mark_runtime_dirty();
                }
            }
        });
    }
}
