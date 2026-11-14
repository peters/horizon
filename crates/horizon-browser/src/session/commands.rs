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
    BrowserCommand, BrowserEventSender, CommandReceiver, DriverState, SCROLLBAR_LAYOUT_REFRESH_INTERVAL,
    SCROLLBAR_LAYOUT_RETRY_DELAY, VIEWPORT_CAPTURE_DELAY, VIEWPORT_RETRY_DELAY,
};

/// The name of the wait of one command of the window, for the latency line of a
/// pass that blocked.
const fn command_step_name(command: &BrowserCommand) -> &'static str {
    match command {
        BrowserCommand::Input(_) => "drain:input",
        BrowserCommand::Navigate(_) | BrowserCommand::Back | BrowserCommand::Forward => "drain:navigate",
        BrowserCommand::Reload => "drain:reload",
        BrowserCommand::SetViewport { .. } => "drain:viewport",
        BrowserCommand::DropFiles { .. } => "drain:drop-files",
        BrowserCommand::NativeSelectChoose { .. } | BrowserCommand::NativeSelectDismiss => "drain:native-select",
        BrowserCommand::HandoffDone => "drain:handoff",
        BrowserCommand::Video { .. } => "drain:video",
        BrowserCommand::Orientation { .. } | BrowserCommand::Stop => "drain:control",
    }
}

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
            // Each command names its own wait, so a line of `latency stall` says
            // which command of the window held the pass and not only that the
            // drain took its time.
            let step = crate::latency::step(command_step_name(&command));
            let dispatched = self.dispatch_command(link, event_tx, frame_slot, command, true);
            drop(step);
            if dispatched.is_ok_and(|stop| stop) {
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
            if matches!(
                request.action,
                crate::BrowserControlAction::Resize { .. } | crate::BrowserControlAction::Zoom { .. }
            ) {
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
            let result = result.map_err(|failure| Self::failure_when_link_lost(link, failure));
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
            BrowserCommand::Orientation { .. } => {
                let _ = event_tx.send(super::BrowserEvent::OrientationChanged(
                    crate::remote::RemoteOrientationView {
                        state: crate::remote::RemoteOrientationState {
                            support: crate::remote::OrientationSupport::Unsupported,
                            applied: None,
                        },
                        error: Some("orientation_unsupported: local browsers use viewport resize".into()),
                        ..Default::default()
                    },
                ));
                Ok(false)
            }
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
                // The reply arrives in a later pass, so a person's key or click
                // that follows this reload is written to the socket at once
                // instead of after a far browser answered it.
                match self.reload_page(link) {
                    Ok(request_id) => {
                        self.reload_request_id = Some(request_id);
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
            BrowserCommand::DropFiles { x, y, paths } => {
                self.dispatch_file_drop(link, event_tx, frame_slot, x, y, &paths)
            }
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
        let traced = frame_slot.take_action();
        let session = self.session_id.clone().ok_or_else(|| {
            BrowserControlFailure::new("browser_unavailable", "the Chromium page session is not attached")
        })?;
        if let Err(error) = link.send_request(method, &params, Some(session.as_str())) {
            if teach_capture.is_some() {
                frame_slot.store_teach_failure("input_failed", &error.to_string(), frame_slot.teach_generation());
            }
            return Err(BrowserControlFailure::new("input_failed", error.to_string()));
        }
        if let Some(action) = traced {
            frame_slot.note_action_written(action, method);
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

    /// A call that failed because the link to the browser went away says so, so
    /// an agent reads a lost link and not a socket error, and never reads a
    /// lost link as a completed action.
    fn failure_when_link_lost(link: &CdpLink, failure: BrowserControlFailure) -> BrowserControlFailure {
        if !link.link_lost() {
            return failure;
        }
        BrowserControlFailure::new(
            "link_lost",
            format!(
                "the link to the browser was lost during this action, so it may or may not have happened; the panel tries again \
                 by itself and a later call works: {}",
                failure.message
            ),
        )
    }

    pub(super) fn page_command_failure(
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

    /// The answer arrived: a page that painted since the last read is still
    /// moving, so the next slow read is due; a still page costs nothing.
    pub(super) fn note_scrollbar_layout_answer(&mut self) {
        if !self.scrollbar_layout.painted_since_refresh {
            return;
        }
        self.scrollbar_layout.painted_since_refresh = false;
        self.schedule_scrollbar_layout_refresh(SCROLLBAR_LAYOUT_REFRESH_INTERVAL);
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
        self.note_scrollbar_layout_answer();
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
        // A picture is not a reason to ask the browser for its geometry: on a
        // far link that request shares the socket with the pictures and its
        // answer wakes the UI again. The slow cadence below keeps a painting
        // page current, and a size change or a scroll command asks at once.
        self.scrollbar_layout.painted_since_refresh = true;
    }

    /// One decoded picture of `size`: a new size means the page reflowed, so
    /// its scroll geometry must be read again.
    pub(super) fn note_frame_size_for_layout(&mut self, size: (u32, u32)) {
        if self.scrollbar_layout.frame_size == Some(size) {
            return;
        }
        self.scrollbar_layout.frame_size = Some(size);
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
        if !self.viewport_policy.follow_host([width, height]) {
            return;
        }
        // Under a zoom the page follows the panel divided by the zoom.
        let [width, height] = self.viewport_policy.applied();
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
        let bound_session = self.session_id.clone();
        match self.send_page_command(
            link,
            event_tx,
            frame_slot,
            "Emulation.setDeviceMetricsOverride",
            &Self::viewport_override_params(width, height),
        ) {
            // A reattachment drained by the command gave the new session the
            // old size; keep the update pending so the next tick applies it there.
            Ok(_) if self.session_id != bound_session => self.viewport_retry_at = None,
            Ok(_) => {
                self.commit_viewport(width, height, event_tx);
                frame_slot.set_viewport_override(self.viewport_policy.frame_override());
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
    use std::time::Duration;

    use crate::frames::FrameSlot;
    use crate::session::{BrowserEventSender, BrowserEventWake, CommittedUrl};
    use crate::{BrowserConfig, BrowserControlFailure, PageScrollState, session::BrowserSessionConfig};

    use super::DriverState;

    fn driver_state() -> DriverState {
        DriverState::new(
            &BrowserSessionConfig {
                attach: None,
                resume_target: None,
                resume_context: None,
                page_isolation: Default::default(),
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

    /// A CDP endpoint that holds the answer of `Page.reload` for a far link's
    /// round trip, and answers every other command at once. It reports the time
    /// between the reload and the next command it reads, so a test can tell
    /// whether that command was written while the reload's answer was owed.
    fn reload_holding_server() -> (String, std::thread::JoinHandle<std::time::Duration>) {
        use std::time::{Duration, Instant};

        const FAR_LINK: Duration = Duration::from_millis(200);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_millis(5))).unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            let mut held: Option<(serde_json::Value, Instant)> = None;
            let mut gap = Duration::ZERO;
            loop {
                match socket.read() {
                    Ok(tungstenite::Message::Text(text)) => {
                        let command: serde_json::Value = serde_json::from_str(&text).unwrap();
                        let method = command["method"].as_str().unwrap_or_default().to_string();
                        if method != "Page.reload"
                            && let Some((reload, reload_at)) = held.take()
                        {
                            // The page answers the reload now, and the command
                            // that arrived while its answer was owed leaves with
                            // the same write.
                            gap = reload_at.elapsed();
                            for answer in [reload, command] {
                                let response = serde_json::json!({"id": answer["id"], "result": {}});
                                socket
                                    .send(tungstenite::Message::Text(response.to_string().into()))
                                    .unwrap();
                            }
                            break;
                        }
                        if held.is_none() {
                            held = Some((command, Instant::now()));
                        }
                    }
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(error))
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(_) => break,
                }
                if let Some((reload, reload_at)) = held.as_ref()
                    && reload_at.elapsed() >= FAR_LINK
                {
                    let (reload, reload_at) = (reload.clone(), *reload_at);
                    // The far link answered the reload before any command of the
                    // window reached the page: the page was held for a whole
                    // round trip.
                    gap = reload_at.elapsed();
                    let response = serde_json::json!({"id": reload["id"], "result": {}});
                    socket
                        .send(tungstenite::Message::Text(response.to_string().into()))
                        .unwrap();
                    break;
                }
            }
            gap
        });
        (url, server)
    }

    /// F1: a key that follows a reload is written to the socket at once. The
    /// reload's answer belongs to a far browser and arrives a round trip later,
    /// and waiting for it in the drain would hold every input behind it.
    #[test]
    fn a_key_that_follows_a_reload_leaves_before_the_reload_is_answered() {
        use std::time::Duration;

        let (url, server) = reload_holding_server();
        let mut state = driver_state();
        state.session_id = Some("page".into());
        let slot = Arc::clone(&state.config.frame_slot);
        let events = test_events();
        let (sender, receiver) = super::super::command_queue::channel(Arc::clone(&slot));
        assert!(sender.send(crate::BrowserCommand::Reload));
        assert!(sender.send(crate::BrowserCommand::Input(crate::BrowserInput::KeyDown {
            physical_key: None,
            key: crate::BrowserKey::Char('a'),
            text: Some("a".into()),
            modifiers: crate::BrowserModifiers::default(),
            repeat: false,
            edit_command: None,
        })));
        let mut link = crate::cdp::CdpLink::connect(&url).unwrap();

        let stop = state.drain_commands(&mut link, &receiver, &events, &slot);

        assert!(!stop, "the drain of the test keeps the session");
        let gap = server.join().unwrap();
        assert!(
            gap < Duration::from_millis(100),
            "the key left {gap:?} after the reload, before its answer at 200 ms"
        );
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

    /// A CDP endpoint that acknowledges every command. With `detach_page`,
    /// the bound page session detaches before the first metrics override is
    /// acknowledged, as a reattachment drained by that command does.
    fn metrics_server(detach_page: bool) -> (String, std::thread::JoinHandle<Vec<serde_json::Value>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            let mut commands = Vec::new();
            let mut detach = detach_page;
            while let Ok(tungstenite::Message::Text(text)) = socket.read() {
                let command: serde_json::Value = serde_json::from_str(&text).unwrap();
                if detach && command["method"] == "Emulation.setDeviceMetricsOverride" {
                    detach = false;
                    let event = serde_json::json!({
                        "method": "Target.detachedFromTarget",
                        "params": {"sessionId": "page"},
                    });
                    socket
                        .send(tungstenite::Message::Text(event.to_string().into()))
                        .unwrap();
                }
                let response = serde_json::json!({"id": command["id"], "result": {}});
                commands.push(command);
                socket
                    .send(tungstenite::Message::Text(response.to_string().into()))
                    .unwrap();
            }
            commands
        });
        (url, server)
    }

    #[test]
    fn a_zoomed_page_keeps_its_update_pending_when_the_session_reattaches_during_it() {
        let (url, server) = metrics_server(true);
        let mut state = driver_state();
        state.session_id = Some("page".into());
        let slot = Arc::clone(&state.config.frame_slot);
        let events = test_events();
        let mut link = crate::cdp::CdpLink::connect(&url).unwrap();
        state
            .viewport_policy
            .commit(super::super::viewport::ViewportChange::Zoom(Some(200)));

        state.set_viewport(&mut link, &events, &slot, 1000, 700);

        assert_eq!(state.session_id, None);
        assert_eq!((state.viewport_w, state.viewport_h), (1280, 800));
        assert_eq!(state.pending_viewport, Some((500, 350)), "the next tick applies it");
        assert_eq!(slot.viewport_override(), None, "no override for a size the page lacks");
        drop(link);
        server.join().unwrap();
    }

    #[test]
    fn a_zoomed_page_follows_the_panel_divided_by_the_zoom() {
        let (url, server) = metrics_server(false);
        let mut state = driver_state();
        state.session_id = Some("page".into());
        let slot = Arc::clone(&state.config.frame_slot);
        let events = test_events();
        let mut link = crate::cdp::CdpLink::connect(&url).unwrap();
        state
            .viewport_policy
            .commit(super::super::viewport::ViewportChange::Zoom(Some(200)));

        state.set_viewport(&mut link, &events, &slot, 1000, 700);

        assert_eq!((state.viewport_w, state.viewport_h), (500, 350));
        assert_eq!(
            slot.viewport_override(),
            Some([500, 350]),
            "the host letterboxes the zoomed page"
        );
        drop(link);
        let applied = server
            .join()
            .unwrap()
            .into_iter()
            .find(|command| command["method"] == "Emulation.setDeviceMetricsOverride")
            .expect("metrics override");
        assert_eq!(
            (
                applied["params"]["width"].as_u64(),
                applied["params"]["height"].as_u64()
            ),
            (Some(500), Some(350))
        );
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
    fn a_call_that_failed_because_the_link_went_away_says_so() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let address = listener.local_addr().expect("mock address");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept mock");
            let socket = tungstenite::accept(stream).expect("mock handshake");
            // The link goes away while the action waits for an answer.
            drop(socket);
        });
        let mut link = crate::cdp::CdpLink::connect(&format!("ws://{address}/")).expect("connect mock");
        let _state = driver_state();

        let refused = DriverState::failure_when_link_lost(
            &link,
            BrowserControlFailure::new("protocol_error", "cdp error -32000: refused"),
        );
        assert_eq!(
            refused.code, "protocol_error",
            "a link that still answers keeps the words of the browser"
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !link.link_lost() && std::time::Instant::now() < deadline {
            let _ = link.read_one();
        }
        assert!(link.link_lost(), "the closed link reports its loss");
        let lost = DriverState::failure_when_link_lost(
            &link,
            BrowserControlFailure::new(
                "protocol_error",
                "websocket: Connection reset without closing handshake",
            ),
        );
        assert_eq!(lost.code, "link_lost");
        assert!(
            lost.message.contains("may or may not have happened"),
            "{}",
            lost.message
        );
        assert!(
            lost.message.contains("a later call works"),
            "the agent reads that the panel recovers: {}",
            lost.message
        );
        drop(link);
        let _ = server.join();
    }

    #[test]
    fn screencast_activity_updates_the_scroll_offset_without_a_request() {
        let mut state = driver_state();
        let events = test_events();
        let frame_slot = Arc::clone(&state.config.frame_slot);
        state.scrollbar_layout.layout = Some(scrollable_layout());
        state.scrollbar_layout.refresh_at = None;

        state.note_screencast_scroll_offset(Some(80.0), &frame_slot, &events);

        assert!(
            state.scrollbar_layout.refresh_at.is_none(),
            "a picture is not a reason to ask for the geometry again"
        );
        assert!(state.scrollbar_layout.painted_since_refresh);
        let layout = state.scrollbar_layout.layout.expect("cached layout");
        assert!((layout.scroll_y - 80.0).abs() < f32::EPSILON);
    }
}
