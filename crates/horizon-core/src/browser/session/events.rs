//! CDP event handling for the driver state machine: target binding, page
//! navigation and title tracking, and screencast lifecycle.
//!
//! Split out of the driver loop ([super]) — the event arms are the
//! state machine's reactive half, while the parent module keeps the loop,
//! command drain, and manifest/signal plumbing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::browser::cdp::{CdpEvent, CdpLink, CdpMsg};
use crate::browser::frames::FrameSlot;

use super::{
    BrowserEvent, BrowserEventSender, DriverState, TITLE_BINDING_NAME, normalized_committed_url, publish_frame,
};

impl DriverState {
    pub(super) fn tick_title_fetch(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
    ) {
        let Some(due) = self.title_fetch_at else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        self.title_fetch_at = None;
        self.fetch_title(link, event_tx, frame_slot);
    }

    /// Best-effort current title for initial attach and navigation, where a
    /// browser-level target-info change may not have been observed yet.
    pub(super) fn fetch_title(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
    ) {
        let Some(session) = self.session_id.clone() else {
            return;
        };
        // Fetch title and href together: after a navigation the session's
        // execution context can still be the *previous* document's, so a
        // bare `document.title` read can return the old page's title. The
        // href check detects that and schedules a retry.
        let params = match self.title_context_id {
            Some(context_id) => serde_json::json!({
                "expression": "JSON.stringify({ t: document.title, h: location.href })",
                "returnByValue": true,
                "contextId": context_id,
            }),
            None => serde_json::json!({
                "expression": "JSON.stringify({ t: document.title, h: location.href })",
                "returnByValue": true,
            }),
        };
        let Ok(result) = self.call_and_ack(
            link,
            event_tx,
            frame_slot,
            "Runtime.evaluate",
            &params,
            Some(session.as_str()),
        ) else {
            return;
        };
        let Some(payload) = result.pointer("/result/value").and_then(|v| v.as_str()) else {
            return;
        };
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(payload) else {
            return;
        };
        let Some(href) = parsed.pointer("/h").and_then(|v| v.as_str()) else {
            return;
        };
        if !href_matches_url(href, &self.url) {
            if self.title_fetch_retries < 10 {
                self.title_fetch_retries += 1;
                self.title_fetch_at = Some(Instant::now() + Duration::from_millis(500));
            }
            return;
        }
        let Some(title) = parsed.pointer("/t").and_then(|v| v.as_str()) else {
            return;
        };
        self.update_title(event_tx, title);
    }

    fn update_title(&mut self, event_tx: &BrowserEventSender, title: &str) {
        if title == self.title {
            return;
        }
        self.title = title.to_string();
        self.manifest_dirty = true;
        self.write_manifest(false);
        let _ = event_tx.send(BrowserEvent::Title(title.to_string()));
    }

    /// Track the top-level document's default execution context for the
    /// title fetch. Iframes also publish `isDefault` contexts, so their
    /// `frameId` must not replace the context used for page metadata.
    fn note_execution_context(&mut self, event: &CdpEvent<'_>) {
        if event.method == "Runtime.executionContextCreated"
            && let Some(id) = default_context_id_for_frame(event.params, self.main_frame_id.as_deref())
        {
            self.title_context_id = Some(id);
        } else if let Some(id) = event
            .params
            .get("executionContextId")
            .and_then(serde_json::Value::as_u64)
            && self.title_context_id == Some(id)
        {
            self.title_context_id = None;
        }
    }

    pub(super) fn handle_message(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        message: CdpMsg,
    ) {
        if let Some(event) = message.event() {
            self.handle_event(link, event_tx, frame_slot, event);
            return;
        }
        let CdpMsg::Response { id, result, error, .. } = message else {
            return;
        };
        if self.clipboard_read_request_ids.remove(&id) {
            if let Some(error) = error {
                tracing::debug!(target: "browser", "clipboard selection capture rejected: {error}");
            } else if let Some(text) = clipboard_text_from_evaluation(result.as_ref())
                && !text.is_empty()
            {
                let _ = event_tx.send(BrowserEvent::ClipboardText(text.to_string()));
            }
            return;
        }
        if self.viewport_capture_request_id == Some(id) {
            self.viewport_capture_request_id = None;
            if let Some(error) = error {
                tracing::debug!(target: "browser", "viewport frame capture rejected: {error}");
                return;
            }
            let Some(data) = result
                .as_ref()
                .and_then(|value| value.get("data"))
                .and_then(serde_json::Value::as_str)
            else {
                return;
            };
            if let Some(seq) = frame_slot.store_base64_jpeg(data) {
                publish_frame(event_tx, frame_slot, seq);
            }
            return;
        }
        // Fire-and-forget input/ack responses also carry ids; only the
        // exact re-attach request may consume the in-flight flag.
        if self.reattach_in_flight && Some(id) == self.reattach_request_id {
            self.reattach_in_flight = false;
            self.reattach_request_id = None;
            if let Some(error) = error {
                tracing::warn!(target: "browser", "re-attach rejected: {error}");
                self.note_reattach_failure(event_tx, &error.to_string());
                return;
            }
            self.reattach_failures = 0;
            // The auto-attach event may already have re-bound us;
            // only the first binder sets up the page.
            if self.session_id.is_none()
                && let Some(session) = result
                    .as_ref()
                    .and_then(|r| r.get("sessionId"))
                    .and_then(|s| s.as_str())
                && let Some(target) = self.target_id.clone()
            {
                self.attach_setup(link, event_tx, frame_slot, session, &target);
            }
            return;
        }
        if let Some(error) = error {
            tracing::debug!(target: "browser", "cdp response error id={id}: {error}");
        }
    }

    pub(super) fn handle_event(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        event: CdpEvent<'_>,
    ) {
        let on_page_session = event.session_id.is_some_and(|s| Some(s) == self.session_id.as_deref());
        match event.method {
            "Target.attachedToTarget" => {
                // Popups and agent-opened tabs must not steal the binding;
                // it is only replaced after it dies or a forced re-attach.
                if self.session_id.is_some() {
                    return;
                }
                let Some(session) = event.session_id else {
                    return;
                };
                let Some(target_id) = attached_bound_page_target(event.params, self.target_id.as_deref()) else {
                    return;
                };
                self.attach_setup(link, event_tx, frame_slot, session, target_id);
            }
            "Target.detachedFromTarget" => {
                if on_page_session {
                    self.session_id = None;
                    self.screencast_on = false;
                    self.pending_viewport_capture_at = None;
                    self.viewport_capture_request_id = None;
                    self.clipboard_read_request_ids.clear();
                    // Drop the tracked context: the next title fetch must
                    // use a fresh default context, not a dead contextId.
                    self.title_context_id = None;
                    self.main_frame_id = None;
                    self.title_fetch_at = None;
                    self.title_fetch_retries = 0;
                    // Unexpected detach (the driver never detaches its own
                    // page session): the target usually still exists —
                    // re-bind instead of freezing. `pending_restart_tick`
                    // acts on this on the next loop pass; a target that is
                    // actually gone surfaces through targetDestroyed.
                    self.pending_reattach = true;
                }
            }
            "Target.targetDestroyed" => {
                let destroyed = event.params.get("targetId").and_then(|t| t.as_str());
                if self.forget_destroyed_bound_target(destroyed) {
                    // External agents discover the page through this field.
                    // Clear it synchronously so a destroyed target is not
                    // advertised as a live endpoint after this event.
                    self.write_manifest(true);
                    // The page is gone (tab closed by another CDP client,
                    // navigation to a new target, …). Re-attach has
                    // nothing to bind to: surface a retryable error
                    // instead of silently ignoring input on a frozen frame.
                    frame_slot.clear();
                    let _ = event_tx.send(BrowserEvent::Warning(
                        "the page target was destroyed; retry to reattach".to_string(),
                    ));
                }
            }
            "Target.targetInfoChanged" => {
                if let Some(title) = title_for_bound_target(event.params, self.target_id.as_deref()) {
                    self.update_title(event_tx, title);
                }
            }
            "Runtime.bindingCalled" => {
                if on_page_session
                    && let Some((title, href)) = title_from_binding(event.params)
                    && href_matches_url(&href, &self.url)
                {
                    self.update_title(event_tx, &title);
                }
            }
            "Page.frameNavigated" => self.handle_frame_navigated(event_tx, event, on_page_session),
            "Page.navigatedWithinDocument" => {
                self.handle_same_document_navigation(event_tx, event, on_page_session);
            }
            "Page.loadEventFired" => {
                if on_page_session {
                    let _ = event_tx.send(BrowserEvent::Loading(false));
                    self.title_fetch_retries = 0;
                    self.title_fetch_at = Some(Instant::now() + Duration::from_millis(400));
                }
            }
            "Runtime.executionContextCreated" | "Runtime.executionContextDestroyed" => {
                if on_page_session {
                    self.note_execution_context(&event);
                }
            }
            "Page.screencastFrame" => Self::handle_screencast_frame(link, event_tx, frame_slot, event),
            _ => {}
        }
    }

    fn forget_destroyed_bound_target(&mut self, destroyed: Option<&str>) -> bool {
        let Some(destroyed) = destroyed else {
            return false;
        };
        if self.target_id.as_deref() != Some(destroyed) {
            return false;
        }
        self.session_id = None;
        self.target_id = None;
        self.main_frame_id = None;
        self.screencast_on = false;
        self.pending_viewport_capture_at = None;
        self.viewport_capture_request_id = None;
        self.clipboard_read_request_ids.clear();
        self.pending_reattach = false;
        self.manifest_dirty = true;
        true
    }

    fn handle_frame_navigated(&mut self, event_tx: &BrowserEventSender, event: CdpEvent<'_>, on_page_session: bool) {
        if !on_page_session {
            return;
        }
        // Subframe (iframe) navigations carry `frame.parentId`; only the top
        // frame defines the panel's URL.
        let Some(frame) = event.params.get("frame") else {
            return;
        };
        if frame.get("parentId").is_some() {
            return;
        }
        self.main_frame_id = frame.get("id").and_then(|id| id.as_str()).map(str::to_string);
        if let Some(target_url) = frame.get("url").and_then(|url| url.as_str())
            && normalized_committed_url(target_url) != self.url
        {
            self.url = normalized_committed_url(target_url).to_string();
            self.manifest_dirty = true;
        }
        self.pending_restart_at = Some(Instant::now());
        // A back/forward-cache restore can commit `frameNavigated` without a
        // later `loadEventFired`. Always schedule the title read here so the
        // panel cannot retain the destination page's title after history
        // navigation. The href guard in `fetch_title` retries if the new
        // execution context is not ready yet.
        self.title_fetch_retries = 0;
        self.title_fetch_at = Some(Instant::now() + Duration::from_millis(400));
        self.write_manifest(true);
        let _ = event_tx.send(BrowserEvent::UrlChanged(self.url.clone()));
        let _ = event_tx.send(BrowserEvent::Loading(true));
    }

    fn handle_same_document_navigation(
        &mut self,
        event_tx: &BrowserEventSender,
        event: CdpEvent<'_>,
        on_page_session: bool,
    ) {
        if !on_page_session {
            return;
        }
        let frame_id = event.params.get("frameId").and_then(|id| id.as_str());
        if self.main_frame_id.as_deref() != frame_id {
            return;
        }
        let Some(target_url) = event.params.get("url").and_then(|url| url.as_str()) else {
            return;
        };
        let url = normalized_committed_url(target_url);
        if url == self.url {
            return;
        }
        self.url = url.to_string();
        self.manifest_dirty = true;
        self.write_manifest(true);
        let _ = event_tx.send(BrowserEvent::UrlChanged(self.url.clone()));
    }

    /// Ack, then decode and store one screencast frame.
    ///
    /// The ack goes out first and on every path: Chrome holds the
    /// screencast until each frame is acknowledged, so a malformed frame
    /// must never stall an otherwise healthy stream.
    fn handle_screencast_frame(
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &FrameSlot,
        event: CdpEvent<'_>,
    ) {
        // Ack so the stream continues: params.sessionId echoes the frame's
        // session identifier, the top-level sessionId scopes the call
        // (flattened sessions).
        let frame_session = event
            .params
            .get("sessionId")
            .cloned()
            .or_else(|| event.session_id.map(|s| serde_json::Value::String(s.to_string())));
        let ack_params = match frame_session {
            Some(value) => serde_json::json!({ "sessionId": value }),
            None => serde_json::json!({}),
        };
        // Id'd request: Chrome silently drops CDP requests without an id.
        let _ = link.send_request("Page.screencastFrameAck", &ack_params, event.session_id);
        let Some(data) = event.params.get("data").and_then(|d| d.as_str()) else {
            return;
        };
        if let Some(seq) = frame_slot.store_base64_jpeg(data) {
            publish_frame(event_tx, frame_slot, seq);
        } else {
            tracing::warn!(target: "browser", "dropping undecodable screencast frame");
        }
    }
}

/// Tolerant URL comparison for the title-fetch href check: Chrome may
/// append a trailing slash relative to the requested URL.
fn href_matches_url(href: &str, url: &str) -> bool {
    let href = href.trim_end_matches('/');
    let url = url.trim_end_matches('/');
    href == url
}

fn clipboard_text_from_evaluation(result: Option<&serde_json::Value>) -> Option<&str> {
    result?.pointer("/result/value")?.as_str()
}

fn title_for_bound_target<'a>(params: &'a serde_json::Value, target_id: Option<&str>) -> Option<&'a str> {
    let target_info = params.get("targetInfo")?;
    let changed_target_id = target_info.get("targetId")?.as_str()?;
    (Some(changed_target_id) == target_id)
        .then(|| target_info.get("title").and_then(serde_json::Value::as_str))
        .flatten()
}

fn attached_bound_page_target<'a>(params: &'a serde_json::Value, bound_target_id: Option<&str>) -> Option<&'a str> {
    let target_info = params.get("targetInfo")?;
    if target_info.get("type")?.as_str()? != "page" {
        return None;
    }
    let attached_target_id = target_info.get("targetId")?.as_str()?;
    (Some(attached_target_id) == bound_target_id).then_some(attached_target_id)
}

fn title_from_binding(params: &serde_json::Value) -> Option<(String, String)> {
    if params.get("name")?.as_str()? != TITLE_BINDING_NAME {
        return None;
    }
    let payload = params.get("payload")?.as_str()?;
    let payload = serde_json::from_str::<serde_json::Value>(payload).ok()?;
    let title = payload.get("title")?.as_str()?.to_string();
    let href = payload.get("href")?.as_str()?.to_string();
    Some((title, href))
}

fn default_context_id_for_frame(params: &serde_json::Value, main_frame_id: Option<&str>) -> Option<u64> {
    let context = params.get("context")?;
    let is_default = context.pointer("/auxData/isDefault")?.as_bool()?;
    let frame_id = context.pointer("/auxData/frameId")?.as_str()?;
    if !is_default || Some(frame_id) != main_frame_id {
        return None;
    }
    context.get("id")?.as_u64()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use crate::browser::frames::FrameSlot;
    use crate::browser::{BrowserConfig, session::BrowserSessionConfig};

    use super::{
        DriverState, attached_bound_page_target, clipboard_text_from_evaluation, default_context_id_for_frame,
        title_for_bound_target, title_from_binding,
    };

    fn driver_state() -> DriverState {
        DriverState::new(
            &BrowserSessionConfig {
                browser: BrowserConfig::default(),
                panel_local_id: "panel-1".to_string(),
                initial_url: None,
                width: 1280,
                height: 800,
                frame_slot: Arc::new(FrameSlot::new()),
            },
            "ws://127.0.0.1/devtools/browser/test",
            Arc::new(AtomicBool::new(false)),
        )
    }

    #[test]
    fn title_context_accepts_only_the_main_frames_default_context() {
        let main = serde_json::json!({
            "context": { "id": 7, "auxData": { "isDefault": true, "frameId": "main" } }
        });
        let iframe = serde_json::json!({
            "context": { "id": 8, "auxData": { "isDefault": true, "frameId": "child" } }
        });
        let isolated = serde_json::json!({
            "context": { "id": 9, "auxData": { "isDefault": false, "frameId": "main" } }
        });

        assert_eq!(default_context_id_for_frame(&main, Some("main")), Some(7));
        assert_eq!(default_context_id_for_frame(&iframe, Some("main")), None);
        assert_eq!(default_context_id_for_frame(&isolated, Some("main")), None);
        assert_eq!(default_context_id_for_frame(&main, None), None);
    }

    #[test]
    fn clipboard_text_reads_runtime_evaluation_values_only() {
        let value = serde_json::json!({ "result": { "type": "string", "value": "selected" } });
        let exception = serde_json::json!({ "exceptionDetails": { "text": "failed" } });

        assert_eq!(clipboard_text_from_evaluation(Some(&value)), Some("selected"));
        assert_eq!(clipboard_text_from_evaluation(Some(&exception)), None);
        assert_eq!(clipboard_text_from_evaluation(None), None);
    }

    #[test]
    fn target_title_updates_only_match_the_bound_page() {
        let params = serde_json::json!({
            "targetInfo": { "targetId": "bound", "type": "page", "title": "Updated" }
        });

        assert_eq!(title_for_bound_target(&params, Some("bound")), Some("Updated"));
        assert_eq!(title_for_bound_target(&params, Some("other")), None);
        assert_eq!(title_for_bound_target(&params, None), None);
    }

    #[test]
    fn auto_attach_accepts_only_the_already_bound_page() {
        let page = serde_json::json!({
            "targetInfo": { "targetId": "bound", "type": "page" }
        });
        let popup = serde_json::json!({
            "targetInfo": { "targetId": "popup", "type": "page" }
        });
        let worker = serde_json::json!({
            "targetInfo": { "targetId": "bound", "type": "worker" }
        });

        assert_eq!(attached_bound_page_target(&page, Some("bound")), Some("bound"));
        assert_eq!(attached_bound_page_target(&popup, Some("bound")), None);
        assert_eq!(attached_bound_page_target(&page, None), None);
        assert_eq!(attached_bound_page_target(&worker, Some("bound")), None);
    }

    #[test]
    fn destroying_the_bound_target_marks_its_manifest_identity_for_removal() {
        let mut state = driver_state();
        assert!(!state.forget_destroyed_bound_target(None));

        state.target_id = Some("bound".to_string());
        state.session_id = Some("session".to_string());
        state.manifest_dirty = false;

        assert!(!state.forget_destroyed_bound_target(Some("popup")));
        assert_eq!(state.target_id.as_deref(), Some("bound"));
        assert!(!state.manifest_dirty);

        assert!(state.forget_destroyed_bound_target(Some("bound")));
        assert_eq!(state.target_id, None);
        assert_eq!(state.session_id, None);
        assert!(state.manifest_dirty);
        assert!(!state.pending_reattach);
    }

    #[test]
    fn title_binding_accepts_only_the_horizon_payload() {
        let params = serde_json::json!({
            "name": "__horizonBrowserTitleChanged",
            "payload": r#"{"title":"Updated","href":"https://example.test/page"}"#,
        });
        let other_binding = serde_json::json!({
            "name": "pageBinding",
            "payload": r#"{"title":"Wrong","href":"https://example.test/page"}"#,
        });

        assert_eq!(
            title_from_binding(&params),
            Some(("Updated".to_string(), "https://example.test/page".to_string()))
        );
        assert_eq!(title_from_binding(&other_binding), None);
        assert_eq!(title_from_binding(&serde_json::json!({})), None);
    }
}
