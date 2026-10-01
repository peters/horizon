use super::super::viewport::tests::{bidi_fixture, events, fixture_driver};
use super::*;
use crate::webdriver::test_server::{Reply, Server};
use serde_json::json;

mod navigation;
mod recovery;
mod startup;

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
    std::sync::Mutex<Vec<Option<crate::remote::RemoteOrientationState>>>,
);
impl crate::BrowserCoordination for Owner {
    fn prepare(&self, _: &str, _: Duration) -> bool {
        true
    }
    fn initialize(&self, _: &str, state: &crate::CoordinationState) -> std::io::Result<()> {
        self.2.lock().unwrap().push(state.remote_orientation);
        Ok(())
    }
    fn update(&self, _: &str, state: &crate::CoordinationState) -> std::io::Result<()> {
        self.2.lock().unwrap().push(state.remote_orientation);
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
fn user_rotation_uses_measured_frames_without_agent_ownership_and_rejects_other_actions() {
    use crate::BrowserAuditStatus;
    use std::sync::Arc;
    let mut replies = vec![Reply::json(200, &json!({"value":null}))];
    replies.extend(observation(4, 2));
    replies.push(Reply::json(200, &json!({"value":"document"})));
    let classic = Server::start(replies);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    let owner = Arc::new(Owner(
        std::sync::Mutex::new(None),
        std::sync::Mutex::new(Vec::new()),
        std::sync::Mutex::new(Vec::new()),
    ));
    driver.owner_seen = None;
    driver.config.coordination = Some(owner.clone());
    driver.remote_orientation = Some(crate::remote::RemoteOrientationState::default());
    driver.begin_user_orientation(
        "user-rotation".into(),
        RemoteOrientation::Landscape,
        &events(),
        &AtomicBool::new(false),
    );
    assert!(matches!(
        driver.pending_orientation.as_ref().unwrap().origin,
        Origin::User { .. }
    ));
    assert_eq!(*owner.1.lock().unwrap(), vec![BrowserAuditStatus::Dispatched]);
    let mut action = rotation_request();
    action.action = BrowserControlAction::Reload;
    driver.service_browser_request(&action, &events(), &AtomicBool::new(false));
    assert_eq!(
        *owner.1.lock().unwrap(),
        vec![
            BrowserAuditStatus::Dispatched,
            BrowserAuditStatus::Rejected,
            BrowserAuditStatus::Failed
        ]
    );
    assert_eq!(
        classic.recorded().len(),
        1,
        "refused action must not reach the provider"
    );
    driver.last_user_active_stamp = Instant::now().checked_sub(Duration::from_secs(6));
    let mut competing = rotation_request();
    competing.actor = "agent".into();
    driver.owner_seen = Some("agent".into());
    assert_eq!(
        driver
            .prepare_orientation(&competing, &AtomicBool::new(false))
            .err()
            .unwrap()
            .code,
        "orientation_user_active"
    );
    driver.owner_seen = None; // Expired agent leases cannot cancel human steering.
    driver.tick_orientation(&events(), &AtomicBool::new(false));
    assert!(
        driver.pending_orientation.is_some(),
        "fresh ownership read is still required for users"
    );
    assert!(
        driver.pending_orientation.as_ref().unwrap().verified.is_some(),
        "GET, page geometry, fresh screenshot and document identity were verified"
    );
    assert_eq!(classic.recorded().len(), 6);
    driver.signal_epoch += 1;
    driver.tick_orientation(&events(), &AtomicBool::new(false));
    assert!(driver.pending_orientation.is_none());
    assert_eq!(
        driver.remote_orientation.unwrap().applied,
        Some(RemoteOrientation::Landscape)
    );
    assert_eq!(owner.1.lock().unwrap().last(), Some(&BrowserAuditStatus::Completed));
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn user_supersession_retains_the_previous_request_completion() {
    let classic = Server::start(vec![
        Reply::json(200, &json!({"value":null})),
        Reply::json(200, &json!({"value":null})),
    ]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    driver.remote_orientation = Some(crate::remote::RemoteOrientationState::default());
    for id in ["viewer-a", "viewer-b"] {
        driver.begin_user_orientation(
            id.into(),
            RemoteOrientation::Landscape,
            &events(),
            &AtomicBool::new(false),
        );
    }
    assert_eq!(driver.orientation_action_id.as_deref(), Some("viewer-b"));
    assert_eq!(driver.orientation_completed.completed[0].action_id, "viewer-a");
    assert!(
        driver.orientation_completed.completed[0]
            .error
            .as_deref()
            .unwrap()
            .contains("orientation_superseded")
    );
    driver.finish_orientation("browser_unavailable", "test stop");
    assert_eq!(driver.orientation_completed.completed.len(), 2);
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn user_rotation_failure_and_input_takeover_preserve_nonfatal_ui_status() {
    use crate::BrowserAuditStatus;
    use std::sync::Arc;
    for stop_before in [false, true] {
        let classic = Server::start(vec![Reply::json(200, &json!({"value":null}))]);
        let (link, worker) = bidi_fixture(false, false);
        let mut driver = fixture_driver(&classic, link);
        let owner = Arc::new(Owner(
            std::sync::Mutex::new(Some("agent".into())),
            std::sync::Mutex::new(Vec::new()),
            std::sync::Mutex::new(Vec::new()),
        ));
        driver.config.coordination = Some(owner.clone());
        driver.remote_orientation = Some(crate::remote::RemoteOrientationState::default());
        driver.begin_user_orientation(
            "user-rotation".into(),
            RemoteOrientation::Landscape,
            &events(),
            &AtomicBool::new(stop_before),
        );
        if stop_before {
            assert!(classic.recorded().is_empty());
            assert_eq!(
                *owner.1.lock().unwrap(),
                vec![BrowserAuditStatus::Rejected, BrowserAuditStatus::Failed]
            );
            assert!(
                driver
                    .orientation_error
                    .as_deref()
                    .unwrap()
                    .contains("browser_unavailable")
            );
        } else {
            driver.panel_slot.set_teach_recording(true);
            driver.tick_orientation(&events(), &AtomicBool::new(false));
            assert_eq!(
                *owner.1.lock().unwrap(),
                vec![BrowserAuditStatus::Dispatched, BrowserAuditStatus::Failed]
            );
            assert!(
                driver
                    .orientation_error
                    .as_deref()
                    .unwrap()
                    .contains("orientation_user_active")
            );
        }
        assert!(driver.pending_orientation.is_none());
        drop(driver);
        assert!(worker.join().unwrap().is_empty());
    }
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

#[test]
fn startup_remeasures_committed_page_instead_of_reusing_allocation_geometry() {
    let classic = Server::start(vec![
        Reply::json(200, &json!({"value":"LANDSCAPE"})),
        Reply::json(
            200,
            &json!({"value":{"width":600,"height":900,"visual_width":600,"visual_height":900,"orientation":"portrait"}}),
        ),
    ]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    driver.remote_orientation = Some(crate::remote::RemoteOrientationState {
        support: OrientationSupport::Supported,
        applied: Some(RemoteOrientation::Landscape),
    });
    driver.orientation_document = DocumentOrientation::NeedsPublication;
    driver.refresh_document_orientation(&events());
    assert_eq!(
        driver.remote_orientation.as_ref().unwrap().support,
        OrientationSupport::Supported
    );
    assert_eq!(driver.remote_orientation.as_ref().unwrap().applied, None);
    assert_eq!(driver.orientation_document, DocumentOrientation::Clean);
    assert_eq!(classic.recorded().len(), 2);
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn document_identity_invalidation_defers_measurement_outside_bounded_observation() {
    let classic = Server::start(vec![
        Reply::json(200, &json!({"value":"new-document"})),
        Reply::json(200, &json!({"value":"LANDSCAPE"})),
        Reply::json(
            200,
            &json!({"value":{"width":600,"height":900,"visual_width":600,"visual_height":900,"orientation":"portrait"}}),
        ),
    ]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    driver.classic_document_identity = Some("old-document".into());
    driver.remote_orientation = Some(crate::remote::RemoteOrientationState {
        support: OrientationSupport::Supported,
        applied: Some(RemoteOrientation::Landscape),
    });
    assert!(
        driver
            .refresh_classic_document_identity_within(Duration::from_secs(1))
            .unwrap()
    );
    assert_eq!(driver.remote_orientation.as_ref().unwrap().applied, None);
    assert_ne!(driver.orientation_document, DocumentOrientation::Clean);
    assert_eq!(
        classic.recorded().len(),
        1,
        "identity checks must not add an orientation roundtrip"
    );
    driver.refresh_document_orientation(&events());
    assert_eq!(driver.remote_orientation.as_ref().unwrap().applied, None);
    assert_eq!(driver.orientation_document, DocumentOrientation::Clean);
    assert_eq!(classic.recorded().len(), 3);
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn synchronous_remote_navigation_schedules_fresh_document_measurement() {
    let classic = Server::start(vec![
        Reply::json(200, &json!({"value":null})),
        Reply::json(200, &json!({"value":"https://example.test/next"})),
        Reply::json(200, &json!({"value":"https://example.test/next"})),
        Reply::json(200, &json!({"value":"Next"})),
        Reply::json(200, &json!({"value":"new-document"})),
        Reply::json(200, &json!({"value":"PORTRAIT"})),
        Reply::json(
            200,
            &json!({"value":{"width":600,"height":900,"visual_width":600,"visual_height":900,"orientation":"portrait"}}),
        ),
    ]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    driver.classic_document_identity = Some("old-document".into());
    driver.remote_orientation = Some(crate::remote::RemoteOrientationState {
        support: OrientationSupport::Supported,
        applied: Some(RemoteOrientation::Landscape),
    });
    driver.navigate("https://example.test/next", &events()).unwrap();
    assert_eq!(driver.remote_orientation.as_ref().unwrap().applied, None);
    assert_ne!(driver.orientation_document, DocumentOrientation::Clean);
    driver.refresh_document_orientation(&events());
    assert_eq!(
        driver.remote_orientation.as_ref().unwrap().applied,
        Some(RemoteOrientation::Portrait)
    );
    assert_eq!(driver.orientation_document, DocumentOrientation::Clean);
    assert_eq!(classic.recorded().len(), 7);
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn document_remeasurement_cannot_take_over_pending_runtime_rotation() {
    let classic = Server::start(vec![Reply::json(200, &json!({"value":null}))]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    driver.owner_seen = Some("agent".into());
    driver.remote_orientation = Some(crate::remote::RemoteOrientationState::default());
    driver.begin_orientation(&rotation_request(), &AtomicBool::new(false));
    driver.invalidate_document_orientation();
    driver.refresh_document_orientation(&events());
    assert!(driver.pending_orientation.is_some());
    assert_ne!(driver.orientation_document, DocumentOrientation::Clean);
    assert_eq!(classic.recorded().len(), 1, "the active request owns measurement");
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn background_document_measurement_waits_for_bounded_actions_to_settle() {
    let classic = Server::start(vec![
        Reply::json(200, &json!({"value":"PORTRAIT"})),
        Reply::json(
            200,
            &json!({"value":{"width":600,"height":900,"visual_width":600,"visual_height":900,"orientation":"portrait"}}),
        ),
    ]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    driver.remote_orientation = Some(crate::remote::RemoteOrientationState {
        support: OrientationSupport::Supported,
        applied: Some(RemoteOrientation::Landscape),
    });
    let (tx, event_rx) = std::sync::mpsc::channel();
    let event_tx = crate::session::BrowserEventSender { tx, ..events() };
    driver.invalidate_document_orientation();
    let mut request = rotation_request();
    request.action = BrowserControlAction::WaitForSelector {
        selector: "#ready".into(),
        state: crate::SelectorState::Visible,
        timeout_millis: Some(1000),
    };
    driver.pending_wait = Some(crate::wait::PendingWait::new(
        request.clone(),
        "#ready".into(),
        crate::SelectorState::Visible,
        Some(1000),
        Duration::ZERO,
        driver.semantic.generation(),
        Instant::now(),
    ));
    driver.refresh_document_orientation(&event_tx);
    assert!(classic.recorded().is_empty());
    assert_eq!(driver.orientation_document, DocumentOrientation::NeedsMeasurement);
    assert!(matches!(
        event_rx.try_recv().unwrap(),
        crate::session::BrowserEvent::OrientationChanged(view) if view.state.applied.is_none()
    ));
    driver.refresh_document_orientation(&event_tx);
    assert!(event_rx.try_recv().is_err());
    assert!(classic.recorded().is_empty());
    assert_eq!(driver.remote_orientation.as_ref().unwrap().applied, None);
    driver.pending_wait = None;
    request.action = BrowserControlAction::Navigate {
        url: "https://example.test/next".into(),
        wait: crate::NavigationWait::Commit,
        timeout_millis: Some(1000),
    };
    driver.pending_navigation = Some(crate::navigation::PendingNavigation::new(
        request,
        "https://example.test/next".into(),
        crate::NavigationWait::Commit,
        Duration::from_secs(1),
        Duration::ZERO,
        Instant::now(),
    ));
    driver.refresh_document_orientation(&events());
    assert!(classic.recorded().is_empty());
    assert_ne!(driver.orientation_document, DocumentOrientation::Clean);
    driver.pending_navigation = None;
    driver.refresh_document_orientation(&events());
    assert_eq!(classic.recorded().len(), 2);
    assert_eq!(driver.orientation_document, DocumentOrientation::Clean);
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}
