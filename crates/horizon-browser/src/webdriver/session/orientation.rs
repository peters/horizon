//! Rotation requests observed from the servicing loop, with current ownership guards.
use super::super::orientation::{observe, remaining, set};
use super::Driver;
use crate::remote::{OrientationSupport, RemoteOrientation};
use crate::session::BrowserEventSender;
use crate::{AgentAction, BrowserControlAction, BrowserControlFailure, BrowserControlValue};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub(super) struct Pending {
    request: AgentAction,
    requested: RemoteOrientation,
    deadline: Instant,
    next_sample: Instant,
    generation: u64,
    verified: Option<([u32; 2], u64)>,
}
impl Driver {
    fn orientation_human_active(&self) -> bool {
        self.panel_slot.teach_recording()
            || self
                .last_user_active_stamp
                .is_some_and(|at| at.elapsed() < super::coordination::USER_ACTIVE_TTL)
    }
    pub(super) fn begin_orientation(&mut self, request: &AgentAction, stopped: &AtomicBool) {
        let prepared = self.prepare_orientation(request, stopped);
        match prepared {
            Ok(pending) => {
                self.audit_agent_action(request, crate::BrowserAuditStatus::Dispatched);
                if let Err(error) = self.dispatch_orientation(pending) {
                    self.complete_agent_action(request, Err(error));
                }
            }
            Err(error) => {
                self.audit_agent_action(request, crate::BrowserAuditStatus::Rejected);
                self.complete_agent_action(request, Err(error));
            }
        }
    }
    fn prepare_orientation(
        &self,
        request: &AgentAction,
        stopped: &AtomicBool,
    ) -> Result<Pending, BrowserControlFailure> {
        if stopped.load(Ordering::Acquire) {
            return Err(BrowserControlFailure::new(
                "browser_unavailable",
                "the browser is stopping",
            ));
        }
        request
            .action
            .validate()
            .map_err(|message| BrowserControlFailure::new("invalid_input", message))?;
        if self.owner_seen.as_deref() != Some(request.actor.as_str()) {
            return Err(BrowserControlFailure::new(
                "orientation_ownership_lost",
                "the requesting agent no longer owns this panel",
            ));
        }
        if self.handoff_seen.is_some() {
            return Err(BrowserControlFailure::new(
                "orientation_handoff_pending",
                "the panel is awaiting human steering",
            ));
        }
        let BrowserControlAction::Orientation {
            orientation,
            timeout_millis,
        } = request.action
        else {
            return Err(BrowserControlFailure::new(
                "invalid_action_state",
                "expected orientation action",
            ));
        };
        let state = self.remote_orientation.as_ref().ok_or_else(|| {
            BrowserControlFailure::new("orientation_unsupported", "local browsers use browser_resize")
        })?;
        if state.support == OrientationSupport::Unsupported {
            return Err(BrowserControlFailure::new(
                "orientation_unsupported",
                "this endpoint does not support orientation",
            ));
        }
        if self.orientation_human_active() {
            return Err(BrowserControlFailure::new(
                "orientation_user_active",
                "human input or Teach mode has exclusive ownership",
            ));
        }
        let queued =
            u64::try_from(crate::navigation::now_millis().saturating_sub(request.requested_at_millis)).unwrap_or(0);
        let deadline = Instant::now() + Duration::from_millis(timeout_millis.saturating_sub(queued));
        remaining(deadline)?;
        Ok(Pending {
            request: request.clone(),
            requested: orientation,
            deadline,
            next_sample: Instant::now(),
            generation: self.semantic.generation(),
            verified: None,
        })
    }
    fn dispatch_orientation(&mut self, mut pending: Pending) -> Result<(), BrowserControlFailure> {
        self.finish_orientation(
            "orientation_superseded",
            "a newer rotation superseded this request; inspect applied orientation",
        );
        // A POST may mutate even when its response is lost. Discard stale geometry first.
        if let Some(state) = self.remote_orientation.as_mut() {
            state.applied = None;
        }
        self.note_remote_activity();
        self.semantic.invalidate();
        self.advance_viewport_generation();
        self.frames.invalidate();
        let result = set(
            self.host.transport(),
            &format!("/session/{}", self.session_id),
            pending.requested,
            pending.deadline,
        );
        if let Err(error) = result {
            if error.code == "orientation_unsupported"
                && let Some(state) = self.remote_orientation.as_mut()
            {
                state.support = OrientationSupport::Unsupported;
            }
            return Err(error);
        }
        pending.generation = self.semantic.generation();
        self.pending_orientation = Some(pending);
        Ok(())
    }
    pub(super) fn finish_orientation(&mut self, code: &str, message: &str) {
        if let Some(pending) = self.pending_orientation.take() {
            if let Some(coordination) = &self.config.coordination {
                coordination.retain_action_result_on_remove(&self.config.panel_local_id, &pending.request.action_id);
            }
            self.complete_agent_action(&pending.request, Err(BrowserControlFailure::new(code, message)));
        }
    }
    pub(super) fn tick_orientation(&mut self, events: &BrowserEventSender, stopped: &AtomicBool) {
        let Some(mut pending) = self.pending_orientation.take() else {
            return;
        };
        let result = self.advance_orientation(&mut pending, stopped, events);
        match result {
            Ok(Some(viewport)) => {
                // Publish a fresh frame and invalidate refs made during the transition.
                self.semantic.invalidate();

                if let Some(state) = self.remote_orientation.as_mut() {
                    state.support = OrientationSupport::Supported;
                    state.applied = Some(pending.requested);
                }
                self.coordination_dirty = true;
                self.write_coordination(true);
                self.complete_agent_action(
                    &pending.request,
                    Ok(BrowserControlValue::Orientation {
                        requested: pending.requested,
                        applied: pending.requested,
                        viewport,
                    }),
                );
            }
            Ok(None) => self.pending_orientation = Some(pending),
            Err(error) => {
                if error.code == "orientation_unsupported"
                    && let Some(state) = self.remote_orientation.as_mut()
                {
                    state.support = OrientationSupport::Unsupported;
                    self.coordination_dirty = true;
                }
                self.complete_agent_action(&pending.request, Err(error));
            }
        }
    }
    fn guard_orientation(&mut self, pending: &Pending, stopped: &AtomicBool) -> Result<(), BrowserControlFailure> {
        if stopped.load(Ordering::Acquire) || self.host.has_exited() {
            return Err(BrowserControlFailure::new(
                "browser_unavailable",
                "browser stopped; orientation may have changed",
            ));
        }
        if self.owner_seen.as_deref() != Some(pending.request.actor.as_str()) {
            return Err(BrowserControlFailure::new(
                "orientation_ownership_lost",
                "ownership changed while rotating; inspect applied orientation",
            ));
        }
        if self.handoff_seen.is_some() {
            return Err(BrowserControlFailure::new(
                "orientation_handoff_pending",
                "handoff started while rotating; inspect applied orientation",
            ));
        }
        if self.orientation_human_active() {
            return Err(BrowserControlFailure::new(
                "orientation_user_active",
                "human input or Teach mode took over; inspect applied orientation",
            ));
        }
        remaining(pending.deadline)?;
        if pending.generation != self.semantic.generation()
            || self.classic_navigation_in_flight()
            || self.pending_navigation.is_some()
        {
            return Err(BrowserControlFailure::new(
                "orientation_navigation_invalidated",
                "the document changed while rotating; inspect applied orientation",
            ));
        }
        Ok(())
    }
    fn advance_orientation(
        &mut self,
        pending: &mut Pending,
        stopped: &AtomicBool,
        events: &BrowserEventSender,
    ) -> Result<Option<[u32; 2]>, BrowserControlFailure> {
        self.guard_orientation(pending, stopped)?;
        if let Some((viewport, epoch)) = pending.verified {
            if self.signal_epoch > epoch {
                return Ok(Some(viewport));
            }
            return Ok(None);
        }
        if Instant::now() < pending.next_sample {
            return Ok(None);
        }
        let result = observe(
            self.host.transport(),
            &format!("/session/{}", self.session_id),
            pending.requested,
            pending.deadline,
        );
        pending.next_sample = Instant::now() + Duration::from_millis(100);
        match result {
            Ok(Some(measured)) => {
                self.frames.invalidate();
                let frame_slot = self.config.frame_slot.clone();
                let old_sequence = frame_slot.latest().map(|frame| frame.seq);
                self.capture_frame_until(&frame_slot, events, Some(pending.deadline));
                remaining(pending.deadline)?;
                if !frame_slot.latest().is_some_and(|frame| {
                    Some(frame.seq) != old_sequence
                        && frame.width != frame.height
                        && (frame.width > frame.height) == (pending.requested == RemoteOrientation::Landscape)
                }) {
                    return Ok(None);
                }
                self.refresh_classic_document_identity_within(
                    remaining(pending.deadline)?.min(Duration::from_secs(3)),
                )?;
                self.guard_orientation(pending, stopped)?;
                pending.verified = Some(([measured.width, measured.height], self.signal_epoch));
                // Only a subsequent successful coordination read can acknowledge
                // ownership after all blocking driver calls. Its actions stay
                // on the normal servicing path rather than being drained here.
                self.request_signal_refresh();
                Ok(None)
            }
            Err(error) if error.code == "orientation_unsupported" => Err(error),
            _ => {
                remaining(pending.deadline)?;
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::viewport::tests::{bidi_fixture, events, fixture_driver};
    use super::*;
    use crate::webdriver::test_server::{Reply, Server};
    use serde_json::json;

    fn rotation_request() -> AgentAction {
        AgentAction {
            action_id: "rotation".into(),
            actor: "agent".into(),
            requested_at_millis: crate::navigation::now_millis(),
            action: BrowserControlAction::Orientation {
                orientation: RemoteOrientation::Landscape,
                timeout_millis: 5000,
            },
        }
    }
    fn image(width: u32, height: u32) -> String {
        use base64::Engine;
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Grayscale);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&vec![0; (width * height) as usize])
                .unwrap();
        }
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }
    fn observation(width: u32, height: u32) -> Vec<Reply> {
        vec![
            Reply::json(200, &json!({"value":"LANDSCAPE"})),
            Reply::json(
                200,
                &json!({"value":{"width":900,"height":600,"visual_width":900,"visual_height":600,"orientation":"landscape"}}),
            ),
            Reply::json(200, &json!({"value":image(width, height)})),
            Reply::json(200, &json!({"value":null})),
        ]
    }

    #[derive(Debug)]
    struct Owner(
        std::sync::Mutex<Option<String>>,
        std::sync::Mutex<Vec<crate::BrowserAuditStatus>>,
    );
    impl crate::BrowserCoordination for Owner {
        fn prepare(&self, _: &str, _: Duration) -> bool {
            true
        }
        fn initialize(&self, _: &str, _: &crate::CoordinationState) -> std::io::Result<()> {
            Ok(())
        }
        fn update(&self, _: &str, _: &crate::CoordinationState) -> std::io::Result<()> {
            Ok(())
        }
        fn set_user_active(&self, _: &str, _: bool) -> std::io::Result<()> {
            Ok(())
        }
        fn signals(&self, _: &str) -> std::io::Result<crate::CoordinationSignals> {
            Ok(crate::CoordinationSignals {
                owner: self.0.lock().unwrap().clone(),
                ..Default::default()
            })
        }
        fn acknowledge_handoff(&self, _: &str, _: &str) -> std::io::Result<bool> {
            Ok(false)
        }
        fn remove(&self, _: &str, _: Duration) -> bool {
            true
        }
        fn record_action(&self, _: &str, entry: &crate::BrowserAuditEntry) -> std::io::Result<()> {
            self.1.lock().unwrap().push(entry.status);
            Ok(())
        }
    }

    #[test]
    fn dispatch_is_audited_before_provider_reply_and_refusals_do_not_dispatch() {
        use crate::BrowserAuditStatus;
        use std::sync::Arc;
        let classic = Server::start(vec![
            Reply::json(200, &json!({"value":null})).delayed(Duration::from_millis(200)),
        ]);
        let (link, worker) = bidi_fixture(false, false);
        let mut driver = fixture_driver(&classic, link);
        let owner = Arc::new(Owner(
            std::sync::Mutex::new(Some("agent".into())),
            std::sync::Mutex::new(Vec::new()),
        ));
        driver.config.coordination = Some(owner.clone());
        driver.remote_orientation = Some(crate::remote::RemoteOrientationState::default());
        let mut invalid = rotation_request();
        invalid.actor = "other".into();
        driver.begin_orientation(&invalid, &AtomicBool::new(false));
        assert_eq!(
            *owner.1.lock().unwrap(),
            vec![BrowserAuditStatus::Rejected, BrowserAuditStatus::Failed]
        );
        assert!(classic.recorded().is_empty());
        owner.1.lock().unwrap().clear();
        let rotation = std::thread::spawn(move || {
            driver.begin_orientation(&rotation_request(), &AtomicBool::new(false));
            driver
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while classic.recorded().is_empty() {
            assert!(Instant::now() < deadline, "orientation POST never reached the mock");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(*owner.1.lock().unwrap(), vec![BrowserAuditStatus::Dispatched]);
        let driver = rotation.join().unwrap();
        assert!(driver.pending_orientation.is_some());
        drop(driver);
        assert!(worker.join().unwrap().is_empty());
    }

    #[test]
    fn stale_portrait_frames_wait_and_takeover_during_identity_cannot_succeed() {
        use std::sync::Arc;
        for cancel in [false, true] {
            let mut replies = vec![Reply::json(200, &json!({"value":null}))];
            replies.extend(observation(2, 4));
            replies.extend(observation(4, 2));
            replies.push(Reply::json(200, &json!({"value":"document"})).delayed(Duration::from_millis(100)));
            let classic = Server::start(replies);
            let (link, worker) = bidi_fixture(false, false);
            let mut driver = fixture_driver(&classic, link);
            let owner = Arc::new(Owner(
                std::sync::Mutex::new(Some("agent".into())),
                std::sync::Mutex::new(Vec::new()),
            ));
            driver.config.coordination = Some(owner.clone());
            driver.remote_orientation = Some(crate::remote::RemoteOrientationState::default());
            driver.begin_orientation(&rotation_request(), &AtomicBool::new(false));
            let mut pending = driver.pending_orientation.take().unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            assert_eq!(
                driver.advance_orientation(&mut pending, &stop, &events()).unwrap(),
                None
            );
            assert!(
                pending.verified.is_none(),
                "new sequence with portrait pixels is still stale"
            );
            assert_eq!(classic.recorded().len(), 5);
            pending.next_sample = Instant::now();
            driver.scrollbar.refresh_at = Instant::now();
            let signal = stop.clone();
            let takeover = std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
                while classic.recorded().len() < 10 {
                    assert!(Instant::now() < deadline, "identity request never reached the mock");
                    std::thread::sleep(Duration::from_millis(1));
                }
                *owner.0.lock().unwrap() = Some("replacement".into());
                signal.store(cancel, Ordering::Release);
                classic
            });
            let result = driver.advance_orientation(&mut pending, &stop, &events());
            if cancel {
                assert_eq!(result.unwrap_err().code, "browser_unavailable");
            } else {
                assert_eq!(result.unwrap(), None);
                assert!(pending.verified.is_some());
                assert!(driver.tick_coordination(&events()).is_empty());
                assert_eq!(
                    driver
                        .advance_orientation(&mut pending, &stop, &events())
                        .unwrap_err()
                        .code,
                    "orientation_ownership_lost"
                );
            }
            drop(takeover.join().unwrap());
            drop(driver);
            assert!(worker.join().unwrap().is_empty());
        }
    }

    #[test]
    fn a_stalled_screenshot_uses_the_remaining_rotation_deadline() {
        let mut replies = vec![Reply::json(200, &json!({"value":null}))];
        let mut sample = observation(4, 2);
        sample.pop();
        sample[2].delay = Duration::from_secs(1);
        replies.extend(sample);
        let classic = Server::start(replies);
        let (link, worker) = bidi_fixture(false, false);
        let mut driver = fixture_driver(&classic, link);
        driver.remote_orientation = Some(crate::remote::RemoteOrientationState::default());
        driver.begin_orientation(&rotation_request(), &AtomicBool::new(false));
        let mut pending = driver.pending_orientation.take().unwrap();
        pending.deadline = Instant::now() + Duration::from_millis(300);
        let started = Instant::now();
        assert_eq!(
            driver
                .advance_orientation(&mut pending, &AtomicBool::new(false), &events())
                .unwrap_err()
                .code,
            "orientation_timeout"
        );
        assert!(started.elapsed() < Duration::from_millis(750));
        assert_eq!(classic.recorded().last().unwrap().path, "/session/test/screenshot");
        drop(driver);
        assert!(worker.join().unwrap().is_empty());
    }

    #[test]
    fn rotation_invalidates_refs_and_waits_for_current_ownership() {
        let classic = Server::start(vec![Reply::json(200, &json!({"value":null}))]);
        let (link, worker) = bidi_fixture(false, false);
        let mut driver = fixture_driver(&classic, link);
        driver.remote_orientation = Some(crate::remote::RemoteOrientationState::default());
        let request = rotation_request();
        let before = driver.semantic.generation();
        driver
            .panel_slot
            .publish_native_select_popup(crate::native_select::NativeSelectPopup {
                css_path: "#choice".into(),
                name: "choice".into(),
                selected_index: 0,
                bounds: crate::BrowserBounds {
                    x: 1.0,
                    y: 1.0,
                    width: 2.0,
                    height: 2.0,
                },
                options: Vec::new(),
            });
        driver.begin_orientation(&request, &AtomicBool::new(false));
        assert_ne!(driver.semantic.generation(), before);
        assert!(
            driver.panel_slot.native_select_popup().is_none(),
            "popup coordinates belonged to the previous viewport"
        );
        let mut pending = driver.pending_orientation.take().unwrap();
        pending.verified = Some(([900, 600], driver.signal_epoch));
        let stop = AtomicBool::new(false);
        assert_eq!(
            driver.advance_orientation(&mut pending, &stop, &events()).unwrap(),
            None
        );
        driver.signal_epoch += 1;
        driver.owner_seen = Some("replacement".into());
        assert_eq!(
            driver
                .advance_orientation(&mut pending, &stop, &events())
                .unwrap_err()
                .code,
            "orientation_ownership_lost"
        );
        driver.owner_seen = Some("agent".into());
        driver.handoff_seen = Some("handoff".into());
        assert_eq!(
            driver.guard_orientation(&pending, &stop).unwrap_err().code,
            "orientation_handoff_pending"
        );
        driver.handoff_seen = None;
        driver.panel_slot.set_teach_recording(true);
        assert_eq!(
            driver.guard_orientation(&pending, &stop).unwrap_err().code,
            "orientation_user_active"
        );
        driver.panel_slot.set_teach_recording(false);
        stop.store(true, Ordering::Release);
        assert_eq!(
            driver.guard_orientation(&pending, &stop).unwrap_err().code,
            "browser_unavailable"
        );
        stop.store(false, Ordering::Release);
        driver.semantic.invalidate();
        assert_eq!(
            driver.guard_orientation(&pending, &stop).unwrap_err().code,
            "orientation_navigation_invalidated"
        );
        drop(driver);
        assert!(worker.join().unwrap().is_empty());
        assert_eq!(classic.recorded().len(), 1);
        assert_eq!(classic.recorded()[0].path, "/session/test/orientation");
    }
}
