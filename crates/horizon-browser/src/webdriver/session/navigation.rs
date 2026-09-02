//! Synchronous `WebDriver` navigation commands and their immediate outcomes.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::navigation::{AgentActionExecution, NavigationSignal, PendingNavigation};
use crate::session::{BrowserEvent, BrowserEventSender};
use crate::{
    AgentAction, BackendKind, BrowserControlFailure, DEFAULT_NAVIGATION_TIMEOUT_MILLIS, NavigationWait,
    normalize_navigation_target,
};

use super::{
    Driver, NAVIGATION_HTTP_TIMEOUT, PendingHistoryStart, classic_navigation_committed, normalize_url, webdriver_value,
};

impl Driver {
    /// Run a `Navigate` agent action. Classic `WebDriver` blocks until the
    /// document committed and loaded, so it settles at once; Firefox `BiDi`
    /// dispatches with `wait: "none"` and settles from its navigation events
    /// or the bounded deadline.
    pub(super) fn navigate_action(
        &mut self,
        request: &AgentAction,
        event_tx: &BrowserEventSender,
    ) -> AgentActionExecution {
        let crate::BrowserControlAction::Navigate {
            url,
            wait,
            timeout_millis,
        } = &request.action
        else {
            return AgentActionExecution::Done(Err(BrowserControlFailure::new(
                "invalid_action_state",
                "navigation was requested for a non-navigation action",
            )));
        };
        let (wait, timeout_millis) = (*wait, *timeout_millis);
        let now = Instant::now();
        self.supersede_pending_navigation(now);
        let dispatch = if self.config.browser.backend == BackendKind::FirefoxBidi {
            self.dispatch_bidi_navigate(url, event_tx)
        } else {
            self.navigate(url, event_tx)
        };
        tracing::debug!(
            target: "browser",
            action_id = %request.action_id,
            ?wait,
            dispatch_millis = u64::try_from(now.elapsed().as_millis()).unwrap_or(u64::MAX),
            ok = dispatch.is_ok(),
            "agent navigation dispatched"
        );
        if let Err(error) = dispatch {
            return AgentActionExecution::Done(Err(BrowserControlFailure::new("navigation_failed", error)));
        }
        let mut pending = PendingNavigation::new(
            request.clone(),
            normalize_navigation_target(url),
            wait,
            Duration::from_millis(timeout_millis.unwrap_or(DEFAULT_NAVIGATION_TIMEOUT_MILLIS)),
            PendingNavigation::queued_for(request, crate::navigation::now_millis()),
            now,
        );
        if self.config.browser.backend != BackendKind::FirefoxBidi {
            let (committed, title) = (self.url.clone(), self.title.clone());
            return AgentActionExecution::Done(Ok(pending.settle_loaded(&committed, &title, Instant::now())));
        }
        if wait == NavigationWait::Dispatched {
            return AgentActionExecution::Done(Ok(pending.dispatched(Instant::now())));
        }
        self.pending_navigation = Some(pending);
        AgentActionExecution::Pending
    }

    /// Send `browsingContext.navigate` without waiting for its reply. Firefox
    /// answers the command only once the destination starts responding, so a
    /// blocking call would stall the driver behind a slow server; the reply is
    /// routed to `handle_bidi_navigate_response` by id.
    fn dispatch_bidi_navigate(&mut self, url: &str, event_tx: &BrowserEventSender) -> Result<(), String> {
        let url = normalize_navigation_target(url);
        self.begin_navigation();
        let _ = event_tx.send(BrowserEvent::Loading(true));
        let context = self.context_id.clone();
        let sent = self
            .bidi
            .as_mut()
            .ok_or_else(|| "BiDi is unavailable".to_string())
            .and_then(|link| {
                link.send_request(
                    "browsingContext.navigate",
                    &json!({ "context": context, "url": &url, "wait": "none" }),
                )
                .map_err(|error| error.to_string())
            });
        match sent {
            Ok(request_id) => {
                self.navigate_request_id = Some(request_id);
                Ok(())
            }
            Err(error) => {
                self.fail_agent_navigation(event_tx, &format!("could not navigate to {url}: {error}"));
                Err(error)
            }
        }
    }

    /// Consume the asynchronous `browsingContext.navigate` reply: an error
    /// reply is a failed navigation.
    pub(super) fn handle_bidi_navigate_response(&mut self, response: &Value, event_tx: &BrowserEventSender) {
        self.navigate_request_id = None;
        let is_error = response.get("type").and_then(Value::as_str) == Some("error") || response.get("error").is_some();
        if is_error {
            let message = response
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("the browser rejected the navigation");
            self.fail_agent_navigation(event_tx, &format!("could not navigate: {message}"));
        }
    }

    fn fail_agent_navigation(&mut self, event_tx: &BrowserEventSender, message: &str) {
        self.navigation_failed = true;
        self.frames.interaction_started_at = None;
        let _ = event_tx.send(BrowserEvent::NavigationFailed(message.to_string()));
        let _ = event_tx.send(BrowserEvent::Loading(false));
        self.observe_navigation_signal(NavigationSignal::Failed(message));
    }

    /// Map a `BiDi` navigation-complete event onto the pending navigation.
    pub(super) fn settle_navigation_from_bidi(&mut self, method: &str) {
        let url = self.url.clone();
        tracing::debug!(target: "browser", method, pending = self.pending_navigation.is_some(), "bidi navigation event");
        if method.ends_with("fragmentNavigated") {
            self.observe_navigation_signal(NavigationSignal::SameDocument(&url));
            return;
        }
        self.observe_navigation_signal(NavigationSignal::Committed(&url));
        if method == "browsingContext.load" {
            self.observe_navigation_signal(NavigationSignal::Load);
        } else {
            self.observe_navigation_signal(NavigationSignal::DomContentLoaded);
        }
    }

    pub(super) fn observe_navigation_signal(&mut self, signal: NavigationSignal<'_>) {
        let now = Instant::now();
        let settled = self
            .pending_navigation
            .as_mut()
            .and_then(|pending| pending.observe(signal, now));
        if let Some(result) = settled
            && let Some(pending) = self.pending_navigation.take()
        {
            tracing::debug!(target: "browser", action_id = %pending.request.action_id, ?signal, "agent navigation settled");
            self.complete_agent_action(&pending.request, result);
        }
    }

    pub(super) fn tick_pending_navigation(&mut self) {
        let now = Instant::now();
        let settled = self.pending_navigation.as_ref().and_then(|pending| pending.tick(now));
        if let Some(result) = settled
            && let Some(pending) = self.pending_navigation.take()
        {
            self.complete_agent_action(&pending.request, result);
        }
    }

    fn supersede_pending_navigation(&mut self, now: Instant) {
        if let Some(pending) = self.pending_navigation.take() {
            self.complete_agent_action(&pending.request, Ok(pending.superseded(now)));
        }
    }

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

    pub(super) fn refresh_page_state(&mut self, event_tx: &BrowserEventSender) {
        if let Ok(response) = self.classic_get("url")
            && let Some(url) = webdriver_value(&response).and_then(Value::as_str)
            && url != self.url
        {
            self.url = normalize_url(url).to_string();
            self.coordination_dirty = true;
            let _ = event_tx.send(BrowserEvent::UrlChanged(self.url.clone()));
        }
        if let Ok(response) = self.classic_get("title")
            && let Some(title) = webdriver_value(&response).and_then(Value::as_str)
            && title != self.title
        {
            self.title = title.to_string();
            self.coordination_dirty = true;
            let _ = event_tx.send(BrowserEvent::Title(self.title.clone()));
            let title = self.title.clone();
            self.observe_navigation_signal(crate::navigation::NavigationSignal::Title(&title));
        }
        let _ = event_tx.send(BrowserEvent::Loading(false));
    }

    pub(super) fn tick_page_state_refresh(&mut self, event_tx: &BrowserEventSender) {
        let Some(refresh_at) = self.refresh_pending_at else {
            return;
        };
        if Instant::now() < refresh_at {
            return;
        }
        self.refresh_pending_at = None;
        self.refresh_page_state(event_tx);
    }
}
