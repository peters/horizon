//! UI-command handling for the browser driver: navigation, viewport, input,
//! activity stamping, and stop/handoff requests.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::cdp::CdpLink;
use crate::frames::FrameSlot;
use crate::{AgentAction, BrowserAuditStatus, BrowserButton, BrowserControlFailure, BrowserInput};

use super::{
    BrowserCommand, BrowserEventSender, CommandReceiver, DriverState, SCROLLBAR_LAYOUT_RETRY_DELAY,
    VIEWPORT_CAPTURE_DELAY, VIEWPORT_RETRY_DELAY, VerticalScrollbarLayout,
};

impl DriverState {
    /// Process every pending UI command. Returns `true` when `Stop` arrived.
    pub(super) fn drain_commands(
        &mut self,
        link: &mut CdpLink,
        command_rx: &CommandReceiver,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
    ) -> bool {
        if self.stop_requested.load(Ordering::Acquire) {
            return true;
        }
        let batch = command_rx.drain(256);
        for command in batch.commands {
            self.audit_user_command(&command);
            if self.dispatch_command(link, event_tx, frame_slot, command, true) {
                return true;
            }
        }
        // The last sender dropped without sending Stop: no one is left to
        // service, so stop instead of keeping the browser and profile alive.
        batch.disconnected
    }

    pub(super) fn drain_agent_actions(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        actions: Vec<AgentAction>,
    ) -> bool {
        for request in actions {
            if let Err(message) = request.action.validate() {
                self.audit_agent_action(&request, BrowserAuditStatus::Rejected);
                self.complete_agent_action(&request, Err(BrowserControlFailure::new("invalid_input", message)));
                continue;
            }
            self.audit_agent_action(&request, BrowserAuditStatus::Dispatched);
            let (result, stop) = self.execute_agent_action(link, event_tx, frame_slot, &request);
            self.complete_agent_action(&request, result);
            if stop {
                return true;
            }
        }
        false
    }

    pub(super) fn dispatch_command(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        command: BrowserCommand,
        user_origin: bool,
    ) -> bool {
        if user_origin && command.is_user_activity() {
            self.stamp_user_active();
        }
        if matches!(
            &command,
            BrowserCommand::Navigate(_)
                | BrowserCommand::Reload
                | BrowserCommand::Back
                | BrowserCommand::Forward
                | BrowserCommand::SetViewport { .. }
                | BrowserCommand::Input(_)
        ) {
            self.interaction_started_at.get_or_insert_with(Instant::now);
        }
        if !matches!(command, BrowserCommand::Input(_)) {
            self.vertical_scrollbar_drag = None;
        }
        match command {
            BrowserCommand::Stop => return true,
            BrowserCommand::Navigate(url) => {
                self.invalidate_scrollbar_layout();
                self.navigate_to(link, event_tx, frame_slot, &url);
            }
            BrowserCommand::Reload => {
                self.invalidate_scrollbar_layout();
                self.pending_restart_at = Some(Instant::now());
                let _ = self.send_page_command(link, event_tx, frame_slot, "Page.reload", &serde_json::json!({}));
            }
            BrowserCommand::Back => {
                self.invalidate_scrollbar_layout();
                self.navigate_history(link, event_tx, frame_slot, -1);
            }
            BrowserCommand::Forward => {
                self.invalidate_scrollbar_layout();
                self.navigate_history(link, event_tx, frame_slot, 1);
            }
            BrowserCommand::SetViewport { width, height } => {
                self.vertical_scrollbar_drag = None;
                self.set_viewport(link, event_tx, frame_slot, width, height);
            }
            BrowserCommand::Input(input) => {
                if self.handle_vertical_scrollbar_input(link, &input) {
                    return false;
                }
                // Input cannot block on a roundtrip: a detaching session
                // would otherwise stall every frame for the call timeout.
                if input.copies_selection() {
                    self.request_clipboard_text(link);
                }
                let refresh_scrollbar_layout = matches!(input, BrowserInput::Wheel { .. });
                let (method, params) = input.cdp();
                if let Some(session) = self.session_id.clone() {
                    let _ = link.send_request(method, &params, Some(session.as_str()));
                }
                if refresh_scrollbar_layout {
                    self.schedule_scrollbar_layout_refresh(SCROLLBAR_LAYOUT_RETRY_DELAY);
                }
            }
            BrowserCommand::HandoffDone => self.resolve_handoff(event_tx),
        }
        false
    }

    fn handle_vertical_scrollbar_input(&mut self, link: &mut CdpLink, input: &BrowserInput) -> bool {
        match input {
            BrowserInput::MousePress {
                x,
                y,
                button: BrowserButton::Left,
                ..
            } => {
                self.vertical_scrollbar_drag = None;
                // Avoid a synchronous layout roundtrip for ordinary page
                // clicks. Native scrollbars are never wider than this gate;
                // the authoritative layout metrics below make the final hit
                // decision and protect right-aligned page content.
                if *x < f64::from(self.viewport_w.saturating_sub(32)) {
                    return false;
                }
                self.schedule_scrollbar_layout_refresh(Duration::ZERO);
                let Some(layout) = self.scrollbar_layout.layout else {
                    return false;
                };
                match vertical_scrollbar_press(layout, f64::from(self.viewport_w), *x, *y) {
                    Some(ScrollbarPress::Drag(drag)) => {
                        self.vertical_scrollbar_drag = Some(drag);
                    }
                    Some(ScrollbarPress::PageTo(target)) => {
                        self.scroll_page_to(link, target);
                    }
                    None => return false,
                }
                true
            }
            BrowserInput::MouseMove { y, buttons, .. }
                if self.vertical_scrollbar_drag.is_some() && buttons & 1 != 0 =>
            {
                let target = self
                    .vertical_scrollbar_drag
                    .map(|drag| drag.target_scroll_y(*y))
                    .unwrap_or_default();
                self.scroll_page_to(link, target);
                true
            }
            BrowserInput::MouseRelease {
                y,
                button: BrowserButton::Left,
                ..
            } if self.vertical_scrollbar_drag.is_some() => {
                let drag = self.vertical_scrollbar_drag.take();
                let target = drag.map(|drag| drag.target_scroll_y(*y)).unwrap_or_default();
                self.scroll_page_to(link, target);
                true
            }
            _ => false,
        }
    }

    fn scroll_page_to(&mut self, link: &mut CdpLink, target: f64) {
        let expression = format!("window.scrollTo(window.scrollX, {target:.3})");
        let Some(session) = self.session_id.clone() else {
            return;
        };
        if link
            .send_request(
                "Runtime.evaluate",
                &serde_json::json!({
                    "expression": expression,
                    "returnByValue": false,
                    "userGesture": true,
                }),
                Some(session.as_str()),
            )
            .is_ok()
        {
            if let Some(layout) = self.scrollbar_layout.layout.as_mut() {
                layout.scroll_y = target.clamp(0.0, layout.content_height - layout.client_height);
            }
            self.schedule_scrollbar_layout_refresh(SCROLLBAR_LAYOUT_RETRY_DELAY);
        }
    }

    pub(super) fn invalidate_scrollbar_layout(&mut self) {
        self.scrollbar_layout.layout = None;
        // A detached page session is not guaranteed to answer its outstanding
        // request. CDP ids are connection-global, so a late reply cannot be
        // mistaken for the replacement request after this slot is cleared.
        self.scrollbar_layout.request_id = None;
        self.scrollbar_layout.refresh_at = Some(Instant::now());
    }

    fn schedule_scrollbar_layout_refresh(&mut self, delay: Duration) {
        let due = Instant::now() + delay;
        self.scrollbar_layout.refresh_at = Some(
            self.scrollbar_layout
                .refresh_at
                .map_or(due, |scheduled| scheduled.min(due)),
        );
    }

    pub(super) fn tick_scrollbar_layout(&mut self, link: &mut CdpLink) {
        if self.scrollbar_layout.request_id.is_some() {
            return;
        }
        let Some(due) = self.scrollbar_layout.refresh_at else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        let Some(session) = self.session_id.clone() else {
            return;
        };
        match link.send_request("Page.getLayoutMetrics", &serde_json::json!({}), Some(session.as_str())) {
            Ok(id) => {
                self.scrollbar_layout.request_id = Some(id);
                self.scrollbar_layout.refresh_at = None;
            }
            Err(error) => {
                self.scrollbar_layout.refresh_at = Some(Instant::now() + SCROLLBAR_LAYOUT_RETRY_DELAY);
                tracing::debug!(target: "browser", "scrollbar layout request failed: {error}");
            }
        }
    }

    pub(super) fn handle_scrollbar_layout_response(
        &mut self,
        id: u64,
        result: Option<&serde_json::Value>,
        rejected: bool,
    ) -> bool {
        if self.scrollbar_layout.request_id != Some(id) {
            return false;
        }
        self.scrollbar_layout.request_id = None;
        if rejected {
            self.scrollbar_layout.layout = None;
            self.schedule_scrollbar_layout_refresh(SCROLLBAR_LAYOUT_RETRY_DELAY);
            return true;
        }
        let Some(layout) = result.and_then(VerticalScrollbarLayout::from_metrics) else {
            self.scrollbar_layout.layout = None;
            self.schedule_scrollbar_layout_refresh(SCROLLBAR_LAYOUT_RETRY_DELAY);
            return true;
        };
        self.scrollbar_layout.layout = Some(layout);
        true
    }

    pub(super) fn note_screencast_scroll_offset(&mut self, scroll_y: Option<f64>) {
        let Some(scroll_y) = scroll_y.filter(|value| value.is_finite() && *value >= 0.0) else {
            return;
        };
        if let Some(layout) = self.scrollbar_layout.layout.as_mut() {
            layout.scroll_y = scroll_y.clamp(0.0, layout.content_height - layout.client_height);
        } else {
            self.schedule_scrollbar_layout_refresh(Duration::ZERO);
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
            // The UI only resends an unchanged target while its latest frame
            // still has different dimensions. Reapply rather than trusting
            // cached protocol state: an acknowledged resize can race with
            // navigation/reattach and leave the actual page at the old size.
            self.pending_viewport = Some((width, height));
            self.viewport_retry_at = None;
            return true;
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
                "screenWidth": width,
                "screenHeight": height,
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
        self.invalidate_scrollbar_layout();
    }

    /// Chrome sometimes applies device metrics without emitting a new
    /// screencast frame. Capture exactly once after a resize burst settles;
    /// the asynchronous response is published by `handle_message`.
    pub(super) fn tick_viewport_capture(&mut self, link: &mut CdpLink, frame_slot: &FrameSlot) {
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
            Ok(request_id) => {
                frame_slot.record_capture_request();
                self.viewport_capture_request_id = Some(request_id);
            }
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
}

enum ScrollbarPress {
    Drag(super::VerticalScrollbarDrag),
    PageTo(f64),
}

fn vertical_scrollbar_press(
    layout: VerticalScrollbarLayout,
    viewport_width: f64,
    x: f64,
    y: f64,
) -> Option<ScrollbarPress> {
    let overlay = layout.client_width >= viewport_width;
    let scrollbar_left = if overlay {
        (viewport_width - 8.0).max(0.0)
    } else {
        layout.client_width
    };
    if layout.content_height <= layout.client_height
        || x < scrollbar_left
        || x > viewport_width
        || y < 0.0
        || y > layout.client_height
    {
        return None;
    }

    let max_scroll = layout.content_height - layout.client_height;
    let thumb_height =
        (layout.client_height * layout.client_height / layout.content_height).clamp(18.0, layout.client_height);
    let thumb_travel = layout.client_height - thumb_height;
    let thumb_top = if max_scroll > 0.0 {
        layout.scroll_y.clamp(0.0, max_scroll) / max_scroll * thumb_travel
    } else {
        0.0
    };
    if y >= thumb_top - 2.0 && y <= thumb_top + thumb_height + 2.0 {
        return Some(ScrollbarPress::Drag(super::VerticalScrollbarDrag {
            pointer_y: y,
            scroll_y: layout.scroll_y,
            max_scroll,
            scroll_per_pointer_pixel: max_scroll / thumb_travel.max(1.0),
        }));
    }
    if overlay {
        return None;
    }

    let direction = if y < thumb_top { -1.0 } else { 1.0 };
    Some(ScrollbarPress::PageTo(
        (layout.scroll_y + (direction * layout.client_height)).clamp(0.0, max_scroll),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use crate::frames::FrameSlot;
    use crate::{BrowserConfig, session::BrowserSessionConfig};

    use super::{DriverState, ScrollbarPress, VerticalScrollbarLayout, vertical_scrollbar_press};

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
            },
            "ws://127.0.0.1/devtools/browser/test",
            None,
            Arc::new(AtomicBool::new(false)),
        )
    }

    fn scrollbar_layout(metrics: &serde_json::Value) -> VerticalScrollbarLayout {
        let Some(layout) = VerticalScrollbarLayout::from_metrics(metrics) else {
            panic!("test metrics should describe a valid layout");
        };
        layout
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

    #[test]
    fn repeated_authoritative_viewport_is_reapplied_for_visual_convergence() {
        let mut state = driver_state();

        assert!(state.queue_viewport(1280, 800));
        assert_eq!(state.pending_viewport, Some((1280, 800)));
    }

    #[test]
    fn asynchronous_scrollbar_layout_response_populates_the_hit_test_cache() {
        let mut state = driver_state();
        state.scrollbar_layout.request_id = Some(41);
        state.scrollbar_layout.refresh_at = None;
        let metrics = serde_json::json!({
            "cssLayoutViewport": {
                "pageY": 120,
                "clientWidth": 1149,
                "clientHeight": 608
            },
            "cssContentSize": { "width": 1149, "height": 3000 }
        });

        assert!(state.handle_scrollbar_layout_response(41, Some(&metrics), false));
        let Some(layout) = state.scrollbar_layout.layout else {
            panic!("valid asynchronous metrics should populate the cache");
        };
        assert!((layout.scroll_y - 120.0).abs() < f64::EPSILON);
        assert_eq!(state.scrollbar_layout.request_id, None);
        assert_eq!(state.scrollbar_layout.refresh_at, None);
    }

    #[test]
    fn invalidation_abandons_an_inflight_scrollbar_layout_request() {
        let mut state = driver_state();
        state.scrollbar_layout.request_id = Some(41);
        state.scrollbar_layout.layout = Some(VerticalScrollbarLayout {
            client_width: 1149.0,
            client_height: 608.0,
            scroll_y: 0.0,
            content_height: 3000.0,
        });

        state.invalidate_scrollbar_layout();

        assert_eq!(state.scrollbar_layout.request_id, None);
        assert!(state.scrollbar_layout.layout.is_none());
        assert!(state.scrollbar_layout.refresh_at.is_some());
        assert!(!state.handle_scrollbar_layout_response(41, None, false));
    }

    #[test]
    fn native_scrollbar_thumb_drag_maps_pointer_travel_to_page_scroll() {
        let metrics = serde_json::json!({
            "cssLayoutViewport": {
                "pageY": 0,
                "clientWidth": 1149,
                "clientHeight": 608
            },
            "cssContentSize": { "width": 1149, "height": 3000 }
        });
        let press = vertical_scrollbar_press(scrollbar_layout(&metrics), 1164.0, 1155.0, 72.0);
        let Some(ScrollbarPress::Drag(drag)) = press else {
            panic!("visible scrollbar thumb should start a drag");
        };

        let target = drag.target_scroll_y(361.0);
        assert!((target - 1_426.0).abs() < 1.0);
    }

    #[test]
    fn overlay_scrollbar_only_claims_its_thumb() {
        let metrics = serde_json::json!({
            "cssLayoutViewport": {
                "pageY": 640,
                "clientWidth": 684,
                "clientHeight": 508
            },
            "cssContentSize": { "width": 684, "height": 2618 }
        });
        let layout = scrollbar_layout(&metrics);

        assert!(matches!(
            vertical_scrollbar_press(layout, 684.0, 676.0, 128.0),
            Some(ScrollbarPress::Drag(_))
        ));
        assert!(vertical_scrollbar_press(layout, 684.0, 676.0, 300.0).is_none());
        assert!(vertical_scrollbar_press(layout, 684.0, 675.0, 128.0).is_none());
    }

    #[test]
    fn native_scrollbar_track_click_pages_without_stealing_content_clicks() {
        let metrics = serde_json::json!({
            "cssLayoutViewport": {
                "pageY": 0,
                "clientWidth": 1149,
                "clientHeight": 608
            },
            "cssContentSize": { "width": 1149, "height": 3000 }
        });

        let layout = scrollbar_layout(&metrics);
        let track = vertical_scrollbar_press(layout, 1164.0, 1155.0, 300.0);
        assert!(matches!(track, Some(ScrollbarPress::PageTo(target)) if (target - 608.0).abs() < f64::EPSILON));
        assert!(vertical_scrollbar_press(layout, 1164.0, 1148.0, 300.0).is_none());
        assert!(vertical_scrollbar_press(layout, 1149.0, 1148.0, 300.0).is_none());
    }

    #[test]
    fn minimum_height_scrollbar_thumb_still_reaches_the_page_end() {
        let metrics = serde_json::json!({
            "cssLayoutViewport": {
                "pageY": 0,
                "clientWidth": 1149,
                "clientHeight": 608
            },
            "cssContentSize": { "width": 1149, "height": 100_000 }
        });
        let press = vertical_scrollbar_press(scrollbar_layout(&metrics), 1164.0, 1155.0, 9.0);
        let Some(ScrollbarPress::Drag(drag)) = press else {
            panic!("minimum-height scrollbar thumb should start a drag");
        };

        assert!((drag.target_scroll_y(599.0) - 99_392.0).abs() < 1.0);
    }
}
