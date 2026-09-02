//! CDP event handling for the driver state machine: target binding, page
//! navigation and title tracking, and screencast lifecycle.
//!
//! Split out of the driver loop ([super]) — the event arms are the
//! state machine's reactive half, while the parent module keeps the loop,
//! command drain, and manifest/signal plumbing.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cdp::{CdpEvent, CdpLink, CdpMsg};
use crate::challenge::{DocumentCommit, REJECTION_MESSAGE};
use crate::frames::FrameSlot;

use super::clipboard::target_event_session_id;
use super::{BrowserEvent, BrowserEventSender, DriverState, normalized_committed_url, publish_frame};
use crate::navigation::NavigationSignal;

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
        let Some(target_id) = self.target_id.clone() else {
            return;
        };
        let Ok(result) = self.call_and_ack(
            link,
            event_tx,
            frame_slot,
            "Target.getTargetInfo",
            &serde_json::json!({ "targetId": target_id }),
            None,
        ) else {
            return;
        };
        if self.retain_frame_during_navigation {
            return;
        }
        let Some(title) = result.pointer("/targetInfo/title").and_then(serde_json::Value::as_str) else {
            return;
        };
        self.update_title(event_tx, title);
    }

    #[cfg(test)]
    fn update_url_from_page(&mut self, event_tx: &BrowserEventSender, href: &str) {
        let href = normalized_committed_url(href);
        if href == self.url {
            return;
        }
        self.url = href.to_string();
        self.manifest_dirty = true;
        self.write_manifest(false);
        self.publish_url_changed(event_tx);
    }

    fn publish_url_changed(&self, event_tx: &BrowserEventSender) {
        let _ = event_tx.send(BrowserEvent::UrlChanged(self.url.clone()));
        if self.challenge_loop.rejection_is_reported() {
            let _ = event_tx.send(BrowserEvent::NavigationFailed(REJECTION_MESSAGE.to_string()));
        }
    }

    fn update_title(&mut self, event_tx: &BrowserEventSender, title: &str) {
        if title == self.title {
            return;
        }
        self.title = title.to_string();
        self.manifest_dirty = true;
        self.write_manifest(false);
        let _ = event_tx.send(BrowserEvent::Title(title.to_string()));
        self.observe_navigation_signal(NavigationSignal::Title(title));
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
        if self.handle_clipboard_response(id, result.as_ref(), error.as_ref(), event_tx) {
            return;
        }
        if self.handle_navigate_response(event_tx, id, result.as_ref(), error.as_ref()) {
            return;
        }
        if self.handle_screencast_response(id, error.as_ref()) {
            return;
        }
        if self.handle_runtime_enable_response(id, error.as_ref()) {
            self.flush_pending_clipboard(link);
            return;
        }
        if self.handle_scrollbar_layout_response(id, result.as_ref(), error.is_some()) {
            if let Some(error) = error {
                tracing::debug!(target: "browser", "scrollbar layout request rejected: {error}");
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
            frame_slot.record_capture_completion();
            if self.retain_frame_during_navigation {
                return;
            }
            if let Some(seq) = frame_slot.store_base64_jpeg(data) {
                self.record_interaction_frame(frame_slot);
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
        if on_page_session {
            self.handle_network_event(&event);
        }
        match event.method {
            "Target.attachedToTarget" => {
                if self.note_clipboard_target_attachment(link, &event) {
                    return;
                }
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
                self.note_clipboard_target_detachment(&event);
                if target_event_session_id(event.params, event.session_id) == self.session_id.as_deref() {
                    self.session_id = None;
                    self.screencast_on = false;
                    self.screencast_request_id = None;
                    self.navigate_request_id = None;
                    self.pending_viewport_capture_at = None;
                    self.viewport_capture_request_id = None;
                    self.invalidate_scrollbar_layout();
                    self.reset_clipboard_tracking();
                    self.main_frame_id = None;
                    self.title_fetch_at = None;
                    self.reset_runtime_enable_state();
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
                if !self.retain_frame_during_navigation
                    && let Some(title) = title_for_bound_target(event.params, self.target_id.as_deref())
                {
                    self.update_title(event_tx, title);
                }
            }
            "Page.frameNavigated" => self.handle_frame_navigated(event_tx, event, on_page_session),
            "Page.navigatedWithinDocument" => {
                self.handle_same_document_navigation(event_tx, event, on_page_session);
            }
            "Page.loadEventFired" => {
                if on_page_session {
                    let _ = event_tx.send(BrowserEvent::Loading(false));
                    self.title_fetch_at = Some(Instant::now() + Duration::from_millis(400));
                    self.observe_navigation_signal(NavigationSignal::Load);
                }
            }
            "Page.domContentEventFired" => {
                if on_page_session {
                    self.observe_navigation_signal(NavigationSignal::DomContentLoaded);
                }
            }
            "Runtime.executionContextCreated"
            | "Runtime.executionContextDestroyed"
            | "Runtime.executionContextsCleared" => {
                self.note_clipboard_execution_context(&event);
                self.flush_pending_clipboard(link);
            }
            "Page.screencastFrame" => self.handle_screencast_frame(link, event_tx, frame_slot, event),
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
        self.screencast_request_id = None;
        self.navigate_request_id = None;
        self.pending_viewport_capture_at = None;
        self.viewport_capture_request_id = None;
        self.invalidate_scrollbar_layout();
        self.reset_clipboard_tracking();
        self.pending_reattach = false;
        self.reset_runtime_enable_state();
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
        self.invalidate_scrollbar_layout();
        self.semantic.invalidate();
        self.main_frame_id = frame.get("id").and_then(|id| id.as_str()).map(str::to_string);
        if let Some(unreachable_url) = frame
            .get("unreachableUrl")
            .and_then(|url| url.as_str())
            .filter(|url| !url.is_empty())
        {
            self.retain_frame_during_navigation = true;
            self.navigation_failed = true;
            self.interaction_started_at = None;
            self.title_fetch_at = None;
            let message = format!("could not navigate to {unreachable_url}: the page was unreachable");
            let _ = event_tx.send(BrowserEvent::NavigationFailed(message.clone()));
            let _ = event_tx.send(BrowserEvent::Loading(false));
            self.observe_navigation_signal(NavigationSignal::Failed(&message));
            return;
        }
        self.retain_frame_during_navigation = false;
        self.navigation_failed = false;
        let previous_url = self.url.clone();
        let committed_url = frame
            .get("url")
            .and_then(|url| url.as_str())
            .map_or(self.url.as_str(), normalized_committed_url)
            .to_string();
        let document_commit = self
            .challenge_loop
            .document_committed(&committed_url, frame.get("loaderId").and_then(|id| id.as_str()));
        if committed_url != self.url {
            self.url = committed_url;
            self.manifest_dirty = true;
        }
        self.pending_restart_at = Some(Instant::now());
        // A back/forward-cache restore can commit `frameNavigated` without a
        // later `loadEventFired`. Always schedule the title read here so the
        // panel cannot retain the destination page's title after history
        // navigation.
        self.title_fetch_at = Some(Instant::now() + Duration::from_millis(400));
        self.write_manifest(true);
        match document_commit {
            DocumentCommit::Rejected if previous_url == self.url => {}
            DocumentCommit::Normal | DocumentCommit::Recovered | DocumentCommit::Rejected => {
                self.publish_url_changed(event_tx);
            }
        }
        let _ = event_tx.send(BrowserEvent::Loading(true));
        let committed = self.url.clone();
        self.observe_navigation_signal(NavigationSignal::Committed(&committed));
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
        if self.retain_frame_during_navigation && !href_matches_url(target_url, &self.url) {
            return;
        }
        self.retain_frame_during_navigation = false;
        self.navigation_failed = false;
        self.invalidate_scrollbar_layout();
        let url = normalized_committed_url(target_url);
        if url == self.url {
            return;
        }
        self.url = url.to_string();
        self.manifest_dirty = true;
        self.write_manifest(true);
        self.publish_url_changed(event_tx);
        self.observe_navigation_signal(NavigationSignal::SameDocument(url));
    }

    /// Ack, then decode and store one screencast frame.
    ///
    /// The ack goes out first and on every path: Chrome holds the
    /// screencast until each frame is acknowledged, so a malformed frame
    /// must never stall an otherwise healthy stream.
    fn handle_screencast_frame(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &FrameSlot,
        event: CdpEvent<'_>,
    ) {
        self.note_screencast_scroll_offset(
            event
                .params
                .pointer("/metadata/scrollOffsetY")
                .and_then(serde_json::Value::as_f64),
        );
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
        frame_slot.record_frame_received();
        // Id'd request: Chrome silently drops CDP requests without an id.
        if link
            .send_request("Page.screencastFrameAck", &ack_params, event.session_id)
            .is_ok()
        {
            frame_slot.record_frame_acked();
        }
        let Some(data) = event.params.get("data").and_then(|d| d.as_str()) else {
            return;
        };
        if self.retain_frame_during_navigation {
            return;
        }
        if let Some(seq) = frame_slot.store_base64_jpeg(data) {
            self.record_interaction_frame(frame_slot);
            publish_frame(event_tx, frame_slot, seq);
        } else {
            tracing::warn!(target: "browser", "dropping undecodable screencast frame");
        }
    }

    fn record_interaction_frame(&mut self, frame_slot: &FrameSlot) {
        if let Some(started_at) = self.interaction_started_at.take() {
            frame_slot.record_interaction_to_frame(started_at.elapsed());
        }
    }
}

/// Tolerant document comparison for page metadata. Chrome can omit the
/// fragment from `frameNavigated` even though `location.href` already has it,
/// and can append a trailing slash to an otherwise identical URL.
fn href_matches_url(href: &str, url: &str) -> bool {
    let href = comparable_document_url(href);
    let url = comparable_document_url(url);
    let href = href.trim_end_matches('/');
    let url = url.trim_end_matches('/');
    href == url
}

/// Chrome omits URL credentials from `location.href`, while target and
/// navigation events may retain them. Credentials do not identify a
/// different document, so remove only the authority's user-info before
/// comparing page metadata. An `@` in a path or query remains significant.
fn comparable_document_url(url: &str) -> Cow<'_, str> {
    let url = url.split_once('#').map_or(url, |(base, _)| base);
    let Some(scheme_end) = url.find("://") else {
        return Cow::Borrowed(url);
    };
    let authority_start = scheme_end + 3;
    let authority_end = url[authority_start..]
        .find(['/', '?'])
        .map_or(url.len(), |offset| authority_start + offset);
    let Some(user_info_end) = url[authority_start..authority_end].rfind('@') else {
        return Cow::Borrowed(url);
    };

    let host_start = authority_start + user_info_end + 1;
    let mut normalized = String::with_capacity(url.len() - (host_start - authority_start));
    normalized.push_str(&url[..authority_start]);
    normalized.push_str(&url[host_start..]);
    Cow::Owned(normalized)
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, mpsc};

    use crate::cdp::CdpEvent;
    use crate::frames::FrameSlot;
    use crate::session::{BrowserEvent, BrowserEventSender, BrowserEventWake, CommittedUrl};
    use crate::{BrowserConfig, session::BrowserSessionConfig};

    use super::{DriverState, attached_bound_page_target, href_matches_url, title_for_bound_target};

    fn driver_state() -> DriverState {
        DriverState::new(
            &BrowserSessionConfig {
                browser: BrowserConfig::default(),
                panel_local_id: "panel-1".to_string(),
                initial_url: None,
                width: 1280,
                height: 800,
                frame_slot: Arc::new(FrameSlot::new()),
                coordination: None,
                capture_directory: None,
            },
            "ws://127.0.0.1/devtools/browser/test",
            None,
            Arc::new(AtomicBool::new(false)),
        )
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
    fn unreachable_top_frame_keeps_last_committed_page() {
        let mut state = driver_state();
        state.url = "https://example.test/good".to_string();
        state.title = "Good page".to_string();
        state.retain_frame_during_navigation = true;
        let (tx, rx) = mpsc::channel();
        let events = BrowserEventSender {
            tx,
            wake: BrowserEventWake::default(),
            committed_url: CommittedUrl::default(),
        };
        let params = serde_json::json!({
            "frame": {
                "id": "main",
                "url": "chrome-error://chromewebdata/",
                "unreachableUrl": "http://127.0.0.1:9/unreachable"
            }
        });

        state.handle_frame_navigated(
            &events,
            CdpEvent {
                method: "Page.frameNavigated",
                params: &params,
                session_id: Some("session"),
            },
            true,
        );

        assert_eq!(state.url, "https://example.test/good");
        assert_eq!(state.title, "Good page");
        assert!(state.retain_frame_during_navigation);
        assert!(state.navigation_failed);
        assert!(matches!(rx.recv(), Ok(BrowserEvent::NavigationFailed(_))));
        assert_eq!(rx.recv(), Ok(BrowserEvent::Loading(false)));

        let unrelated = serde_json::json!({
            "frameId": "main",
            "url": "http://127.0.0.1:9/unreachable#fragment"
        });
        state.handle_same_document_navigation(
            &events,
            CdpEvent {
                method: "Page.navigatedWithinDocument",
                params: &unrelated,
                session_id: Some("session"),
            },
            true,
        );
        assert!(state.retain_frame_during_navigation);
        assert_eq!(state.url, "https://example.test/good");

        let reachable = serde_json::json!({
            "frameId": "main",
            "url": "https://example.test/good#fragment"
        });
        state.handle_same_document_navigation(
            &events,
            CdpEvent {
                method: "Page.navigatedWithinDocument",
                params: &reachable,
                session_id: Some("session"),
            },
            true,
        );
        assert!(!state.retain_frame_during_navigation);
        assert!(!state.navigation_failed);
        assert_eq!(state.url, "https://example.test/good#fragment");
    }

    #[test]
    fn same_url_challenge_commit_waits_for_a_confirmed_success_before_clearing() {
        const URL: &str = "https://example.test/protected";
        let challenge_headers = serde_json::json!({ "cf-mitigated": "challenge" });
        let mut state = driver_state();
        state.url = URL.to_string();
        state
            .challenge_loop
            .observe_document_response(URL, Some(403), &challenge_headers, Some("initial"));
        state.challenge_loop.handoff_completed();
        state
            .challenge_loop
            .observe_document_response(URL, Some(403), &challenge_headers, Some("challenge-reload"));
        assert_eq!(
            state.challenge_loop.take_rejection_after_delay_for_test(),
            Some(crate::challenge::REJECTION_MESSAGE)
        );
        let (tx, rx) = mpsc::channel();
        let events = BrowserEventSender {
            tx,
            wake: BrowserEventWake::default(),
            committed_url: CommittedUrl::default(),
        };
        let challenge_commit = serde_json::json!({
            "frame": { "id": "main", "loaderId": "challenge-reload", "url": URL }
        });

        state.handle_frame_navigated(
            &events,
            CdpEvent {
                method: "Page.frameNavigated",
                params: &challenge_commit,
                session_id: Some("session"),
            },
            true,
        );

        assert_eq!(rx.recv(), Ok(BrowserEvent::Loading(true)));
        assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty));

        state.update_url_from_page(&events, &format!("{URL}#metadata"));
        assert!(matches!(rx.recv(), Ok(BrowserEvent::UrlChanged(_))));
        assert!(matches!(rx.recv(), Ok(BrowserEvent::NavigationFailed(_))));
        let same_document = serde_json::json!({ "frameId": "main", "url": format!("{URL}#anchor") });
        state.handle_same_document_navigation(
            &events,
            CdpEvent {
                method: "Page.navigatedWithinDocument",
                params: &same_document,
                session_id: Some("session"),
            },
            true,
        );
        assert!(matches!(rx.recv(), Ok(BrowserEvent::UrlChanged(_))));
        assert!(matches!(rx.recv(), Ok(BrowserEvent::NavigationFailed(_))));

        state.challenge_loop.document_navigation_started();
        state.challenge_loop.observe_document_response(
            URL,
            Some(200),
            &serde_json::Value::Null,
            Some("successful-retry"),
        );
        assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty));
        let success_commit = serde_json::json!({
            "frame": { "id": "main", "loaderId": "successful-retry", "url": URL }
        });
        state.handle_frame_navigated(
            &events,
            CdpEvent {
                method: "Page.frameNavigated",
                params: &success_commit,
                session_id: Some("session"),
            },
            true,
        );

        assert_eq!(rx.recv(), Ok(BrowserEvent::UrlChanged(URL.to_string())));
        assert_eq!(rx.recv(), Ok(BrowserEvent::Loading(true)));
        assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty));
    }

    #[test]
    fn title_href_matching_accepts_only_the_same_document() {
        assert!(href_matches_url(
            "https://example.test/path?mode=1#section",
            "https://example.test/path?mode=1",
        ));
        assert!(href_matches_url(
            "https://example.test/path/",
            "https://example.test/path"
        ));
        assert!(href_matches_url(
            "http://127.0.0.1/page?mode=1#section",
            "http://user:secret@127.0.0.1/page?mode=1#submitted",
        ));
        assert!(href_matches_url(
            "https://example.test/path@marker?mode=1",
            "https://example.test/path@marker?mode=1#section",
        ));
        assert!(!href_matches_url(
            "https://example.test/path?mode=2",
            "https://example.test/path?mode=1",
        ));
        assert!(!href_matches_url(
            "http://127.0.0.2/page?mode=1",
            "http://user:secret@127.0.0.1/page?mode=1",
        ));
        assert!(!href_matches_url(
            "https://example.test/other",
            "https://example.test/path",
        ));
    }
}
