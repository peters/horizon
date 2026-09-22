//! `WebDriver` `BiDi` companion link: command calls, event draining, the
//! navigation and context events that mutate the session, and the
//! subscription and preload handshake.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::BackendKind;
use crate::challenge::{DocumentCommit, REJECTION_MESSAGE};
use crate::disclosure::COMMON_SIGNAL_PRELOAD_FUNCTION;
use crate::session::{BrowserEvent, BrowserEventSender};
use crate::websocket::{JsonWsError, JsonWsLink};

use super::{COMMAND_TIMEOUT, Driver, consume_pending_history_start};

pub(super) const MAX_EVENT_BURST: usize = 32;

impl Driver {
    pub(super) fn call_bidi(
        &mut self,
        method: &str,
        params: &Value,
        event_tx: &BrowserEventSender,
    ) -> Result<Value, String> {
        let link = self.bidi.as_mut().ok_or_else(|| "BiDi is unavailable".to_string())?;
        let outcome = link.call(COMMAND_TIMEOUT, method, params);
        self.host.record_bidi_result(method, params, &outcome.result);
        for event in outcome.events {
            self.handle_bidi_event(&event, event_tx);
        }
        outcome.result.map_err(|error| error.to_string())
    }

    pub(super) fn poll_bidi_events(&mut self, slot: &crate::frames::FrameSlot, events: &BrowserEventSender) -> bool {
        if let Err(error) = self.drain_bidi_events(events) {
            tracing::warn!(backend = ?self.config.browser.backend, "BiDi event pump failed: {error}");
            if self.firefox_bidi() {
                let _ = events.send(BrowserEvent::Warning(format!("Firefox BiDi disconnected: {error}")));
                return false;
            }
            self.disable_optional_bidi(slot, events);
        }
        self.tick_file_chooser(events);
        true
    }

    pub(super) fn drain_bidi_events(&mut self, event_tx: &BrowserEventSender) -> Result<(), String> {
        let Some(link) = self.bidi.as_mut() else {
            return Ok(());
        };
        let events = link.drain(MAX_EVENT_BURST).map_err(|error| error.to_string())?;
        for event in events {
            self.handle_bidi_event(&event, event_tx);
        }
        Ok(())
    }

    pub(super) fn handle_bidi_event(&mut self, event: &Value, event_tx: &BrowserEventSender) {
        if let Some(id) = event.get("id").and_then(Value::as_u64)
            && self.navigate_request_id == Some(id)
        {
            self.handle_bidi_navigate_response(event, event_tx);
            return;
        }
        if !self.host.accepts_bidi_event(event) {
            return;
        }
        if event.get("method").and_then(Value::as_str) == Some("network.authRequired") {
            self.continue_http_auth(event, event_tx);
            return;
        }
        self.forget_completed_http_auth(event);
        if self.handle_file_chooser_event(event, event_tx) || self.handle_network_bidi_event(event) {
            return;
        }
        let method = event.get("method").and_then(Value::as_str).unwrap_or_default();
        let params = event.get("params").unwrap_or(&Value::Null);
        if self.context_id.is_none()
            && self.host.shared_context().is_none()
            && let Some(context) = created_top_level_context(method, params)
        {
            self.context_id = Some(context.to_string());
        }
        if !bidi_event_targets_context(method, params, self.context_id.as_deref()) {
            return;
        }
        if method.ends_with("navigationStarted") {
            if consume_pending_history_start(
                &mut self.pending_classic_history_start,
                params.get("url").and_then(Value::as_str),
                Instant::now(),
            ) {
                return;
            }
            self.begin_navigation();
            let _ = event_tx.send(BrowserEvent::Loading(true));
            return;
        }
        if bidi_navigation_failed(method) {
            let navigation = params.get("navigation").and_then(Value::as_str);
            let url = params
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("the requested page");
            let message = format!("could not navigate to {url}");
            if let Some(pending) = self.pending_navigation.as_ref() {
                if !pending.correlates(navigation) {
                    // A superseded navigation failing late must not poison the
                    // state of the navigation that replaced it.
                    tracing::debug!(target: "browser", navigation, "ignoring failure of a superseded navigation");
                    return;
                }
                if pending.attribution_is_pending(navigation) {
                    // The dispatch reply has not named this navigation yet.
                    // Hold the failure without changing page-wide state; the
                    // reply either attributes it to this action or discards it
                    // as a late failure from the navigation it replaced.
                    self.observe_navigation_signal(crate::navigation::NavigationSignal::Failed {
                        message: &message,
                        id: navigation,
                    });
                    return;
                }
            }
            self.apply_navigation_failure_state(event_tx, &message);
            self.observe_navigation_signal(crate::navigation::NavigationSignal::Failed {
                message: &message,
                id: navigation,
            });
            return;
        }
        let navigation_complete = bidi_navigation_complete(method);
        if navigation_complete && !self.navigation_failed {
            let committed_url = params
                .get("url")
                .and_then(Value::as_str)
                .map_or_else(|| self.url.clone(), str::to_string);
            let previous_url = self.url.clone();
            let document_commit = self
                .challenge_loop
                .document_committed(&committed_url, params.get("navigation").and_then(Value::as_str));
            if committed_url != self.url {
                self.url = committed_url;
                self.coordination_dirty = true;
            }
            if document_commit == DocumentCommit::Recovered || previous_url != self.url {
                let _ = event_tx.send(BrowserEvent::UrlChanged(self.url.clone()));
            }
            if document_commit == DocumentCommit::Rejected && previous_url != self.url {
                let _ = event_tx.send(BrowserEvent::NavigationFailed(REJECTION_MESSAGE.to_string()));
            }
            self.retain_frame_during_navigation = false;
            let _ = event_tx.send(BrowserEvent::Loading(false));
            self.frames.demand();
            self.refresh_pending_at = Some(Instant::now() + Duration::from_millis(50));
            self.settle_navigation_from_bidi(method, params.get("navigation").and_then(Value::as_str));
        } else if method.ends_with("contextDestroyed") {
            let destroyed = params.get("context").and_then(Value::as_str);
            if destroyed == self.context_id.as_deref() {
                self.context_id = None;
                self.host.context_destroyed();
                self.advance_generation();
            }
        }
    }
}

pub(super) fn discover_context(link: &mut JsonWsLink) -> Option<String> {
    let outcome = link.call(COMMAND_TIMEOUT, "browsingContext.getTree", &json!({ "maxDepth": 0 }));
    outcome.result.ok().and_then(|result| {
        result
            .get("contexts")?
            .as_array()?
            .first()?
            .get("context")?
            .as_str()
            .map(str::to_string)
    })
}

pub(super) fn connect_bidi_with_startup_retry(url: &str, stop_requested: &AtomicBool) -> Result<JsonWsLink, String> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match JsonWsLink::connect(url) {
            Ok(link) => return Ok(link),
            Err(JsonWsError::InvalidUrl(_)) => {
                return Err("browser returned an invalid non-loopback BiDi endpoint".to_string());
            }
            Err(error) => {
                if stop_requested.load(Ordering::Acquire) || Instant::now() >= deadline {
                    return Err(format!("failed to connect returned BiDi endpoint: {error}"));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub(super) fn subscribe_shared_page(
    link: &mut JsonWsLink,
    host: &mut super::super::host::DriverHost,
    disclosure: crate::AutomationDisclosurePolicy,
) -> Result<(), String> {
    let context = host
        .shared_context()
        .ok_or_else(|| "missing shared Firefox context".to_string())?
        .to_owned();
    let mut register = |method, params: Value, field, remove| -> Result<(), String> {
        let result = link
            .call(COMMAND_TIMEOUT, method, &params)
            .result
            .map_err(|error| error.to_string())?;
        let id = result
            .get(field)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{method} returned no registration identity"))?;
        host.remember_bidi_registration(remove, id.to_owned());
        Ok(())
    };
    register(
        "session.subscribe",
        bidi_subscription_params(&base_bidi_events(), Some(&context)),
        "subscription",
        "session.unsubscribe",
    )?;
    register(
        "session.subscribe",
        bidi_subscription_params(super::http_auth::firefox_http_auth_events(), Some(&context)),
        "subscription",
        "session.unsubscribe",
    )?;
    register(
        "network.addIntercept",
        super::http_auth::firefox_http_auth_intercept_params(&context),
        "intercept",
        "network.removeIntercept",
    )?;
    if disclosure == crate::AutomationDisclosurePolicy::MinimizeCommonSignals {
        register(
            "script.addPreloadScript",
            json!({"functionDeclaration": COMMON_SIGNAL_PRELOAD_FUNCTION, "contexts": [context]}),
            "script",
            "script.removePreloadScript",
        )?;
    }
    Ok(())
}

pub(super) fn subscribe(link: &mut JsonWsLink, backend: BackendKind, context_id: Option<&str>) -> Result<(), String> {
    subscribe_bidi_events(link, &base_bidi_events(), None)?;
    if backend == BackendKind::FirefoxBidi {
        let context = context_id.ok_or_else(|| "Firefox BiDi returned no top-level browsing context".to_string())?;
        subscribe_bidi_events(link, super::http_auth::firefox_http_auth_events(), Some(context))?;
        link.call(
            COMMAND_TIMEOUT,
            "network.addIntercept",
            &super::http_auth::firefox_http_auth_intercept_params(context),
        )
        .result
        .map(|_| ())
        .map_err(|error| format!("Firefox could not intercept HTTP authentication challenges: {error}"))?;
    }
    Ok(())
}

pub(super) fn subscribe_bidi_events(
    link: &mut JsonWsLink,
    events: &[&str],
    context: Option<&str>,
) -> Result<(), String> {
    let params = bidi_subscription_params(events, context);
    link.call(COMMAND_TIMEOUT, "session.subscribe", &params)
        .result
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(super) fn bidi_subscription_params(events: &[&str], context: Option<&str>) -> Value {
    let mut params = json!({ "events": events });
    if let Some(context) = context {
        params["contexts"] = json!([context]);
    }
    params
}

pub(super) fn base_bidi_events() -> Vec<&'static str> {
    vec![
        "browsingContext.contextCreated",
        "browsingContext.contextDestroyed",
        "browsingContext.navigationStarted",
        "browsingContext.navigationFailed",
        "browsingContext.fragmentNavigated",
        "browsingContext.domContentLoaded",
        "browsingContext.load",
    ]
}

pub(super) fn install_common_signal_preload(link: &mut JsonWsLink) -> Result<(), String> {
    link.call(
        COMMAND_TIMEOUT,
        "script.addPreloadScript",
        &json!({ "functionDeclaration": COMMON_SIGNAL_PRELOAD_FUNCTION }),
    )
    .result
    .map(|_| ())
    .map_err(|error| error.to_string())
}

pub(super) fn bidi_navigation_complete(method: &str) -> bool {
    method.ends_with("domContentLoaded") || method.ends_with("fragmentNavigated") || method.ends_with("load")
}

pub(super) fn bidi_navigation_failed(method: &str) -> bool {
    method.ends_with("navigationFailed")
}

pub(super) fn bidi_event_targets_context(method: &str, params: &Value, context_id: Option<&str>) -> bool {
    let context_scoped = method.ends_with("navigationStarted")
        || bidi_navigation_failed(method)
        || bidi_navigation_complete(method)
        || method.ends_with("contextDestroyed");
    !context_scoped || params.get("context").and_then(Value::as_str) == context_id
}

fn created_top_level_context<'a>(method: &str, params: &'a Value) -> Option<&'a str> {
    if method != "browsingContext.contextCreated" || !params.get("parent").is_some_and(Value::is_null) {
        return None;
    }
    params
        .get("context")
        .and_then(Value::as_str)
        .filter(|context| !context.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_created_top_level_context_can_replace_a_destroyed_binding() {
        for (method, params) in [
            (
                "browsingContext.contextCreated",
                json!({"context":"frame", "parent":"top"}),
            ),
            ("browsingContext.contextCreated", json!({"context":"unknown"})),
            (
                "browsingContext.navigationStarted",
                json!({"context":"frame", "parent":null}),
            ),
            (
                "browsingContext.contextDestroyed",
                json!({"context":"old", "parent":null}),
            ),
        ] {
            assert_eq!(created_top_level_context(method, &params), None);
        }
        let root = json!({"context":"replacement", "parent":null});
        assert_eq!(
            created_top_level_context("browsingContext.contextCreated", &root),
            Some("replacement")
        );
    }
}
