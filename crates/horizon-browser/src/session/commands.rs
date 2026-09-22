//! UI-command handling for the browser driver: navigation, viewport, input,
//! activity stamping, and stop/handoff requests.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::shared::DriverProcess;
use crate::cdp::CdpLink;
use crate::frames::FrameSlot;
use crate::input::{BrowserInputCdpExt, is_user_activity};
use crate::page_scroll::VerticalScrollbarPress;
use crate::{AgentAction, BrowserAuditStatus, BrowserButton, BrowserControlFailure, BrowserInput, PageScrollState};

use crate::navigation::AgentActionExecution;

use super::{
    BrowserCommand, BrowserEventSender, CommandReceiver, DriverState, SCROLLBAR_LAYOUT_RETRY_DELAY,
    VIEWPORT_CAPTURE_DELAY, VIEWPORT_RETRY_DELAY,
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
            if self
                .dispatch_command(link, event_tx, frame_slot, command, true)
                .is_ok_and(|stop| stop)
            {
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
        chrome: &mut DriverProcess,
    ) -> bool {
        for request in actions {
            if frame_slot.file_chooser().blocks(&request.action) {
                self.audit_agent_action(&request, BrowserAuditStatus::Rejected);
                self.complete_agent_action(
                    &request,
                    Err(BrowserControlFailure::new(
                        "file_chooser_pending",
                        "Select or cancel files in the Horizon dialog before changing the page",
                    )),
                );
                continue;
            }
            // A blocking action later in the batch must not delay the typed
            // timeout of a navigation or wait dispatched earlier in it.
            self.tick_pending_navigation();
            self.tick_pending_wait(link, event_tx, frame_slot, chrome);
            if let Err(message) = request.action.validate() {
                self.audit_agent_action(&request, BrowserAuditStatus::Rejected);
                self.complete_agent_action(&request, Err(BrowserControlFailure::new("invalid_input", message)));
                continue;
            }
            self.audit_agent_action(&request, BrowserAuditStatus::Dispatched);
            if matches!(request.action, crate::BrowserControlAction::Resize { .. }) {
                self.begin_resize(&request, frame_slot);
                continue;
            }
            if matches!(request.action, crate::BrowserControlAction::Navigate { .. }) {
                // Navigation settles from page events; a dispatch-only wait
                // completes here, everything else stays pending.
                if let AgentActionExecution::Done(result) =
                    self.begin_agent_navigation(link, event_tx, frame_slot, &request)
                {
                    self.complete_agent_action(&request, result);
                }
                continue;
            }
            if matches!(request.action, crate::BrowserControlAction::WaitForSelector { .. }) {
                // Waits are observed from the driver loop; only a wait that
                // is already satisfied (or invalid) completes here.
                if let AgentActionExecution::Done(result) =
                    self.begin_agent_wait(link, event_tx, frame_slot, &request, chrome)
                {
                    self.complete_agent_action(&request, result);
                }
                continue;
            }
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
    ) -> Result<bool, BrowserControlFailure> {
        if user_origin && is_user_activity(&command) {
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
                | BrowserCommand::NativeSelectChoose { .. }
                | BrowserCommand::NativeSelectDismiss
        ) {
            self.interaction_started_at.get_or_insert_with(Instant::now);
        }
        if !matches!(command, BrowserCommand::Input(_)) {
            self.vertical_scrollbar_drag = None;
        }
        match command {
            BrowserCommand::Stop => Ok(true),
            BrowserCommand::Navigate(url) => {
                self.invalidate_scrollbar_layout(event_tx);
                // The user (or a non-agent caller) took over the page: a
                // pending agent navigation can no longer claim the outcome.
                self.supersede_pending_navigation(Instant::now());
                self.navigate_to(link, event_tx, frame_slot, &url)?;
                Ok(false)
            }
            BrowserCommand::Reload => {
                self.invalidate_scrollbar_layout(event_tx);
                self.supersede_pending_navigation(Instant::now());
                // Marked before dispatch: events routed during the command's
                // round trip (a fast reload's commit or stop) may already
                // clear it, and must not be overwritten afterwards.
                self.top_frame_navigating = true;
                match self.send_page_command(link, event_tx, frame_slot, "Page.reload", &serde_json::json!({})) {
                    Ok(_) => {
                        self.pending_restart_at = Some(Instant::now());
                        Ok(false)
                    }
                    Err(error) => {
                        self.top_frame_navigating = false;
                        Err(self.page_command_failure(event_tx, "reload", "protocol_error", error.to_string()))
                    }
                }
            }
            BrowserCommand::Back => {
                self.invalidate_scrollbar_layout(event_tx);
                self.navigate_history(link, event_tx, frame_slot, -1)?;
                Ok(false)
            }
            BrowserCommand::Forward => {
                self.invalidate_scrollbar_layout(event_tx);
                self.navigate_history(link, event_tx, frame_slot, 1)?;
                Ok(false)
            }
            BrowserCommand::SetViewport { width, height } => {
                self.vertical_scrollbar_drag = None;
                self.set_viewport(link, event_tx, frame_slot, width, height);
                Ok(false)
            }
            BrowserCommand::Input(input) => self.dispatch_page_input(link, event_tx, frame_slot, input),
            BrowserCommand::NativeSelectChoose { index } => {
                self.apply_native_select_choice(link, event_tx, frame_slot, index);
                Ok(false)
            }
            BrowserCommand::NativeSelectDismiss => {
                self.dismiss_native_select(event_tx);
                self.send_escape_to_page(link);
                Ok(false)
            }
            BrowserCommand::HandoffDone => {
                self.resolve_handoff(event_tx);
                Ok(false)
            }
            BrowserCommand::Video { operation, options } => {
                if let Err(error) = self.video_action(frame_slot, &crate::new_action_id(), operation, options.as_ref())
                {
                    let _ = event_tx.send(super::BrowserEvent::VideoFailed(format!(
                        "{}: {}",
                        error.code, error.message
                    )));
                }
                Ok(false)
            }
        }
    }

    fn dispatch_page_input(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        input: BrowserInput,
    ) -> Result<bool, BrowserControlFailure> {
        if self.handle_vertical_scrollbar_input(link, event_tx, frame_slot, &input) {
            return Ok(false);
        }
        let teach_capture = if frame_slot.teach_recording() {
            self.semantic.teach_capture_point(&input)
        } else {
            None
        };
        if input.copies_selection() {
            self.request_clipboard_text(link);
        }
        let refresh_scrollbar_layout = matches!(input, BrowserInput::Wheel { .. });
        let probe_input = input.clone();
        let (method, params) = input.cdp();
        let session = self.session_id.clone().ok_or_else(|| {
            BrowserControlFailure::new("browser_unavailable", "the Chromium page session is not attached")
        })?;
        if let Err(error) = link.send_request(method, &params, Some(session.as_str())) {
            if teach_capture.is_some() {
                frame_slot.store_teach_failure("input_failed", &error.to_string(), frame_slot.teach_generation());
            }
            return Err(BrowserControlFailure::new("input_failed", error.to_string()));
        }
        self.note_native_select_input(link, event_tx, frame_slot, &probe_input);
        if let Some(capture) = teach_capture
            && let Some(point) = capture.point()
            && let Err(error) = self.capture_teach_fingerprint(link, event_tx, frame_slot, Some(point))
        {
            tracing::warn!(
                target: "browser",
                "teach fingerprint failed: {}",
                error.message
            );
        }
        if refresh_scrollbar_layout {
            self.schedule_scrollbar_layout_refresh(SCROLLBAR_LAYOUT_RETRY_DELAY);
        }
        Ok(false)
    }

    fn page_command_failure(
        &mut self,
        event_tx: &BrowserEventSender,
        action: &str,
        code: &str,
        message: String,
    ) -> BrowserControlFailure {
        self.interaction_started_at = None;
        let _ = event_tx.send(super::BrowserEvent::NavigationFailed(format!(
            "{action} failed: {message}"
        )));
        let _ = event_tx.send(super::BrowserEvent::Loading(false));
        BrowserControlFailure::new(code, message)
    }

    fn handle_vertical_scrollbar_input(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &FrameSlot,
        input: &BrowserInput,
    ) -> bool {
        match input {
            BrowserInput::MousePress {
                x,
                y,
                button: BrowserButton::Left,
                ..
            } => {
                self.vertical_scrollbar_drag = None;
                // Avoid a synchronous layout roundtrip for ordinary page
                // clicks. Native and host-painted tracks are never wider than
                // this gate; the authoritative overlay metrics below make the
                // final hit decision and protect right-aligned page content.
                if *x < f64::from(self.viewport_w.saturating_sub(32)) {
                    return false;
                }
                self.schedule_scrollbar_layout_refresh(Duration::ZERO);
                let Some(layout) = self.scrollbar_layout.layout else {
                    return false;
                };
                match layout.vertical_press(*x, *y) {
                    Some(VerticalScrollbarPress::Drag(drag)) => {
                        self.vertical_scrollbar_drag = Some(drag);
                    }
                    Some(VerticalScrollbarPress::Track(target)) => {
                        self.scroll_page_to(link, event_tx, frame_slot, target);
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
                self.scroll_page_to(link, event_tx, frame_slot, target);
                true
            }
            BrowserInput::MouseRelease {
                y,
                button: BrowserButton::Left,
                ..
            } if self.vertical_scrollbar_drag.is_some() => {
                let drag = self.vertical_scrollbar_drag.take();
                let target = drag.map(|drag| drag.target_scroll_y(*y)).unwrap_or_default();
                self.scroll_page_to(link, event_tx, frame_slot, target);
                true
            }
            _ => false,
        }
    }

    fn scroll_page_to(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &FrameSlot,
        target: f64,
    ) {
        self.request_runtime_for_sessions(link);
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
                *layout = layout.with_scroll_y(target);
                if frame_slot.publish_page_scroll_state(*layout) {
                    event_tx.wake_ui();
                }
            }
            self.schedule_scrollbar_layout_refresh(SCROLLBAR_LAYOUT_RETRY_DELAY);
        }
    }

    pub(super) fn viewport_override_params(width: u32, height: u32) -> serde_json::Value {
        serde_json::json!({
            "width": width,
            "height": height,
            "deviceScaleFactor": 0,
            "mobile": false,
        })
    }

    pub(super) fn invalidate_scrollbar_layout(&mut self, event_tx: &BrowserEventSender) {
        self.vertical_scrollbar_drag = None;
        self.scrollbar_layout.layout = None;
        // A detached page session is not guaranteed to answer its outstanding
        // request. CDP ids are connection-global, so a late reply cannot be
        // mistaken for the replacement request after this slot is cleared.
        self.scrollbar_layout.request_id = None;
        self.scrollbar_layout.refresh_at = Some(Instant::now());
        Self::clear_published_scrollbar(&self.config.frame_slot, event_tx);
        self.dismiss_native_select(event_tx);
    }

    fn clear_published_scrollbar(frame_slot: &FrameSlot, event_tx: &BrowserEventSender) {
        if frame_slot.clear_page_scroll_state() {
            event_tx.wake_ui();
        }
    }

    fn abandon_sampled_scrollbar(&mut self, frame_slot: &FrameSlot, event_tx: &BrowserEventSender) {
        self.vertical_scrollbar_drag = None;
        self.scrollbar_layout.layout = None;
        Self::clear_published_scrollbar(frame_slot, event_tx);
        self.schedule_scrollbar_layout_refresh(SCROLLBAR_LAYOUT_RETRY_DELAY);
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
        frame_slot: &FrameSlot,
        event_tx: &BrowserEventSender,
    ) -> bool {
        if self.scrollbar_layout.request_id != Some(id) {
            return false;
        }
        self.scrollbar_layout.request_id = None;
        if rejected {
            self.abandon_sampled_scrollbar(frame_slot, event_tx);
            return true;
        }
        let Some(layout) = result.and_then(|metrics| {
            PageScrollState::from_chromium_layout_metrics(metrics, self.viewport_w, self.viewport_h)
        }) else {
            self.abandon_sampled_scrollbar(frame_slot, event_tx);
            return true;
        };
        self.scrollbar_layout.layout = Some(layout);
        if frame_slot.publish_page_scroll_state(layout) {
            event_tx.wake_ui();
        }
        true
    }

    pub(super) fn note_screencast_scroll_offset(
        &mut self,
        scroll_y: Option<f64>,
        frame_slot: &FrameSlot,
        event_tx: &BrowserEventSender,
    ) {
        if let Some(scroll_y) = scroll_y.filter(|value| value.is_finite() && *value >= 0.0)
            && let Some(layout) = self.scrollbar_layout.layout.as_mut()
        {
            *layout = layout.with_scroll_y(scroll_y);
            if frame_slot.publish_page_scroll_state(*layout) {
                event_tx.wake_ui();
            }
        }
        // Screencast frames arrive when the page paints, including DOM-driven
        // height changes that leave scrollY unchanged. Refresh layout metrics
        // on that cadence so the overlay and hit box follow content size.
        self.schedule_scrollbar_layout_refresh(SCROLLBAR_LAYOUT_RETRY_DELAY);
    }

    fn set_viewport(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        width: u32,
        height: u32,
    ) {
        if !self.viewport_policy.follow_host([width, height]) || !self.queue_viewport(width, height) {
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
            &Self::viewport_override_params(width, height),
        ) {
            Ok(_) => {
                self.commit_viewport(width, height, event_tx);
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

    pub(super) fn commit_viewport(&mut self, width: u32, height: u32, event_tx: &BrowserEventSender) {
        self.viewport_w = width;
        self.viewport_h = height;
        self.pending_viewport = None;
        self.viewport_retry_at = None;
        self.invalidate_scrollbar_layout(event_tx);
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
    ) -> Result<(), BrowserControlFailure> {
        self.supersede_pending_navigation(Instant::now());
        let Some(session) = self.session_id.clone() else {
            return Err(self.page_command_failure(
                event_tx,
                "history traversal",
                "browser_unavailable",
                "the Chromium page session is not attached".to_string(),
            ));
        };
        let history = match self.call_and_ack(
            link,
            event_tx,
            frame_slot,
            "Page.getNavigationHistory",
            &serde_json::json!({}),
            Some(session.as_str()),
        ) {
            Ok(history) => history,
            Err(error) => {
                return Err(self.page_command_failure(
                    event_tx,
                    "history traversal",
                    "protocol_error",
                    error.to_string(),
                ));
            }
        };
        let Some(current) = history.get("currentIndex").and_then(serde_json::Value::as_i64) else {
            return Err(self.page_command_failure(
                event_tx,
                "history traversal",
                "invalid_result",
                "CDP returned no current history index".to_string(),
            ));
        };
        let Some(entries) = history.get("entries").and_then(serde_json::Value::as_array) else {
            return Err(self.page_command_failure(
                event_tx,
                "history traversal",
                "invalid_result",
                "CDP returned no history entries".to_string(),
            ));
        };
        let Some(last) = i64::try_from(entries.len()).ok().and_then(|len| len.checked_sub(1)) else {
            return Ok(());
        };
        let target = (current + delta).clamp(0, last);
        if target == current {
            return Ok(());
        }
        let entry_index = usize::try_from(target).unwrap_or(0);
        // Chrome returns `id`; the request calls the same value `entryId`.
        let entry_value = &entries[entry_index];
        let Some(entry_id) = entry_value
            .get("entryId")
            .or_else(|| entry_value.get("id"))
            .and_then(serde_json::Value::as_i64)
        else {
            return Err(self.page_command_failure(
                event_tx,
                "history traversal",
                "invalid_result",
                "CDP returned a history entry without an identifier".to_string(),
            ));
        };
        // Marked before dispatch so events routed during the round trip can
        // already clear it (see the reload command).
        self.top_frame_navigating = true;
        match self.send_page_command(
            link,
            event_tx,
            frame_slot,
            "Page.navigateToHistoryEntry",
            &serde_json::json!({ "entryId": entry_id }),
        ) {
            Ok(_) => {
                self.pending_restart_at = Some(Instant::now());
                Ok(())
            }
            Err(error) => {
                self.top_frame_navigating = false;
                Err(self.page_command_failure(event_tx, "history traversal", "protocol_error", error.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    use crate::frames::FrameSlot;
    use crate::session::{BrowserEventSender, BrowserEventWake, CommittedUrl};
    use crate::{BrowserConfig, PageScrollState, session::BrowserSessionConfig};

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
                coordination: None,
                capture_directory: None,
                video: Arc::new(crate::VideoCaptureHandle::default()),
                remote: None,
            },
            "ws://127.0.0.1/devtools/browser/test",
            None,
            Arc::new(AtomicBool::new(false)),
        )
    }

    fn test_events() -> BrowserEventSender {
        let (tx, _rx) = mpsc::channel();
        BrowserEventSender {
            tx,
            wake: BrowserEventWake::default(),
            committed_url: CommittedUrl::default(),
        }
    }

    #[test]
    fn viewport_override_does_not_claim_the_panel_is_the_display() {
        let params = DriverState::viewport_override_params(1280, 800);

        assert_eq!(params["width"], 1280);
        assert_eq!(params["height"], 800);
        assert_eq!(params["deviceScaleFactor"], 0);
        assert_eq!(params["mobile"], serde_json::Value::Bool(false));
        assert!(params.get("screenWidth").is_none());
        assert!(params.get("screenHeight").is_none());
    }

    #[test]
    fn viewport_resize_becomes_authoritative_only_after_acknowledgement() {
        let mut state = driver_state();
        let events = test_events();

        assert!(state.queue_viewport(900, 600));
        assert_eq!((state.viewport_w, state.viewport_h), (1280, 800));
        assert_eq!(state.pending_viewport, Some((900, 600)));

        state.commit_viewport(900, 600, &events);

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

    fn seed_thumb_drag(state: &mut DriverState, layout: PageScrollState) {
        let Some(crate::page_scroll::VerticalScrollbarPress::Drag(drag)) = layout.vertical_press(1_155.0, 72.0) else {
            panic!("scrollable layout should start a thumb drag");
        };
        state.vertical_scrollbar_drag = Some(drag);
    }

    fn scrollable_layout() -> PageScrollState {
        PageScrollState {
            scroll_x: 0.0,
            scroll_y: 0.0,
            viewport_width: 1_164.0,
            viewport_height: 608.0,
            client_width: 1_149.0,
            client_height: 608.0,
            content_width: 1_149.0,
            content_height: 3_000.0,
        }
    }

    #[test]
    fn asynchronous_scrollbar_layout_response_populates_the_hit_test_cache() {
        let mut state = driver_state();
        let events = test_events();
        let frame_slot = Arc::clone(&state.config.frame_slot);
        state.scrollbar_layout.request_id = Some(41);
        state.scrollbar_layout.refresh_at = None;
        state.viewport_w = 1_164;
        state.viewport_h = 608;
        let metrics = serde_json::json!({
            "cssLayoutViewport": {
                "pageX": 0,
                "pageY": 120,
                "clientWidth": 1149,
                "clientHeight": 608
            },
            "cssContentSize": { "width": 1149, "height": 3000 }
        });

        assert!(state.handle_scrollbar_layout_response(41, Some(&metrics), false, &frame_slot, &events));
        let Some(layout) = state.scrollbar_layout.layout else {
            panic!("valid asynchronous metrics should populate the cache");
        };
        assert!((layout.scroll_y - 120.0).abs() < f32::EPSILON);
        assert!(layout.is_vertically_scrollable());
        assert_eq!(state.scrollbar_layout.request_id, None);
        assert_eq!(state.scrollbar_layout.refresh_at, None);
        assert_eq!(frame_slot.page_scroll_state(), Some(layout));
    }

    #[test]
    fn invalidation_clears_the_published_overlay_and_abandons_inflight_metrics() {
        let mut state = driver_state();
        let events = test_events();
        let frame_slot = Arc::clone(&state.config.frame_slot);
        let layout = scrollable_layout();
        state.scrollbar_layout.request_id = Some(41);
        state.scrollbar_layout.layout = Some(layout);
        seed_thumb_drag(&mut state, layout);
        assert!(frame_slot.publish_page_scroll_state(layout));

        state.invalidate_scrollbar_layout(&events);

        assert_eq!(state.scrollbar_layout.request_id, None);
        assert!(state.scrollbar_layout.layout.is_none());
        assert!(state.vertical_scrollbar_drag.is_none());
        assert!(state.scrollbar_layout.refresh_at.is_some());
        assert!(frame_slot.page_scroll_state().is_none());
        assert!(!state.handle_scrollbar_layout_response(41, None, false, &frame_slot, &events));
    }

    #[test]
    fn rejected_or_malformed_metrics_clear_the_published_overlay() {
        let mut state = driver_state();
        let events = test_events();
        let frame_slot = Arc::clone(&state.config.frame_slot);
        let layout = scrollable_layout();
        state.scrollbar_layout.layout = Some(layout);
        seed_thumb_drag(&mut state, layout);
        assert!(frame_slot.publish_page_scroll_state(layout));

        state.scrollbar_layout.request_id = Some(7);
        assert!(state.handle_scrollbar_layout_response(7, None, true, &frame_slot, &events));
        assert!(state.scrollbar_layout.layout.is_none());
        assert!(state.vertical_scrollbar_drag.is_none());
        assert!(frame_slot.page_scroll_state().is_none());

        assert!(frame_slot.publish_page_scroll_state(layout));
        state.scrollbar_layout.layout = Some(layout);
        seed_thumb_drag(&mut state, layout);
        state.scrollbar_layout.request_id = Some(8);
        assert!(state.handle_scrollbar_layout_response(8, Some(&serde_json::json!({})), false, &frame_slot, &events));
        assert!(state.scrollbar_layout.layout.is_none());
        assert!(state.vertical_scrollbar_drag.is_none());
        assert!(frame_slot.page_scroll_state().is_none());
    }

    #[test]
    fn screencast_activity_refreshes_cached_layout_metrics() {
        let mut state = driver_state();
        let events = test_events();
        let frame_slot = Arc::clone(&state.config.frame_slot);
        state.scrollbar_layout.layout = Some(scrollable_layout());
        state.scrollbar_layout.refresh_at = None;

        state.note_screencast_scroll_offset(Some(80.0), &frame_slot, &events);

        assert!(state.scrollbar_layout.refresh_at.is_some());
        let layout = state.scrollbar_layout.layout.expect("cached layout");
        assert!((layout.scroll_y - 80.0).abs() < f32::EPSILON);
    }
}
