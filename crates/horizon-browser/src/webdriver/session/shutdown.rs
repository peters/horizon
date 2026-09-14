//! Ordered `WebDriver` session and capture teardown.

use std::sync::mpsc;
use std::time::Instant;

use crate::RemoteSessionEvent;

use serde_json::Value;

use crate::process::ChromeProcessControl;
use crate::session::BrowserEvent;
use crate::session::BrowserEventSender;

use super::Driver;

pub(super) struct Completion {
    tx: Option<mpsc::Sender<()>>,
    process: ChromeProcessControl,
}

impl Completion {
    pub(super) fn new(tx: mpsc::Sender<()>, process: ChromeProcessControl) -> Self {
        Self { tx: Some(tx), process }
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        self.process.mark_registration_settled();
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Driver {
    pub(super) fn stop_for_service_exit(&mut self, event_tx: &BrowserEventSender) -> bool {
        let now = Instant::now();
        let Some(exit) = self.host.exit(now) else {
            return false;
        };
        self.settle_pending_wait_for_shutdown(now);
        if let super::super::host::HostExit::Expired(reason) = exit {
            let label = self
                .host
                .remote()
                .map(|host| host.label().to_string())
                .unwrap_or_default();
            let _ = event_tx.send(BrowserEvent::RemoteSession(RemoteSessionEvent::Expired {
                label,
                reason,
            }));
            self.close(event_tx);
        }
        let _ = event_tx.send(BrowserEvent::Stopped { code: exit.code() });
        true
    }

    pub(super) fn close(&mut self, event_tx: &BrowserEventSender) {
        if self.network.is_active() {
            // Flush the explicit export before the page and its BiDi channel
            // disappear. Shutdown still proceeds if optional cleanup fails.
            self.flush_firefox_http_response_bodies(event_tx);
            self.remove_firefox_network_bridge(event_tx);
            let _ = self.network.stop();
        }
        let _ = self.classic_delete("actions");
        if let Some(outcome) = self.host.release(&self.session_id) {
            let label = self
                .host
                .remote()
                .map(|host| host.label().to_string())
                .unwrap_or_default();
            let _ = event_tx.send(BrowserEvent::RemoteSession(RemoteSessionEvent::Released {
                label,
                outcome,
            }));
        }
    }

    pub(super) fn classic_delete(&self, suffix: &str) -> Result<Value, String> {
        self.host
            .transport()
            .delete(&self.session_path(suffix))
            .map_err(|error| error.to_string())
    }
}
