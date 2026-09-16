//! Firefox CSS viewport application, acknowledgement, and binding recovery.

use std::time::{Duration, Instant};

use serde_json::json;

use crate::session::BrowserEventSender;
use crate::{AgentAction, BrowserControlFailure, BrowserControlValue};

use super::Driver;

const INITIAL_RETRY: Duration = Duration::from_millis(100);
const MAX_RETRY: Duration = Duration::from_secs(2);

pub(super) struct RestorationRetry {
    context: Option<String>,
    next_attempt: Instant,
    delay: Duration,
}

impl Default for RestorationRetry {
    fn default() -> Self {
        Self {
            context: None,
            next_attempt: Instant::now(),
            delay: INITIAL_RETRY,
        }
    }
}

impl RestorationRetry {
    fn due(&mut self, context: Option<&str>) -> bool {
        if self.context.as_deref() != context {
            self.context = context.map(str::to_owned);
            self.succeeded();
        }
        Instant::now() >= self.next_attempt
    }

    fn succeeded(&mut self) {
        self.delay = INITIAL_RETRY;
        self.next_attempt = Instant::now();
    }

    fn failed(&mut self) {
        // Backoff starts after the bounded protocol call completes.
        self.next_attempt = Instant::now() + self.delay;
        self.delay = (self.delay * 2).min(MAX_RETRY);
    }
}

impl Driver {
    pub(super) fn begin_resize(&mut self, request: &AgentAction) {
        let result = self.prepare_resize(request);
        match result {
            Ok(pending) => {
                self.audit_agent_action(request, crate::BrowserAuditStatus::Dispatched);
                self.finish_pending_resize("viewport_superseded", "a newer resize replaced this request");
                self.pending_resize = Some(pending);
            }
            Err(error) => {
                self.audit_agent_action(request, crate::BrowserAuditStatus::Rejected);
                self.complete_agent_action(request, Err(error));
            }
        }
    }

    fn prepare_resize(
        &self,
        request: &AgentAction,
    ) -> Result<crate::session::viewport::PendingResize, BrowserControlFailure> {
        if self.host.is_remote() {
            return Err(BrowserControlFailure::new(
                "remote_viewport_fixed",
                "remote device viewports cannot be resized",
            ));
        }
        if !self.firefox_bidi() {
            return Err(BrowserControlFailure::new(
                "viewport_unsupported",
                "exact viewport resizing requires Chromium or local Firefox BiDi",
            ));
        }
        if self.panel_slot.teach_recording() {
            return Err(BrowserControlFailure::new(
                "teach_recording",
                "Teach mode has exclusive ownership of this panel",
            ));
        }
        crate::session::viewport::PendingResize::new(request, self.viewport_policy)
    }

    pub(super) fn finish_pending_resize(&mut self, code: &str, message: &str) {
        if let Some(pending) = self.pending_resize.take() {
            if let Some(coordination) = &self.config.coordination {
                coordination.retain_action_result_on_remove(&self.config.panel_local_id, &pending.request.action_id);
            }
            self.complete_agent_action(&pending.request, Err(BrowserControlFailure::new(code, message)));
        }
    }

    pub(super) fn tick_pending_resize(&mut self, events: &BrowserEventSender, stop: &std::sync::atomic::AtomicBool) {
        let Some(mut pending) = self.pending_resize.take() else {
            if !stop.load(std::sync::atomic::Ordering::Acquire) {
                self.restore_viewport_binding(events);
            }
            return;
        };
        match self.advance_resize(&mut pending, events, stop) {
            Ok(None) => self.pending_resize = Some(pending),
            Ok(Some(value)) => self.complete_agent_action(&pending.request, Ok(value)),
            Err(error) => {
                if let Some(coordination) = &self.config.coordination {
                    coordination
                        .retain_action_result_on_remove(&self.config.panel_local_id, &pending.request.action_id);
                }
                self.complete_agent_action(&pending.request, Err(error));
            }
        }
    }

    fn restore_viewport_binding(&mut self, events: &BrowserEventSender) {
        let Some(target) = self
            .viewport_policy
            .rebind_target(self.viewport_context.as_deref(), self.context_id.as_deref())
        else {
            return;
        };
        if !self.viewport_retry.due(self.context_id.as_deref()) {
            return;
        }
        if let Err(error) = self.apply_explicit_viewport(target, Duration::from_millis(100), events) {
            self.viewport_retry.failed();
            tracing::debug!("viewport restoration pending: {}", error.message);
        }
    }

    fn apply_explicit_viewport(
        &mut self,
        [width, height]: [u32; 2],
        timeout: std::time::Duration,
        events: &BrowserEventSender,
    ) -> Result<(), BrowserControlFailure> {
        let context = self.context_id.clone();
        let link = self
            .bidi
            .as_mut()
            .ok_or_else(|| BrowserControlFailure::new("viewport_unsupported", "Firefox BiDi is unavailable"))?;
        let outcome = link.call(
            timeout,
            "browsingContext.setViewport",
            &json!({
                "context": context, "viewport": { "width": width, "height": height },
            }),
        );
        for event in outcome.events {
            self.handle_bidi_event(&event, events);
        }
        outcome
            .result
            .map_err(|error| BrowserControlFailure::new("viewport_failed", error.to_string()))?;
        // Events drained during the command may bind a different context.
        self.viewport_context = context;
        self.viewport_retry.succeeded();
        self.advance_generation();
        self.frames.demand();
        Ok(())
    }

    fn advance_resize(
        &mut self,
        pending: &mut crate::session::viewport::PendingResize,
        events: &BrowserEventSender,
        stop: &std::sync::atomic::AtomicBool,
    ) -> Result<Option<BrowserControlValue>, BrowserControlFailure> {
        pending.guard(
            self.owner_seen.as_deref(),
            self.handoff_seen.is_some(),
            self.panel_slot.teach_recording()
                || self
                    .last_user_active_stamp
                    .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(5)),
            stop.load(std::sync::atomic::Ordering::Acquire) || self.host.has_exited(),
        )?;
        if !pending.poll_due() || self.context_id.is_none() {
            return Ok(None);
        }
        let target = self.viewport_policy.target(pending.requested);
        if pending.target != target {
            pending.bound_session = None;
            pending.target = target;
        }
        if pending.bound_session.is_none() || pending.bound_session != self.context_id {
            let context = self.context_id.clone();
            let result = self.apply_explicit_viewport(target, pending.budget()?, events);
            if result.is_err() && context != self.context_id {
                return Ok(None);
            }
            result?;
            self.viewport_policy.commit(pending.requested);
            self.panel_slot.set_viewport_override(pending.requested);
            pending.bound_session = context;
            return Ok(None);
        }
        let Ok(value) = self.evaluate_json_within(crate::session::viewport::MEASURE_VIEWPORT, Some(pending.budget()?))
        else {
            pending.budget()?;
            return Ok(None);
        };
        let result = pending.observe(value, self.signal_epoch)?;
        self.request_signal_refresh();
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::net::TcpListener;
    use std::sync::{Arc, atomic::AtomicBool, mpsc};

    use serde_json::Value;
    use tungstenite::Message;

    use super::super::{frames::AdaptiveFrames, scrollbar};
    use super::*;
    use crate::frames::FrameSlot;
    use crate::semantic::SemanticState;
    use crate::session::{BrowserEventWake, BrowserSessionConfig, CommittedUrl, viewport::PendingResize};
    use crate::webdriver::actions::ActionState;
    use crate::webdriver::host::DriverHost;
    use crate::webdriver::remote::RemoteHost;
    use crate::webdriver::test_server::{Reply, Server};
    use crate::websocket::JsonWsLink;
    use crate::{BackendKind, BrowserConfig, BrowserControlAction};

    #[test]
    fn restoration_backoff_starts_after_completion_and_resets_for_new_contexts() {
        let mut retry = RestorationRetry::default();
        assert!(retry.due(Some("first")));
        for _ in 0..10 {
            let completed = Instant::now();
            retry.failed();
            assert!(retry.next_attempt >= completed + INITIAL_RETRY);
            assert!(retry.delay <= MAX_RETRY);
            assert!(!retry.due(Some("first")));
        }
        assert_eq!(retry.delay, MAX_RETRY);
        assert!(retry.due(Some("replacement")));
        assert_eq!(retry.delay, INITIAL_RETRY);
        retry.failed();
        retry.succeeded();
        assert!(retry.due(Some("replacement")));
        assert_eq!(retry.delay, INITIAL_RETRY);
    }

    fn event(method: &str, context: &str, parent: &Value) -> Value {
        json!({"method":format!("browsingContext.{method}"),"params":{"context":context,"parent":parent}})
    }

    fn bidi_fixture(refuse: bool) -> (JsonWsLink, std::thread::JoinHandle<Vec<Value>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            let mut commands = Vec::new();
            while let Ok(Message::Text(text)) = socket.read() {
                let command: Value = serde_json::from_str(&text).unwrap();
                commands.push(command.clone());
                if commands.len() == 2 {
                    for notification in [
                        event("contextDestroyed", "second", &Value::Null),
                        event("contextCreated", "iframe", &json!("second")),
                        event("contextCreated", "third", &Value::Null),
                    ] {
                        socket.send(Message::Text(notification.to_string().into())).unwrap();
                    }
                }
                let response = if refuse {
                    json!({"id":command["id"],"type":"error","error":"unsupported operation","message":"refused"})
                } else {
                    json!({"id":command["id"],"type":"success","result":{}})
                };
                socket.send(Message::Text(response.to_string().into())).unwrap();
            }
            commands
        });
        (JsonWsLink::connect(&url).unwrap(), worker)
    }

    fn fixture_driver(classic: &Server, link: JsonWsLink) -> Driver {
        // Use the existing loopback classic transport adapter without launching
        // a process. The real Firefox resize and event paths run below; remote
        // capability admission is intentionally not part of this fixture.
        let request = crate::RemoteSessionRequest {
            endpoint: classic.endpoint(""),
            authorization: None,
            capabilities: json!({}),
            allocation_timeout: Duration::from_secs(1),
            max_session: Duration::from_secs(60),
            idle_release: Duration::from_secs(60),
            label: "test".into(),
            provider: "test".into(),
            quota_key: "test".into(),
            browser: BackendKind::FirefoxBidi,
            device: horizon_browser_protocol::remote::DeviceRequirement {
                kind: horizon_browser_protocol::remote::DeviceKind::Any,
                model: None,
                os_version: None,
            },
            evidence: crate::DeviceEvidenceSource::Capabilities,
        };
        let config = BrowserSessionConfig {
            browser: BrowserConfig {
                backend: BackendKind::FirefoxBidi,
                ..BrowserConfig::default()
            },
            panel_local_id: "test".into(),
            initial_url: None,
            width: 900,
            height: 600,
            frame_slot: Arc::new(FrameSlot::new()),
            coordination: None,
            capture_directory: None,
            video: Arc::default(),
            remote: None,
        };
        let frame_slot = &config.frame_slot;
        let host = DriverHost::Remote(RemoteHost::connect(&request).unwrap());
        let remote_release = Arc::default();
        let remote_device = None;
        let session_id = "test".into();
        let bidi = Some(link);
        let automation_ws = String::new();
        let context_id = Some("first".into());
        let safari = None;
        Driver {
            config: config.clone(),
            host,
            remote_release,
            remote_device,
            session_id,
            bidi,
            automation_ws,
            context_id,
            safari,
            actions: ActionState::default(),
            frames: AdaptiveFrames::new(),
            pending_resize: None,
            viewport_policy: crate::session::viewport::ViewportPolicy::new([config.width, config.height]),
            viewport_context: None,
            viewport_retry: RestorationRetry::default(),
            scrollbar: scrollbar::State::new(),
            url: String::new(),
            title: String::new(),
            generation: 0,
            retain_frame_during_navigation: false,
            navigation_failed: false,
            pending_navigation: None,
            pending_wait: None,
            navigate_request_id: None,
            pending_classic_history_start: None,
            refresh_pending_at: None,
            classic_timeout_to_restore: None,
            classic_document_identity: None,
            classic_refresh: None,
            coordination_dirty: true,
            last_coordination_write: Instant::now(),
            last_signal_check: Instant::now(),
            last_user_active_stamp: None,
            owner_seen: Some("agent".into()),
            signal_epoch: 0,
            handoff_seen: None,
            audit_sampler: crate::audit::BrowserAuditSampler::default(),
            semantic: SemanticState::default(),
            challenge_loop: crate::challenge::ChallengeLoopDetector::default(),
            http_auth: crate::http_auth::HttpAuthState::default(),
            network: crate::network::NetworkCaptureState::default(),
            video: crate::video::VideoCaptureState::new(Arc::clone(&config.video)),
            firefox_network: None,
            pending_http_bodies: VecDeque::new(),
            panel_slot: Arc::clone(frame_slot),
            native_select: super::super::native_select::NativeSelectState::default(),
        }
    }

    fn events() -> BrowserEventSender {
        let (tx, _) = mpsc::channel();
        BrowserEventSender {
            tx,
            wake: BrowserEventWake::default(),
            committed_url: CommittedUrl::default(),
        }
    }

    fn pending(state: &Driver, viewport: Option<[u32; 2]>) -> PendingResize {
        PendingResize::new(
            &AgentAction {
                action_id: "test".into(),
                actor: "agent".into(),
                requested_at_millis: crate::navigation::now_millis(),
                action: BrowserControlAction::Resize {
                    viewport,
                    timeout_millis: 5000,
                },
            },
            state.viewport_policy,
        )
        .unwrap()
    }

    fn acknowledge(state: &mut Driver, viewport: Option<[u32; 2]>, events: &BrowserEventSender) {
        let stop = AtomicBool::new(false);
        let mut pending = pending(state, viewport);
        assert_eq!(state.advance_resize(&mut pending, events, &stop).unwrap(), None);
        std::thread::sleep(INITIAL_RETRY + Duration::from_millis(10));
        state.signal_epoch += 1;
        assert_eq!(state.advance_resize(&mut pending, events, &stop).unwrap(), None);
        std::thread::sleep(INITIAL_RETRY + Duration::from_millis(10));
        state.signal_epoch += 1;
        assert_eq!(
            state.advance_resize(&mut pending, events, &stop).unwrap(),
            Some(BrowserControlValue::Viewport {
                requested: viewport,
                applied: viewport.unwrap_or([900, 600]),
            })
        );
    }

    #[test]
    fn firefox_restores_committed_pin_on_rebind_drains_events_and_reset_stops_restoration() {
        let classic = Server::start(
            [[390, 844], [390, 844], [900, 600], [900, 600]]
                .into_iter()
                .map(|size| Reply::json(200, &json!({"value":size})))
                .collect(),
        );
        let (link, worker) = bidi_fixture(false);
        let mut state = fixture_driver(&classic, link);
        let events = events();
        acknowledge(&mut state, Some([390, 844]), &events);
        assert_eq!(state.panel_slot.viewport_override(), Some([390, 844]));
        state.handle_bidi_event(&event("contextDestroyed", "first", &Value::Null), &events);
        state.handle_bidi_event(&event("contextCreated", "iframe", &json!("second")), &events);
        assert!(state.context_id.is_none());
        state.restore_viewport_binding(&events);
        state.handle_bidi_event(&event("contextCreated", "second", &Value::Null), &events);
        state.restore_viewport_binding(&events);
        assert_eq!(state.context_id.as_deref(), Some("third"));
        assert_eq!(state.viewport_context.as_deref(), Some("second"));
        state.restore_viewport_binding(&events);
        assert_eq!(state.viewport_context.as_deref(), Some("third"));
        acknowledge(&mut state, None, &events);
        assert_eq!(state.panel_slot.viewport_override(), None);
        state.handle_bidi_event(&event("contextDestroyed", "third", &Value::Null), &events);
        state.handle_bidi_event(&event("contextCreated", "fourth", &Value::Null), &events);
        state.restore_viewport_binding(&events);
        drop(state);
        let commands = worker.join().unwrap();
        assert_eq!(commands.len(), 4, "reset must stop restoration");
        for (command, context) in commands.iter().zip(["first", "second", "third", "third"]) {
            assert_eq!(command["method"], "browsingContext.setViewport");
            assert_eq!(command["params"]["context"], context);
        }
        assert_eq!(commands[2]["params"]["viewport"], json!({"width":390,"height":844}));
        assert_eq!(commands[3]["params"]["viewport"], json!({"width":900,"height":600}));
        assert_eq!(
            classic.recorded().len(),
            4,
            "acknowledgement measures classic WebDriver dimensions"
        );
    }

    #[test]
    fn firefox_protocol_refusal_does_not_commit_a_pin() {
        let classic = Server::start(vec![]);
        let (link, worker) = bidi_fixture(true);
        let mut state = fixture_driver(&classic, link);
        let mut pending = pending(&state, Some([390, 844]));
        let failure = state
            .advance_resize(&mut pending, &events(), &AtomicBool::new(false))
            .unwrap_err();
        assert_eq!(failure.code, "viewport_failed");
        assert_eq!(state.panel_slot.viewport_override(), None);
        assert!(state.viewport_policy.follow_host([1000, 700]));
        drop(state);
        assert_eq!(worker.join().unwrap().len(), 1);
    }
}
