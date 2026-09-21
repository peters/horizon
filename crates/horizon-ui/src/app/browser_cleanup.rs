use super::HorizonApp;

impl HorizonApp {
    pub(super) fn close_ended_browser_panels(&mut self) -> bool {
        let ended: Vec<_> = self
            .board
            .ended_browser_panels()
            // The create result must retain its typed backend/provider failure.
            .filter(|id| !self.browser_create_is_pending(*id))
            .filter(|id| !self.browser_is_managed_cloud_member(*id))
            .collect();
        for &id in &ended {
            if let Some(browser) = self.board.panel(id).and_then(horizon_core::Panel::browser) {
                tracing::info!(panel_id = id.0, status = ?browser.status, "closing ended browser panel");
            }
            if self.fullscreen_panel == Some(id) {
                self.fullscreen_panel = None;
            }
            self.close_panel(id);
            self.panel_screen_rects.remove(&id);
            self.terminal_body_screen_rects.remove(&id);
        }
        if !ended.is_empty() {
            self.mark_runtime_dirty();
        }
        !ended.is_empty()
    }

    fn browser_is_managed_cloud_member(&self, id: horizon_core::PanelId) -> bool {
        #[cfg(feature = "cloud-workspaces")]
        if let Some(panel) = self.board.panel(id) {
            // Saved membership protects placeholders before cloud restoration runs.
            return self
                .cloud_prototype
                .groups
                .0
                .iter()
                .chain(&self.board.cloud_groups.0)
                .any(|group| group.remote.is_some() && group.panels.contains(&panel.local_id));
        }
        let _ = id;
        false
    }
}

#[cfg(test)]
mod tests;
