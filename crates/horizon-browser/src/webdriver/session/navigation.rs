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
    Driver, NAVIGATION_HTTP_TIMEOUT, PAGE_LOAD_TIMEOUT_MILLIS, PendingHistoryStart, classic_navigation_committed,
    normalize_url, webdriver_value,
};

/// How much longer than a classic navigation's bound its HTTP read waits for
/// the `WebDriver` timeout error: enough for a normal reply, but well inside
/// the MCP controller's 5 s delivery headroom so a hung driver still ends in
/// the typed `timed_out` outcome after the read gives up.
const CLASSIC_BOUND_READ_MARGIN_MILLIS: u64 = 2_000;

impl Driver {
    /// Run a `Navigate` agent action. Firefox `BiDi` dispatches with
    /// `wait: "none"` and settles from its navigation events or the bounded
    /// deadline. Classic `WebDriver` has no dispatch-only primitive: its
    /// navigation command blocks until the document loaded or the action's
    /// bound elapsed (applied as the session page-load timeout), so every
    /// wait settles at once, as `timed_out` when the bound was hit.
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
        let mut pending = PendingNavigation::new(
            request.clone(),
            normalize_navigation_target(url),
            wait,
            Duration::from_millis(timeout_millis.unwrap_or(DEFAULT_NAVIGATION_TIMEOUT_MILLIS)),
            PendingNavigation::queued_for(request, crate::navigation::now_millis()),
            now,
        );
        if let Some(expired) = pending.tick(now) {
            // The bound elapsed while the action sat in the queue (for example
            // behind a blocking classic navigation): report it without
            // touching the page after the caller's deadline, and without
            // superseding a navigation that keeps running unreplaced.
            return AgentActionExecution::Done(expired);
        }
        self.supersede_pending_navigation(now);
        if self.config.browser.backend != BackendKind::FirefoxBidi {
            return AgentActionExecution::Done(self.navigate_classic_bounded(url, &mut pending, event_tx));
        }
        let dispatch = self.dispatch_bidi_navigate(url, event_tx);
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
        if wait == NavigationWait::Dispatched {
            return AgentActionExecution::Done(Ok(pending.dispatched(Instant::now())));
        }
        self.pending_navigation = Some(pending);
        AgentActionExecution::Pending
    }

    /// Classic `WebDriver` navigation under the action's remaining bound:
    /// the session page-load timeout is lowered for this command and restored
    /// afterwards. Hitting it reports `timed_out` with the URL the browser
    /// committed meanwhile (the navigation itself keeps running); any other
    /// error is a failed navigation.
    fn navigate_classic_bounded(
        &mut self,
        url: &str,
        pending: &mut PendingNavigation,
        event_tx: &BrowserEventSender,
    ) -> Result<crate::BrowserControlValue, BrowserControlFailure> {
        let url = normalize_navigation_target(url);
        let previous_url = self.url.clone();
        let bound_millis = u64::try_from(pending.remaining(Instant::now()).as_millis())
            .unwrap_or(u64::MAX)
            .max(1);
        // The action's bound replaces the session default in both directions
        // (a 60 s bound must not fail at the 50 s default); navigating under
        // the wrong bound would misreport the outcome, so a failed update is
        // a typed failure. The HTTP read outlasts the bound so the WebDriver
        // timeout error, not a socket timeout, normally ends the command, but
        // by less than the controller's delivery headroom so a hung driver
        // still yields the typed timeout in time.
        if let Err(error) = self.classic_post("timeouts", &json!({ "pageLoad": bound_millis })) {
            return Err(BrowserControlFailure::new(
                "protocol_error",
                format!("could not apply the navigation bound: {error}"),
            ));
        }
        let read_timeout = Duration::from_millis(bound_millis.saturating_add(CLASSIC_BOUND_READ_MARGIN_MILLIS));
        self.begin_navigation();
        let _ = event_tx.send(BrowserEvent::Loading(true));
        let result = self
            .classic_navigation_post_within("url", &json!({ "url": &url }), read_timeout)
            .and_then(|_| self.classic_get("url"))
            .and_then(|response| {
                classic_navigation_committed(&response, &url, &previous_url)
                    .then_some(())
                    .ok_or_else(|| "browser did not commit a reachable URL".to_string())
            });
        let now = Instant::now();
        if let Err(error) = &result
            && classic_error_is_page_load_timeout(error)
        {
            // Publish the typed outcome first. Restoring the session timeout
            // and re-reading the page state are best-effort calls with their
            // own 10 s guards; against a hung driver they would eat the
            // controller's delivery headroom, so the loop runs them after the
            // result is written.
            self.retain_frame_during_navigation = false;
            self.navigation_failed = false;
            self.classic_timeout_to_restore = Some(PAGE_LOAD_TIMEOUT_MILLIS);
            self.refresh_pending_at = Some(now);
            self.frames.demand();
            return Ok(pending.settle_timed_out(None, now));
        }
        let _ = self.classic_post("timeouts", &json!({ "pageLoad": PAGE_LOAD_TIMEOUT_MILLIS }));
        self.finish_page(result, &format!("navigation to {url}"), event_tx)
            .map_err(|error| BrowserControlFailure::new("navigation_failed", error))?;
        let (committed, title) = (self.url.clone(), self.title.clone());
        Ok(pending.settle_loaded(&committed, &title, Instant::now()))
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
            return;
        }
        let navigation = response
            .pointer("/result/navigation")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        self.attach_navigation_id(navigation);
    }

    fn fail_agent_navigation(&mut self, event_tx: &BrowserEventSender, message: &str) {
        self.navigation_failed = true;
        self.frames.interaction_started_at = None;
        let _ = event_tx.send(BrowserEvent::NavigationFailed(message.to_string()));
        let _ = event_tx.send(BrowserEvent::Loading(false));
        self.observe_navigation_signal(NavigationSignal::Failed { message, id: None });
    }

    /// The `browsingContext.navigate` reply named the navigation; attribute
    /// any held-back event.
    pub(super) fn attach_navigation_id(&mut self, navigation: Option<&str>) {
        let now = Instant::now();
        let settled = self
            .pending_navigation
            .as_mut()
            .and_then(|pending| pending.attach_id(navigation, now));
        if let Some(result) = settled
            && let Some(pending) = self.pending_navigation.take()
        {
            self.complete_agent_action(&pending.request, result);
        }
    }

    /// Map a `BiDi` navigation-complete event, identified by its navigation
    /// id, onto the pending navigation.
    pub(super) fn settle_navigation_from_bidi(&mut self, method: &str, navigation: Option<&str>) {
        let url = self.url.clone();
        tracing::debug!(target: "browser", method, navigation, pending = self.pending_navigation.is_some(), "bidi navigation event");
        if method.ends_with("fragmentNavigated") {
            self.observe_navigation_signal(NavigationSignal::SameDocument {
                url: &url,
                id: navigation,
            });
            return;
        }
        self.observe_navigation_signal(NavigationSignal::Committed {
            url: &url,
            id: navigation,
        });
        if method == "browsingContext.load" {
            self.observe_navigation_signal(NavigationSignal::Load { id: navigation });
        } else {
            self.observe_navigation_signal(NavigationSignal::DomContentLoaded { id: navigation });
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

    pub(super) fn supersede_pending_navigation(&mut self, now: Instant) {
        if let Some(pending) = self.pending_navigation.take() {
            // The superseded navigation's `browsingContext.navigate` reply may
            // still be in flight; a late rejection must not fail its
            // replacement.
            self.navigate_request_id = None;
            self.complete_agent_action(&pending.request, Ok(pending.superseded(now)));
        }
    }

    pub(super) fn navigate(&mut self, url: &str, event_tx: &BrowserEventSender) -> Result<(), String> {
        let url = normalize_navigation_target(url);
        // A non-agent navigation takes the page over: a pending agent
        // navigation can no longer claim the outcome.
        self.supersede_pending_navigation(Instant::now());
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
        self.supersede_pending_navigation(Instant::now());
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
        self.supersede_pending_navigation(Instant::now());
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
        self.classic_navigation_post_within(suffix, body, NAVIGATION_HTTP_TIMEOUT)
    }

    fn classic_navigation_post_within(
        &self,
        suffix: &str,
        body: &serde_json::Value,
        read_timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        self.service
            .http
            .post_with_read_timeout(&self.session_path(suffix), body, read_timeout)
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

    /// Restore the session page-load timeout a bounded classic navigation
    /// lowered, once its typed outcome has been published.
    pub(super) fn tick_classic_timeout_restore(&mut self) {
        if let Some(page_load) = self.classic_timeout_to_restore
            && self.classic_post("timeouts", &json!({ "pageLoad": page_load })).is_ok()
        {
            self.classic_timeout_to_restore = None;
        }
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

/// Whether a classic navigation command ended because the page-load bound
/// elapsed: either the `WebDriver` `timeout` error (the navigation keeps
/// running in the browser) or the HTTP read timeout guarding that command.
/// Any other error is a genuine navigation failure.
pub(super) fn classic_error_is_page_load_timeout(error: &str) -> bool {
    error.starts_with("WebDriver timeout:")
        || (error.starts_with("WebDriver HTTP I/O:")
            && (error.contains("timed out") || error.contains("temporarily unavailable")))
}
