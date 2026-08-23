//! CDP event handling for the driver state machine: target binding, page
//! navigation and title tracking, and screencast lifecycle.
//!
//! Split out of the driver loop ([super]) — the event arms are the
//! state machine's reactive half, while the parent module keeps the loop,
//! command drain, and manifest/signal plumbing.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use base64::Engine;

use crate::browser::cdp::{CdpEvent, CdpLink, CdpMsg};
use crate::browser::frames::FrameSlot;

use super::{BrowserEvent, DriverState};

impl DriverState {
    pub(super) fn tick_title_fetch(
        &mut self,
        link: &mut CdpLink,
        event_tx: &mpsc::Sender<BrowserEvent>,
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

    /// Best-effort current title: `Page.titleUpdated` only fires on
    /// *changes*, so a page whose title is static (or that loaded before we
    /// attached) reports none — read it directly instead.
    pub(super) fn fetch_title(
        &mut self,
        link: &mut CdpLink,
        event_tx: &mpsc::Sender<BrowserEvent>,
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
        if title.is_empty() || title == self.title {
            return;
        }
        self.title = title.to_string();
        self.manifest_dirty = true;
        self.write_manifest(false);
        let _ = event_tx.send(BrowserEvent::Title(title.to_string()));
    }

    /// Track the current document's default execution context for the
    /// title fetch. A creation with `auxData.isDefault` replaces the
    /// tracked id; a destruction of the tracked id clears it (the next
    /// fetch then runs without an explicit contextId).
    fn note_execution_context(&mut self, event: &CdpEvent<'_>) {
        let context = event.params.get("context");
        let is_default = context
            .and_then(|c| c.pointer("/auxData/isDefault"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if is_default && let Some(id) = context.and_then(|c| c.get("id")).and_then(serde_json::Value::as_u64) {
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
        event_tx: &mpsc::Sender<BrowserEvent>,
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
        if self.reattach_in_flight {
            self.reattach_in_flight = false;
            match error {
                Some(error) => {
                    tracing::warn!(target: "browser", "re-attach rejected: {error}");
                    self.pending_restart_at = Some(Instant::now() + self.restart_backoff());
                }
                None => {
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
                }
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
        event_tx: &mpsc::Sender<BrowserEvent>,
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
                let target_info = event.params.get("targetInfo");
                let target_type = target_info
                    .and_then(|t| t.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                let Some(target_id) = target_info.and_then(|t| t.get("targetId")).and_then(|t| t.as_str()) else {
                    return;
                };
                if target_type != "page" {
                    return;
                }
                self.attach_setup(link, event_tx, frame_slot, session, target_id);
            }
            "Target.detachedFromTarget" => {
                if on_page_session {
                    self.session_id = None;
                    self.screencast_on = false;
                    // Drop the tracked context: the next title fetch must
                    // use a fresh default context, not a dead contextId.
                    self.title_context_id = None;
                    self.title_fetch_at = None;
                    self.title_fetch_retries = 0;
                }
            }
            "Target.targetDestroyed" => {
                let destroyed = event.params.get("targetId").and_then(|t| t.as_str());
                if self.target_id.as_deref() == destroyed {
                    self.session_id = None;
                    self.target_id = None;
                    self.screencast_on = false;
                }
            }
            "Page.frameNavigated" => {
                if !on_page_session {
                    return;
                }
                // Subframe (iframe) navigations carry `frame.parentId`;
                // only the top frame defines the panel's URL.
                let Some(frame) = event.params.get("frame") else {
                    return;
                };
                if frame.get("parentId").is_some() {
                    return;
                }
                if let Some(url) = frame.get("url").and_then(|u| u.as_str())
                    && !url.is_empty()
                    && url != self.url
                {
                    self.url = url.to_string();
                    self.manifest_dirty = true;
                }
                self.pending_restart_at = Some(Instant::now());
                self.write_manifest(true);
                let _ = event_tx.send(BrowserEvent::UrlChanged(self.url.clone()));
                let _ = event_tx.send(BrowserEvent::Loading(true));
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
            "Page.titleUpdated" => {
                if !on_page_session {
                    return;
                }
                if let Some(title) = event.params.get("title").and_then(|t| t.as_str())
                    && !title.is_empty()
                    && title != self.title
                {
                    self.title = title.to_string();
                    self.manifest_dirty = true;
                    self.write_manifest(false);
                    let _ = event_tx.send(BrowserEvent::Title(title.to_string()));
                }
            }
            "Page.screencastFrame" => Self::handle_screencast_frame(link, event_tx, frame_slot, event),
            _ => {}
        }
    }

    /// Decode, store, and ack one screencast frame.
    fn handle_screencast_frame(
        link: &mut CdpLink,
        event_tx: &mpsc::Sender<BrowserEvent>,
        frame_slot: &FrameSlot,
        event: CdpEvent<'_>,
    ) {
        let Some(data) = event.params.get("data").and_then(|d| d.as_str()) else {
            return;
        };
        let Ok(jpeg) = base64::engine::general_purpose::STANDARD.decode(data) else {
            return;
        };
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
        if let Some(seq) = frame_slot.store_jpeg(&jpeg) {
            let _ = event_tx.send(BrowserEvent::Frame { seq });
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
