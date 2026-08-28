//! Synchronous `WebDriver` navigation commands and their immediate outcomes.

use std::time::{Duration, Instant};

use serde_json::json;

use crate::session::{BrowserEvent, BrowserEventSender};
use crate::{BackendKind, normalize_navigation_target};

use super::{Driver, NAVIGATION_HTTP_TIMEOUT, PendingHistoryStart, classic_navigation_committed};

impl Driver {
    pub(super) fn navigate(&mut self, url: &str, event_tx: &BrowserEventSender) -> Result<(), String> {
        let url = normalize_navigation_target(url);
        self.begin_navigation();
        let _ = event_tx.send(BrowserEvent::Loading(true));
        let result = if self.config.browser.backend == BackendKind::FirefoxBidi {
            self.call_bidi(
                "browsingContext.navigate",
                &json!({ "context": self.context_id, "url": &url, "wait": "none" }),
                event_tx,
            )
            .map(|_| ())
        } else {
            self.classic_navigation_post("url", &json!({ "url": &url }))
                .and_then(|_| self.classic_get("url"))
                .and_then(|response| {
                    classic_navigation_committed(&response, &url, &self.url)
                        .then_some(())
                        .ok_or_else(|| "browser did not commit a reachable URL".to_string())
                })
        };
        match result {
            Ok(()) => {
                if self.config.browser.backend != BackendKind::FirefoxBidi {
                    self.retain_frame_during_navigation = false;
                    self.navigation_failed = false;
                    self.refresh_page_state(event_tx);
                    self.frames.demand();
                }
                Ok(())
            }
            Err(error) => {
                self.navigation_failed = true;
                self.frames.interaction_started_at = None;
                let _ = event_tx.send(BrowserEvent::NavigationFailed(format!(
                    "could not navigate to {url}: {error}"
                )));
                let _ = event_tx.send(BrowserEvent::Loading(false));
                Err(error)
            }
        }
    }

    pub(super) fn reload(&mut self, event_tx: &BrowserEventSender) -> Result<(), String> {
        self.begin_navigation();
        let result = if self.config.browser.backend == BackendKind::FirefoxBidi {
            self.call_bidi(
                "browsingContext.reload",
                &json!({ "context": self.context_id, "wait": "none" }),
                event_tx,
            )
            .map(|_| ())
        } else {
            self.classic_navigation_post("refresh", &json!({})).map(|_| ())
        };
        self.finish_page(result, "reload", event_tx)
    }

    pub(super) fn traverse(&mut self, delta: i64, event_tx: &BrowserEventSender) -> Result<(), String> {
        self.begin_navigation();
        // Firefox's BiDi traversal can return without a completion event, so
        // use its blocking classic endpoint and suppress the matching late
        // navigationStarted event that would otherwise cancel this capture.
        let result = self
            .classic_navigation_post(if delta < 0 { "back" } else { "forward" }, &json!({}))
            .map(|_| ());
        if result.is_ok() && self.config.browser.backend == BackendKind::FirefoxBidi {
            self.retain_frame_during_navigation = false;
            self.navigation_failed = false;
            self.refresh_page_state(event_tx);
            self.pending_classic_history_start = Some(PendingHistoryStart {
                url: self.url.clone(),
                expires_at: Instant::now() + Duration::from_secs(1),
            });
            self.frames.demand();
        }
        self.finish_page(result, "history traversal", event_tx)
    }

    fn finish_page(
        &mut self,
        result: Result<(), String>,
        name: &str,
        events: &BrowserEventSender,
    ) -> Result<(), String> {
        if let Err(error) = result {
            self.navigation_failed = true;
            self.frames.interaction_started_at = None;
            let _ = events.send(BrowserEvent::NavigationFailed(format!("{name} failed: {error}")));
            let _ = events.send(BrowserEvent::Loading(false));
            return Err(error);
        }
        if self.config.browser.backend != BackendKind::FirefoxBidi {
            self.retain_frame_during_navigation = false;
            self.navigation_failed = false;
            self.refresh_page_state(events);
            self.frames.demand();
        }
        Ok(())
    }

    fn classic_navigation_post(&self, suffix: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.service
            .http
            .post_with_read_timeout(&self.session_path(suffix), body, NAVIGATION_HTTP_TIMEOUT)
            .map_err(|error| error.to_string())
    }
}
