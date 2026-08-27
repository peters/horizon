//! Ordered `WebDriver` session and capture teardown.

use serde_json::Value;

use crate::session::BrowserEventSender;

use super::Driver;

impl Driver {
    pub(super) fn close(&mut self, event_tx: &BrowserEventSender) {
        if self.network.is_active() {
            // Flush the explicit export before the page and its BiDi channel
            // disappear. Shutdown still proceeds if optional cleanup fails.
            self.remove_firefox_network_bridge(event_tx);
            let _ = self.network.stop();
        }
        let _ = self.classic_delete("actions");
        let _ = self.service.http.delete(&format!("/session/{}", self.session_id));
        let _ = self.service.process.kill();
    }

    pub(super) fn classic_delete(&self, suffix: &str) -> Result<Value, String> {
        self.service
            .http
            .delete(&self.session_path(suffix))
            .map_err(|error| error.to_string())
    }
}
