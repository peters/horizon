//! Explicit viewport sizing and bounded browser measurement. Host layout is
//! remembered while pinned so reset resumes the latest panel dimensions.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::cdp::CdpLink;
use crate::frames::FrameSlot;
use crate::{AgentAction, BrowserControlAction, BrowserControlFailure, BrowserControlValue};

use super::{BrowserEventSender, DriverState, VIEWPORT_CAPTURE_DELAY};

pub(crate) const MEASURE_VIEWPORT: &str = "[window.innerWidth, window.innerHeight]";

#[derive(Clone, Copy, Debug)]
pub(crate) struct ViewportPolicy {
    host: [u32; 2],
    explicit: Option<[u32; 2]>,
}

impl ViewportPolicy {
    pub(crate) const fn new(host: [u32; 2]) -> Self {
        Self { host, explicit: None }
    }

    pub(crate) fn follow_host(&mut self, size: [u32; 2]) -> bool {
        if size.contains(&0) {
            return false;
        }
        self.host = size;
        self.explicit.is_none()
    }

    pub(crate) fn target(self, requested: Option<[u32; 2]>) -> [u32; 2] {
        requested.unwrap_or(self.host)
    }

    pub(crate) fn commit(&mut self, requested: Option<[u32; 2]>) {
        self.explicit = requested;
    }
}

#[derive(Debug)]
pub(crate) struct PendingResize {
    pub(crate) request: AgentAction,
    pub(crate) requested: Option<[u32; 2]>,
    pub(crate) target: [u32; 2],
    pub(crate) bound_session: Option<String>,
    deadline: Instant,
    next_poll: Instant,
    measured_epoch: Option<u64>,
}

impl PendingResize {
    pub(crate) fn new(request: &AgentAction, policy: ViewportPolicy) -> Result<Self, BrowserControlFailure> {
        request
            .action
            .validate()
            .map_err(|message| BrowserControlFailure::new("invalid_input", message))?;
        let BrowserControlAction::Resize {
            viewport,
            timeout_millis,
        } = request.action
        else {
            return Err(BrowserControlFailure::new(
                "invalid_action_state",
                "expected viewport resize",
            ));
        };
        let queued_millis = u64::try_from(crate::navigation::now_millis().saturating_sub(request.requested_at_millis))
            .unwrap_or_default();
        let remaining = timeout_millis.saturating_sub(queued_millis);
        if remaining == 0 {
            return Err(resize_timeout());
        }
        let now = Instant::now();
        Ok(Self {
            request: request.clone(),
            requested: viewport,
            target: policy.target(viewport),
            bound_session: None,
            deadline: now + Duration::from_millis(remaining),
            next_poll: now,
            measured_epoch: None,
        })
    }

    pub(crate) fn budget(&self) -> Result<Duration, BrowserControlFailure> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .map(|remaining| remaining.min(Duration::from_millis(100)))
            .ok_or_else(resize_timeout)
    }

    pub(crate) fn guard(
        &self,
        owner: Option<&str>,
        handoff: bool,
        user_active: bool,
        stopping: bool,
    ) -> Result<(), BrowserControlFailure> {
        let failure = if stopping {
            Some(("browser_unavailable", "browser session stopped"))
        } else if handoff {
            Some(("viewport_handoff_pending", "user handoff is pending"))
        } else if user_active {
            Some(("viewport_user_active", "user is steering the panel"))
        } else if owner != Some(self.request.actor.as_str()) {
            Some(("viewport_ownership_lost", "browser ownership changed"))
        } else {
            None
        };
        if let Some((code, message)) = failure {
            return Err(BrowserControlFailure::new(code, message));
        }
        self.budget().map(|_| ())
    }

    pub(crate) fn poll_due(&mut self) -> bool {
        let now = Instant::now();
        if now < self.next_poll {
            return false;
        }
        self.next_poll = now + Duration::from_millis(100);
        true
    }

    pub(crate) fn observe(
        &mut self,
        value: Value,
        epoch: u64,
    ) -> Result<Option<BrowserControlValue>, BrowserControlFailure> {
        self.budget()?;
        let applied: [u32; 2] = serde_json::from_value(value)
            .map_err(|_| BrowserControlFailure::new("invalid_result", "browser returned no CSS viewport dimensions"))?;
        if applied != self.target {
            self.measured_epoch = None;
            return Ok(None);
        }
        if self.measured_epoch.is_some_and(|measured| measured != epoch) {
            return Ok(Some(BrowserControlValue::Viewport {
                requested: self.requested,
                applied,
            }));
        }
        self.measured_epoch = Some(epoch);
        Ok(None)
    }
}

fn resize_timeout() -> BrowserControlFailure {
    BrowserControlFailure::new(
        "viewport_timeout",
        "viewport was not measured at the requested size before the deadline; it may have applied",
    )
}

impl DriverState {
    pub(super) fn begin_resize(&mut self, request: &AgentAction, frame_slot: &FrameSlot) {
        if frame_slot.teach_recording() {
            self.complete_agent_action(
                request,
                Err(BrowserControlFailure::new(
                    "teach_recording",
                    "Teach mode has exclusive ownership of this panel",
                )),
            );
            return;
        }
        match PendingResize::new(request, self.viewport_policy) {
            Ok(pending) => {
                self.finish_pending_resize("viewport_superseded", "a newer resize replaced this request");
                self.pending_resize = Some(pending);
            }
            Err(error) => self.complete_agent_action(request, Err(error)),
        }
    }

    pub(super) fn finish_pending_resize(&mut self, code: &str, message: &str) {
        if let Some(pending) = self.pending_resize.take() {
            if code == "browser_unavailable" {
                self.retain_resize_result(&pending.request);
            }
            self.complete_agent_action(&pending.request, Err(BrowserControlFailure::new(code, message)));
        }
    }

    pub(super) fn tick_pending_resize(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
    ) {
        let Some(mut pending) = self.pending_resize.take() else {
            return;
        };
        let result = self.advance_resize(link, event_tx, frame_slot, &mut pending);
        match result {
            Ok(None) => self.pending_resize = Some(pending),
            Ok(Some(value)) => self.complete_agent_action(&pending.request, Ok(value)),
            Err(error) => {
                self.retain_resize_result(&pending.request);
                self.complete_agent_action(&pending.request, Err(error));
            }
        }
    }

    fn retain_resize_result(&self, request: &AgentAction) {
        if let Some(coordination) = &self.config.coordination {
            coordination.retain_action_result_on_remove(&self.config.panel_local_id, &request.action_id);
        }
    }

    fn advance_resize(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        pending: &mut PendingResize,
    ) -> Result<Option<BrowserControlValue>, BrowserControlFailure> {
        pending.guard(
            self.owner_seen.as_deref(),
            self.handoff_seen.is_some(),
            frame_slot.teach_recording()
                || self
                    .last_user_active_stamp
                    .is_some_and(|at| at.elapsed() < super::USER_ACTIVE_TTL),
            self.stop_requested.load(std::sync::atomic::Ordering::Acquire),
        )?;
        if !pending.poll_due() {
            return Ok(None);
        }
        let target = self.viewport_policy.target(pending.requested);
        if pending.target != target {
            pending.bound_session = None;
            pending.target = target;
        }
        if pending.bound_session.is_none() || pending.bound_session != self.session_id {
            let bound_session = self.session_id.clone();
            let [width, height] = pending.target;
            self.send_page_command_within(
                link,
                event_tx,
                frame_slot,
                "Emulation.setDeviceMetricsOverride",
                &Self::viewport_override_params(width, height),
                pending.budget()?,
            )
            .map_err(|error| BrowserControlFailure::new("viewport_failed", error.to_string()))?;
            self.viewport_policy.commit(pending.requested);
            frame_slot.set_viewport_override(pending.requested);
            self.commit_viewport(width, height, event_tx);
            self.semantic.invalidate();
            self.pending_viewport_capture_at = Some(Instant::now() + VIEWPORT_CAPTURE_DELAY);
            // A reattachment drained by the command restored the old size.
            // Keep the original binding so the next tick reapplies to the new session.
            pending.bound_session = bound_session;
            return Ok(None);
        }
        let Ok(value) = self.evaluate_json_within(link, event_tx, frame_slot, MEASURE_VIEWPORT, pending.budget()?)
        else {
            pending.budget()?;
            return Ok(None);
        };
        if pending.bound_session != self.session_id {
            return Ok(None);
        }
        let result = pending.observe(value, self.signal_epoch)?;
        self.request_signal_refresh();
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{BrowserEventWake, BrowserSessionConfig, CommittedUrl};
    use crate::{AutomationDisclosurePolicy, BrowserConfig};
    use serde_json::json;
    use std::net::TcpListener;
    use std::sync::{atomic::AtomicBool, mpsc};
    use tungstenite::Message;

    fn request(viewport: Option<[u32; 2]>) -> AgentAction {
        AgentAction {
            action_id: "resize".into(),
            actor: "agent".into(),
            requested_at_millis: crate::navigation::now_millis(),
            action: BrowserControlAction::Resize {
                viewport,
                timeout_millis: 2000,
            },
        }
    }

    #[test]
    fn two_panel_policies_keep_independent_pins_and_reset_to_latest_host_size() {
        let mut first = ViewportPolicy::new([1280, 800]);
        let mut second = ViewportPolicy::new([640, 480]);
        for target in [[1440, 900], [820, 1180], [390, 844]] {
            first.commit(Some(target));
            assert!(!first.follow_host([900, 600]));
            assert_eq!(first.explicit, Some(target));
            assert!(second.follow_host([640, 480]));
            assert_eq!(second.explicit, None);
        }
        assert!(!first.follow_host([0, 0]));
        assert_eq!(first.target(None), [900, 600]);
        first.commit(None);
        assert!(first.follow_host([1024, 768]));
    }

    #[test]
    fn measured_match_waits_for_fresh_ownership_then_measures_again() {
        let mut pending = PendingResize::new(&request(Some([390, 844])), ViewportPolicy::new([900, 600])).unwrap();
        assert_eq!(pending.observe(json!([900, 600]), 1).unwrap(), None);
        assert_eq!(pending.observe(json!([390, 844]), 1).unwrap(), None);
        assert_eq!(pending.observe(json!([390, 844]), 1).unwrap(), None);
        assert_eq!(
            pending.observe(json!([390, 844]), 2).unwrap(),
            Some(BrowserControlValue::Viewport {
                requested: Some([390, 844]),
                applied: [390, 844]
            })
        );
        assert_eq!(pending.observe(json!(null), 2).unwrap_err().code, "invalid_result");
        for (owner, handoff, user, stop, code) in [
            (Some("other"), false, false, false, "viewport_ownership_lost"),
            (Some("agent"), true, false, false, "viewport_handoff_pending"),
            (Some("agent"), false, true, false, "viewport_user_active"),
            (Some("agent"), false, false, true, "browser_unavailable"),
        ] {
            assert_eq!(pending.guard(owner, handoff, user, stop).unwrap_err().code, code);
        }
        pending.deadline = Instant::now();
        assert_eq!(
            pending.observe(json!([390, 844]), 3).unwrap_err().code,
            "viewport_timeout"
        );
    }

    #[test]
    fn reset_is_measured_and_invalid_or_expired_requests_do_not_dispatch() {
        let policy = ViewportPolicy::new([900, 600]);
        let mut pending = PendingResize::new(&request(None), policy).unwrap();
        assert_eq!(pending.observe(json!([900, 600]), 1).unwrap(), None);
        assert_eq!(
            pending.observe(json!([900, 600]), 2).unwrap(),
            Some(BrowserControlValue::Viewport {
                requested: None,
                applied: [900, 600]
            })
        );
        for size in [[0, 844], [390, 319], [8001, 844]] {
            assert!(PendingResize::new(&request(Some(size)), policy).is_err());
        }
        for size in [[320, 320], [8000, 8000], [390, 844]] {
            assert!(PendingResize::new(&request(Some(size)), policy).is_ok());
        }
        let mut expired = request(None);
        expired.requested_at_millis -= 3000;
        assert_eq!(
            PendingResize::new(&expired, policy).unwrap_err().code,
            "viewport_timeout"
        );
        for timeout_millis in [0, 60_001, u64::MAX] {
            expired.action = BrowserControlAction::Resize {
                viewport: None,
                timeout_millis,
            };
            assert_eq!(PendingResize::new(&expired, policy).unwrap_err().code, "invalid_input");
        }
    }

    #[test]
    fn frame_override_is_shared_and_cleared_on_backend_restart() {
        let slot = FrameSlot::new();
        let clone = slot.clone();
        slot.set_viewport_override(Some([390, 844]));
        assert_eq!(clone.viewport_override(), Some([390, 844]));
        clone.clear_backend_capabilities();
        assert_eq!(slot.viewport_override(), None);
    }

    #[test]
    fn chromium_refusal_does_not_pin_and_rebinding_reapplies_before_measuring() {
        for refuse in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("ws://{}/", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut socket = tungstenite::accept(stream).unwrap();
                let mut commands = Vec::new();
                while let Ok(Message::Text(text)) = socket.read() {
                    let command: Value = serde_json::from_str(&text).unwrap();
                    let response = if refuse && command["method"] == "Emulation.setDeviceMetricsOverride" {
                        json!({"id": command["id"], "error": {"code": -32000, "message": "refused"}})
                    } else if command["method"] == "Runtime.evaluate" {
                        json!({"id": command["id"], "result": {"result": {"value": [390, 844]}}})
                    } else {
                        json!({"id": command["id"], "result": {}})
                    };
                    commands.push(command);
                    socket.send(Message::Text(response.to_string().into())).unwrap();
                }
                commands
            });
            let slot = Arc::new(FrameSlot::new());
            let config = BrowserSessionConfig {
                browser: BrowserConfig {
                    automation_disclosure: AutomationDisclosurePolicy::BrowserDefault,
                    ..BrowserConfig::default()
                },
                panel_local_id: "resize-test".into(),
                initial_url: None,
                width: 1280,
                height: 800,
                frame_slot: Arc::clone(&slot),
                coordination: None,
                capture_directory: None,
                video: Arc::default(),
                remote: None,
            };
            let mut state = DriverState::new(&config, &url, None, Arc::new(AtomicBool::new(false)));
            state.owner_seen = Some("agent".into());
            state.session_id = Some("first".into());
            let (tx, _rx) = mpsc::channel();
            let events = BrowserEventSender {
                tx,
                wake: BrowserEventWake::default(),
                committed_url: CommittedUrl::default(),
            };
            let mut link = CdpLink::connect(&url).unwrap();
            let mut pending = PendingResize::new(&request(Some([390, 844])), state.viewport_policy).unwrap();
            let result = state.advance_resize(&mut link, &events, &slot, &mut pending);
            if refuse {
                assert_eq!(result.unwrap_err().code, "viewport_failed");
                assert_eq!(slot.viewport_override(), None);
                assert!(state.viewport_policy.follow_host([900, 600]));
            } else {
                assert_eq!(result.unwrap(), None);
                assert_eq!(slot.viewport_override(), Some([390, 844]));
                assert!(!state.viewport_policy.follow_host([900, 600]));
                // A recovered binding must receive the pin before any
                // measurement can acknowledge it, even after host repaint.
                state.session_id = Some("rebound".into());
                for epoch in 1..=3 {
                    pending.next_poll = Instant::now();
                    state.signal_epoch = epoch;
                    let result = state.advance_resize(&mut link, &events, &slot, &mut pending).unwrap();
                    if epoch == 3 {
                        assert!(matches!(
                            result,
                            Some(BrowserControlValue::Viewport {
                                applied: [390, 844],
                                ..
                            })
                        ));
                    } else {
                        assert_eq!(result, None);
                    }
                }
            }
            drop(link);
            let commands = server.join().unwrap();
            let resize_commands: Vec<_> = commands
                .iter()
                .filter(|command| command["method"] == "Emulation.setDeviceMetricsOverride")
                .collect();
            assert_eq!(resize_commands.len(), if refuse { 1 } else { 2 });
            if !refuse {
                assert_eq!(resize_commands[1]["sessionId"], "rebound");
            }
        }
    }
}
