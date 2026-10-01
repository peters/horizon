use super::*;

#[test]
fn handback_acknowledgments_allow_repeated_requests_and_retry_after_failure() {
    let mut panel = BrowserPanelState::inert();
    for reason in ["First request", "Second request"] {
        panel.apply_cloud_state(
            CloudViewState {
                handoff: Some(reason.into()),
                ready: true,
                ..Default::default()
            },
            true,
        );
        assert_eq!(panel.handoff_reason.as_deref(), Some(reason));
        panel.handoff_resolution_pending = true;
        panel.apply_cloud_state(
            CloudViewState {
                handoff: Some(reason.into()),
                ready: true,
                ..Default::default()
            },
            false,
        );
        assert!(
            panel.handoff_resolution_pending,
            "ordinary polling cannot acknowledge hand-back"
        );
        panel.apply_cloud_state(
            CloudViewState {
                ready: true,
                ..Default::default()
            },
            true,
        );
        assert!(!panel.handoff_resolution_pending);
        assert!(panel.handoff_reason.is_none());
    }
    panel.handoff_resolution_pending = true;
    panel.apply_cloud_state(
        CloudViewState {
            handoff: Some("Retry request".into()),
            handoff_error: Some("Acknowledgment failed".into()),
            ..Default::default()
        },
        true,
    );
    assert!(!panel.handoff_resolution_pending);
    assert_eq!(panel.handoff_error.as_deref(), Some("Acknowledgment failed"));
    assert_eq!(panel.handoff_reason.as_deref(), Some("Retry request"));
}

#[test]
#[cfg(unix)]
fn stopped_cloud_keeps_its_environment_and_refuses_local_backend_switch() {
    let mut child = std::process::Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
    child.wait().unwrap();
    let (tx, _rx) = mpsc::sync_channel(1);
    let mut panel = BrowserPanelState::inert();
    panel.cloud = Some(CloudView {
        connection: Connection {
            host: "127.0.0.1".into(),
            port: 1,
            identity: "/unused".into(),
            known_hosts: "/unused".into(),
            host_key_alias: "cloud-fixture".into(),
        },
        id: "fixture".into(),
        initial_url: None,
        backend: super::super::BackendKind::ChromiumCdp,
        target: None,
        device: None,
        process_lost: false,
        tx,
        latest: Latest::default(),
        waker: Waker::default(),
        stop: Arc::new(AtomicBool::new(false)),
        child: Arc::new(Mutex::new(child)),
        handoff_sequence: std::sync::atomic::AtomicU64::new(0),
    });
    for shutdown in [false, true] {
        if shutdown {
            panel.request_shutdown();
        } else {
            panel.stop();
        }
        assert!(panel.is_remote());
        assert!(panel.can_retry());
        let backend = panel.backend();
        panel.switch_backend(horizon_browser::BackendKind::FirefoxBidi);
        assert_eq!(panel.backend(), backend);
        assert!(panel.session.is_none());
        assert!(panel.cloud.is_some());
        assert!(!panel.backend_capabilities().clipboard);
    }
    panel.config.backend = super::super::BackendKind::FirefoxBidi;
    let (reply, responses) = mpsc::sync_channel(1);
    let (_, commands) = mpsc::sync_channel(1);
    let response: CloudViewResponse = serde_json::from_value(serde_json::json!({
        "browsers": [CloudViewState { id: "fixture".into(), lost: true, error: Some("Browser process was lost".into()), ..Default::default() }],
        "error": null,
    })).unwrap();
    reply.send(response).unwrap();
    drop(reply);
    let latest = Latest::default();
    let mut requests = Vec::new();
    pump(
        &mut requests,
        &responses,
        &commands,
        &FrameSlot::new(),
        &latest,
        &Waker::default(),
        &AtomicBool::new(false),
        CloudViewRequest::Open {
            id: "fixture".into(),
            url: None,
            backend: None,
            target: None,
        },
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(requests).unwrap().lines().count(),
        1,
        "lost process must stop polling"
    );
    panel.apply_cloud_state(latest.lock().unwrap().take().unwrap(), false);
    assert_eq!(panel.backend(), super::super::BackendKind::FirefoxBidi);
    assert!(!panel.can_retry());
    let child = panel.cloud.as_ref().unwrap().child.clone();
    panel.relaunch_cloud();
    assert!(Arc::ptr_eq(&child, &panel.cloud.as_ref().unwrap().child));
    panel.apply_cloud_state(
        CloudViewState {
            error: Some("transport interrupted".into()),
            ..Default::default()
        },
        false,
    );
    assert!(
        !panel.can_retry(),
        "transport failure must not forget confirmed process loss"
    );
}

#[test]
#[cfg(unix)]
fn failed_firefox_transport_preserves_engine_for_persistence_and_retry() {
    let connection = Connection {
        host: "127.0.0.1".into(),
        // OpenSSH rejects this before connecting to any real service.
        port: 0,
        identity: "/unused".into(),
        known_hosts: "/unused".into(),
        host_key_alias: "cloud-fixture".into(),
    };
    let config = super::super::BrowserConfig {
        backend: super::super::BackendKind::FirefoxBidi,
        ..Default::default()
    };
    let mut panel = BrowserPanelState::start_cloud(
        "firefox".into(),
        connection,
        Some("catalog.fixture".into()),
        None,
        &config,
    )
    .unwrap();
    for retry in [false, true] {
        if retry {
            panel.relaunch_cloud();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            panel.drain_cloud();
            if matches!(panel.status, BrowserStatus::Error { .. }) {
                break;
            }
            assert!(Instant::now() < deadline, "transport failure was not reported");
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(panel.remote_target(), Some("catalog.fixture"));
        assert_eq!(panel.backend(), super::super::BackendKind::FirefoxBidi);
        let saved = serde_json::to_value(&panel.config).unwrap();
        assert_eq!(saved["backend"], "firefox");
    }
}

#[test]
fn discovered_lost_browser_reports_process_loss_and_preserves_engine() {
    let mut panel = BrowserPanelState::inert();
    panel.apply_cloud_state(
        CloudViewState {
            backend: super::super::BackendKind::FirefoxBidi,
            lost: true,
            ..Default::default()
        },
        false,
    );
    assert!(matches!(panel.status, BrowserStatus::Error { ref message } if message.contains("process was lost")));
    assert_eq!(panel.backend(), super::super::BackendKind::FirefoxBidi);
    assert!(panel.session.is_none());
}

#[test]
fn polling_retries_missing_or_rejected_pixels_until_a_frame_is_stored() {
    let frames = FrameSlot::new();
    let (sender, responses) = mpsc::channel();
    let (_commands, receiver) = mpsc::sync_channel(1);
    for (png, lost) in [
        (Some("not a PNG".to_owned()), false),
        (None, false),
        (
            Some(
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=="
                    .to_owned(),
            ),
            false,
        ),
        (None, true),
    ] {
        sender
            .send(CloudViewResponse {
                browsers: vec![CloudViewState {
                    id: "fixture".into(),
                    sequence: 7,
                    png,
                    lost,
                    ..Default::default()
                }],
                ..Default::default()
            })
            .unwrap();
    }
    let mut requests = Vec::new();
    pump(
        &mut requests,
        &responses,
        &receiver,
        &frames,
        &Latest::default(),
        &Waker::default(),
        &AtomicBool::new(false),
        CloudViewRequest::Open {
            id: "fixture".into(),
            url: None,
            backend: None,
            target: None,
        },
    )
    .unwrap();
    let requests: Vec<CloudViewRequest> = String::from_utf8(requests)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let acknowledgments: Vec<u64> = requests
        .into_iter()
        .filter_map(|request| match request {
            CloudViewRequest::Poll { after, .. } => Some(after),
            _ => None,
        })
        .collect();
    assert_eq!(acknowledgments, [0, 0, 7]);
}

#[test]
fn cloud_orientation_progress_and_failures_reach_panel_without_fatal_error() {
    use crate::browser::remote::{
        OrientationSupport, RemoteOrientation, RemoteOrientationState, RemoteOrientationView,
    };
    let mut panel = BrowserPanelState::inert();
    for pending in [Some(RemoteOrientation::Landscape), None] {
        let view = RemoteOrientationView {
            completed: Vec::new(),
            action_id: None,
            state: RemoteOrientationState {
                support: OrientationSupport::Supported,
                applied: None,
            },
            pending,
            error: pending.is_none().then(|| "orientation_timeout".into()),
        };
        panel.apply_cloud_state(
            CloudViewState {
                ready: true,
                orientation: view.clone(),
                ..CloudViewState::default()
            },
            false,
        );
        assert_eq!(panel.orientation, view);
        assert!(matches!(panel.status, BrowserStatus::Ready));
    }
}

#[test]
fn stale_cloud_poll_cannot_clear_a_queued_rotation_until_matching_acknowledgement() {
    use crate::browser::remote::{RemoteOrientation, RemoteOrientationView};
    let mut panel = BrowserPanelState::inert();
    panel.orientation = RemoteOrientationView {
        action_id: Some("current".into()),
        pending: Some(RemoteOrientation::Landscape),
        ..RemoteOrientationView::default()
    };
    panel.orientation_pending_since = Some(std::time::Instant::now());
    panel.apply_cloud_state(
        CloudViewState {
            ready: true,
            ..CloudViewState::default()
        },
        false,
    );
    assert_eq!(panel.orientation.pending, Some(RemoteOrientation::Landscape));
    panel.apply_cloud_state(
        CloudViewState {
            ready: true,
            orientation: RemoteOrientationView {
                action_id: Some("current".into()),
                ..RemoteOrientationView::default()
            },
            ..CloudViewState::default()
        },
        false,
    );
    assert!(panel.orientation.pending.is_none());
}

#[test]
fn another_cloud_viewers_rotation_and_worker_queue_refusal_settle_the_original_request() {
    use crate::browser::remote::{RemoteOrientation, RemoteOrientationCompletion, RemoteOrientationView};
    for error in ["orientation_superseded", "orientation_queue_rejected"] {
        let mut panel = BrowserPanelState::inert();
        panel.orientation = RemoteOrientationView {
            action_id: Some("viewer-a".into()),
            pending: Some(RemoteOrientation::Landscape),
            ..RemoteOrientationView::default()
        };
        panel.orientation_pending_since = Some(std::time::Instant::now());
        panel.apply_cloud_state(
            CloudViewState {
                ready: true,
                orientation: RemoteOrientationView {
                    action_id: Some("viewer-b".into()),
                    completed: vec![RemoteOrientationCompletion {
                        action_id: "viewer-a".into(),
                        error: Some(error.into()),
                    }],
                    ..RemoteOrientationView::default()
                },
                ..CloudViewState::default()
            },
            false,
        );
        assert!(panel.orientation.pending.is_none());
        assert_eq!(panel.orientation.error.as_deref(), Some(error));
        let repeat = CloudViewState {
            ready: true,
            orientation: RemoteOrientationView {
                action_id: Some("viewer-b".into()),
                completed: vec![RemoteOrientationCompletion {
                    action_id: "viewer-a".into(),
                    error: Some(error.into()),
                }],
                ..RemoteOrientationView::default()
            },
            ..CloudViewState::default()
        };
        for _ in 0..3 {
            panel.apply_cloud_state(repeat.clone(), false);
            assert_eq!(panel.orientation.error.as_deref(), Some(error));
            assert_eq!(panel.orientation.action_id.as_deref(), Some("viewer-a"));
        }
    }
}
