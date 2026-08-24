//! UI-command handling for the browser driver: navigation, viewport, input,
//! activity stamping, and stop/handoff requests.

use std::sync::{Arc, mpsc};
use std::time::Instant;

use crate::browser::cdp::CdpLink;
use crate::browser::frames::FrameSlot;
use crate::browser::manifest;

use super::{BrowserCommand, BrowserEvent, DriverState, USER_ACTIVE_STAMP_INTERVAL};

impl DriverState {
    /// Process every pending UI command. Returns `true` when `Stop` arrived.
    pub(super) fn drain_commands(
        &mut self,
        link: &mut CdpLink,
        command_rx: &mpsc::Receiver<BrowserCommand>,
        event_tx: &mpsc::Sender<BrowserEvent>,
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
                    let (method, params) = input.cdp();
                    if let Some(session) = self.session_id.clone() {
                        let _ = link.send_request(method, &params, Some(session.as_str()));
                    }
                    if is_activity {
                        self.stamp_user_active();
                    }
                }
                BrowserCommand::HandoffDone => self.resolve_handoff(),
            }
        }
        false
    }

    fn set_viewport(
        &mut self,
        link: &mut CdpLink,
        event_tx: &mpsc::Sender<BrowserEvent>,
        frame_slot: &Arc<FrameSlot>,
        width: u32,
        height: u32,
    ) {
        if width == 0 || height == 0 || (width, height) == (self.viewport_w, self.viewport_h) {
            return;
        }
        self.viewport_w = width;
        self.viewport_h = height;
        if self
            .send_page_command(
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
            )
            .is_ok()
        {
            self.pending_viewport_capture_at = Some(Instant::now() + super::VIEWPORT_CAPTURE_DELAY);
        }
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
        event_tx: &mpsc::Sender<BrowserEvent>,
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
        self.last_user_active_stamp = Some(Instant::now());
        self.write_manifest_extra(true, |manifest| {
            manifest.user_active = true;
            manifest.user_active_at = manifest::now_millis();
        });
    }
}
