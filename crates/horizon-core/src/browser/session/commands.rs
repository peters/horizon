//! UI-command handling for the browser driver: navigation, viewport, input,
//! activity stamping, and stop/handoff requests.

use std::sync::{Arc, mpsc};
use std::time::Instant;

use crate::browser::cdp::CdpLink;
use crate::browser::frames::FrameSlot;
use crate::browser::manifest;

use super::{
    BrowserCommand, BrowserEventSender, DriverState, USER_ACTIVE_STAMP_INTERVAL, VIEWPORT_CAPTURE_DELAY,
    VIEWPORT_RETRY_DELAY,
};

const SELECTED_TEXT_EXPRESSION: &str = r#"(() => {
  const selectedText = (document) => {
    const active = document.activeElement;
    if (active?.tagName === "IFRAME") {
      try {
        const nested = active.contentDocument && selectedText(active.contentDocument);
        if (nested) return nested;
      } catch (_) {}
    }
    if (active && active.type !== "password" && typeof active.value === "string"
        && Number.isInteger(active.selectionStart) && Number.isInteger(active.selectionEnd)) {
      return active.value.slice(active.selectionStart, active.selectionEnd);
    }
    return document.getSelection()?.toString() || "";
  };
  return selectedText(document);
})()"#;

impl DriverState {
    /// Process every pending UI command. Returns `true` when `Stop` arrived.
    pub(super) fn drain_commands(
        &mut self,
        link: &mut CdpLink,
        command_rx: &mpsc::Receiver<BrowserCommand>,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
    ) -> bool {
        while let Ok(command) = command_rx.try_recv() {
            match command {
                BrowserCommand::Stop => return true,
                BrowserCommand::Navigate(url) => self.navigate_to(link, event_tx, frame_slot, &url),
                BrowserCommand::Reload => {
                    self.pending_restart_at = Some(Instant::now());
                    let _ = self.send_page_command(link, event_tx, frame_slot, "Page.reload", &serde_json::json!({}));
                }
                BrowserCommand::Back => self.navigate_history(link, event_tx, frame_slot, -1),
                BrowserCommand::Forward => self.navigate_history(link, event_tx, frame_slot, 1),
                BrowserCommand::SetViewport { width, height } => {
                    self.set_viewport(link, event_tx, frame_slot, width, height);
                }
                BrowserCommand::Input(input) => {
                    // Input cannot block on a roundtrip: a detaching session
                    // would otherwise stall every frame for the call timeout.
                    let is_activity = input.is_activity();
                    if input.copies_selection() {
                        self.request_clipboard_text(link);
                    }
                    let (method, params) = input.cdp();
                    if let Some(session) = self.session_id.clone() {
                        let _ = link.send_request(method, &params, Some(session.as_str()));
                    }
                    if is_activity {
                        self.stamp_user_active();
                    }
                }
                BrowserCommand::HandoffDone => self.resolve_handoff(event_tx),
            }
        }
        false
    }

    fn request_clipboard_text(&mut self, link: &mut CdpLink) {
        let Some(session) = self.session_id.clone() else {
            return;
        };
        match link.send_request(
            "Runtime.evaluate",
            &serde_json::json!({
                "expression": SELECTED_TEXT_EXPRESSION,
                "returnByValue": true,
            }),
            Some(session.as_str()),
        ) {
            Ok(request_id) => {
                self.clipboard_read_request_ids.insert(request_id);
            }
            Err(error) => tracing::debug!(target: "browser", "clipboard selection capture failed: {error}"),
        }
    }

    fn set_viewport(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        width: u32,
        height: u32,
    ) {
        if !self.queue_viewport(width, height) {
            return;
        }
        self.apply_pending_viewport(link, event_tx, frame_slot);
    }

    fn queue_viewport(&mut self, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        if (width, height) == (self.viewport_w, self.viewport_h) {
            self.pending_viewport = None;
            self.viewport_retry_at = None;
            return false;
        }
        self.pending_viewport = Some((width, height));
        self.viewport_retry_at = None;
        true
    }

    pub(super) fn tick_viewport_resize(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
    ) {
        if self.pending_viewport.is_none() || self.viewport_retry_at.is_some_and(|retry_at| Instant::now() < retry_at) {
            return;
        }
        self.apply_pending_viewport(link, event_tx, frame_slot);
    }

    fn apply_pending_viewport(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
    ) {
        let Some((width, height)) = self.pending_viewport else {
            return;
        };
        match self.send_page_command(
            link,
            event_tx,
            frame_slot,
            "Emulation.setDeviceMetricsOverride",
            &serde_json::json!({
                "width": width,
                "height": height,
                "deviceScaleFactor": 1,
                "mobile": false,
            }),
        ) {
            Ok(_) => {
                self.commit_viewport(width, height);
                self.pending_viewport_capture_at = Some(Instant::now() + VIEWPORT_CAPTURE_DELAY);
            }
            Err(error) => {
                self.viewport_retry_at = Some(Instant::now() + VIEWPORT_RETRY_DELAY);
                tracing::debug!(
                    target: "browser",
                    width,
                    height,
                    "viewport resize failed and will retry: {error}"
                );
            }
        }
    }

    fn commit_viewport(&mut self, width: u32, height: u32) {
        self.viewport_w = width;
        self.viewport_h = height;
        self.pending_viewport = None;
        self.viewport_retry_at = None;
    }

    /// Chrome sometimes applies device metrics without emitting a new
    /// screencast frame. Capture exactly once after a resize burst settles;
    /// the asynchronous response is published by `handle_message`.
    pub(super) fn tick_viewport_capture(&mut self, link: &mut CdpLink) {
        if self.viewport_capture_request_id.is_some() {
            return;
        }
        let Some(due) = self.pending_viewport_capture_at else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        self.pending_viewport_capture_at = None;
        let Some(session) = self.session_id.clone() else {
            return;
        };
        match link.send_request(
            "Page.captureScreenshot",
            &serde_json::json!({
                "format": "jpeg",
                "quality": self.config.browser.quality,
                "fromSurface": true,
                "captureBeyondViewport": false,
            }),
            Some(session.as_str()),
        ) {
            Ok(request_id) => self.viewport_capture_request_id = Some(request_id),
            Err(error) => tracing::debug!(target: "browser", "viewport frame capture failed: {error}"),
        }
    }

    /// `History.back`/`forward` are JavaScript, not CDP: step through the
    /// page's navigation history explicitly instead.
    fn navigate_history(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        delta: i64,
    ) {
        let Some(session) = self.session_id.clone() else {
            return;
        };
        let Ok(history) = self.call_and_ack(
            link,
            event_tx,
            frame_slot,
            "Page.getNavigationHistory",
            &serde_json::json!({}),
            Some(session.as_str()),
        ) else {
            return;
        };
        let Some(current) = history.get("currentIndex").and_then(serde_json::Value::as_i64) else {
            return;
        };
        let Some(entries) = history.get("entries").and_then(serde_json::Value::as_array) else {
            return;
        };
        let Some(last) = i64::try_from(entries.len()).ok().and_then(|len| len.checked_sub(1)) else {
            return;
        };
        let target = (current + delta).clamp(0, last);
        if target == current {
            return;
        }
        let entry_index = usize::try_from(target).unwrap_or(0);
        // Chrome returns `id`; the request calls the same value `entryId`.
        let entry_value = &entries[entry_index];
        let Some(entry_id) = entry_value
            .get("entryId")
            .or_else(|| entry_value.get("id"))
            .and_then(serde_json::Value::as_i64)
        else {
            return;
        };
        self.pending_restart_at = Some(Instant::now());
        let _ = self.send_page_command(
            link,
            event_tx,
            frame_slot,
            "Page.navigateToHistoryEntry",
            &serde_json::json!({ "entryId": entry_id }),
        );
    }

    /// Stamp the manifest so an agent can tell the user is driving right now.
    fn stamp_user_active(&mut self) {
        if self
            .last_user_active_stamp
            .is_some_and(|last_stamp| last_stamp.elapsed() < USER_ACTIVE_STAMP_INTERVAL)
        {
            return;
        }
        let stamped_at = Instant::now();
        match self.write_manifest_extra(true, |manifest| {
            manifest.user_active = true;
            manifest.user_active_at = manifest::now_millis();
        }) {
            Ok(()) => self.last_user_active_stamp = Some(stamped_at),
            Err(error) => tracing::warn!(target: "browser", "failed to stamp user activity: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use crate::browser::frames::FrameSlot;
    use crate::browser::{BrowserConfig, session::BrowserSessionConfig};

    use super::DriverState;

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
    fn viewport_resize_becomes_authoritative_only_after_acknowledgement() {
        let mut state = driver_state();

        assert!(state.queue_viewport(900, 600));
        assert_eq!((state.viewport_w, state.viewport_h), (1280, 800));
        assert_eq!(state.pending_viewport, Some((900, 600)));

        state.commit_viewport(900, 600);

        assert_eq!((state.viewport_w, state.viewport_h), (900, 600));
        assert_eq!(state.pending_viewport, None);
    }

    #[test]
    fn latest_viewport_request_replaces_a_pending_retry() {
        let mut state = driver_state();

        assert!(state.queue_viewport(900, 600));
        assert!(state.queue_viewport(720, 480));

        assert_eq!((state.viewport_w, state.viewport_h), (1280, 800));
        assert_eq!(state.pending_viewport, Some((720, 480)));
    }
}
