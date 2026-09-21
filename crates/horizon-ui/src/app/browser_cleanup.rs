use super::HorizonApp;

impl HorizonApp {
    pub(super) fn close_ended_browser_panels(&mut self) -> bool {
        let ended: Vec<_> = self
            .board
            .ended_browser_panels()
            // The create result must retain its typed backend/provider failure.
            .filter(|id| !self.browser_create_is_pending(*id))
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
}

#[cfg(test)]
mod tests;
