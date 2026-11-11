//! The browser panels that ended, and the reason that the person reads.
//!
//! A browser panel whose engine cannot start ends at once, and this cleanup
//! takes the panel away. A local panel whose engine stops later ends the same
//! way. The panel of a browser host is not one of these: it keeps the page's
//! place and says its own reason, because the person can retry it. The person
//! who pressed the link or the picker gets no chance to read a panel that goes,
//! so its reason goes to the sidebar first, and to the card whose link opened
//! it.

use horizon_core::PanelKind;
use horizon_core::browser::BrowserStatus;

use super::HorizonApp;

impl HorizonApp {
    pub(super) fn close_ended_browser_panels(&mut self) -> bool {
        let ended: Vec<_> = self
            .board
            .ended_browser_panels()
            // The create result must retain its typed backend/provider failure.
            .filter(|id| !self.browser_create_is_pending(*id))
            .collect();
        if ended.is_empty() {
            self.drop_forgotten_browser_creates();
            return false;
        }
        for &id in &ended {
            let status = self
                .board
                .panel(id)
                .and_then(horizon_core::Panel::browser)
                .map(|b| b.status.clone());
            if let Some(status) = &status {
                tracing::info!(panel_id = id.0, status = ?status, "closing ended browser panel");
                self.say_why_a_browser_panel_ended(id, status);
            }
            if self.fullscreen_panel == Some(id) {
                self.fullscreen_panel = None;
            }
            self.close_panel(id);
            self.panel_screen_rects.remove(&id);
            self.terminal_body_screen_rects.remove(&id);
        }
        self.drop_forgotten_browser_creates();
        self.mark_runtime_dirty();
        true
    }

    /// Says why a browser panel that a person's action opened ended, before the
    /// cleanup takes it away.
    ///
    /// An agent's create answers the agent with its own typed failure, so it is
    /// not one of these; and one panel says one reason, so a panel that the
    /// person opened once cannot repeat its notice.
    fn say_why_a_browser_panel_ended(&mut self, panel_id: horizon_core::PanelId, status: &BrowserStatus) {
        let Some(ended) = self.person_browser_creates.take(panel_id) else {
            return;
        };
        let Some(reason) = super::creation_notice::engine_failure_reason(status) else {
            return;
        };
        self.set_sidebar_notice(super::creation_notice::panel_creation_notice(
            PanelKind::Browser,
            &reason,
        ));
        if let Some(link) = ended.link {
            self.card_link_failed(link.card, &link.url, &reason);
        }
    }

    /// Drops the records of browser panels that are gone, so the list holds no
    /// panel that a close or a restart already removed.
    fn drop_forgotten_browser_creates(&mut self) {
        if self.person_browser_creates.is_empty() {
            return;
        }
        let open: Vec<horizon_core::PanelId> = self.board.panels.iter().map(|panel| panel.id).collect();
        self.person_browser_creates.retain(|panel| open.contains(&panel));
    }
}

#[cfg(test)]
mod tests;
