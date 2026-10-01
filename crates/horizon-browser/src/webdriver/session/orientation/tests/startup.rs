use super::*;
use crate::session::{BrowserEvent, BrowserEventSender};
use std::sync::{Arc, Mutex, mpsc};

#[test]
fn startup_publishes_unverified_before_navigation_and_stays_unverified_while_pending() {
    let classic = Server::start(vec![
        Reply::json(200, &json!({"value":"old-document"})),
        Reply::json(200, &json!({"value":null})),
        Reply::json(
            500,
            &json!({"value":{"error":"timeout","message":"page load timed out"}}),
        ),
        Reply::json(200, &json!({"value":null})),
    ]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    let owner = Arc::new(Owner(Mutex::new(None), Mutex::new(Vec::new()), Mutex::new(Vec::new())));
    driver.config.coordination = Some(owner.clone());
    driver.config.browser.backend = crate::BackendKind::SafariWebDriver;
    driver.config.initial_url = Some("https://example.test/slow".into());
    driver.remote_orientation = Some(crate::remote::RemoteOrientationState {
        support: OrientationSupport::Supported,
        applied: Some(RemoteOrientation::Landscape),
    });
    let config = driver.config.clone();
    let frame_slot = Arc::clone(&config.frame_slot);
    let (tx, rx) = mpsc::channel();
    let event_tx = BrowserEventSender { tx, ..events() };
    assert!(driver.prepare_ready(&config, &frame_slot, &event_tx, &AtomicBool::new(false)));
    let states = owner.2.lock().unwrap();
    assert!(!states.is_empty());
    assert!(states.iter().all(|state| state.unwrap().applied.is_none()));
    assert!(driver.classic_refresh.is_some());
    assert_eq!(driver.remote_orientation.unwrap().applied, None);
    assert_eq!(
        driver.remote_orientation.unwrap().support,
        OrientationSupport::Supported
    );
    assert_eq!(
        classic.recorded().len(),
        4,
        "never measure the previous document while startup is pending"
    );
    let observed: Vec<_> = rx.try_iter().collect();
    assert!(observed.iter().any(|event| matches!(event, BrowserEvent::Ready)));
    let invalidation = observed
        .iter()
        .position(|event| matches!(event, BrowserEvent::OrientationChanged(view) if view.state.applied.is_none()))
        .unwrap();
    let navigation = observed
        .iter()
        .position(|event| matches!(event, BrowserEvent::Loading(true)))
        .unwrap();
    assert!(
        invalidation < navigation,
        "publish invalidation before the blocking navigation call"
    );
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

fn explicit_driver(classic: &Server, link: crate::websocket::JsonWsLink) -> Driver {
    let mut driver = fixture_driver(classic, link);
    driver.config.browser.backend = crate::BackendKind::SafariWebDriver;
    let mut request = crate::webdriver::remote::tests::request(&classic.endpoint(""));
    request.capabilities["appium:orientation"] = json!("LANDSCAPE");
    driver.config.remote = Some(request);
    driver.remote_orientation = Some(crate::remote::RemoteOrientationState {
        support: OrientationSupport::Supported,
        applied: Some(RemoteOrientation::Landscape),
    });
    driver
}

fn measured(width: u32, height: u32, orientation: &str) -> Reply {
    Reply::json(
        200,
        &json!({"value":{"width":width,"height":height,"visual_width":width,"visual_height":height,"orientation":orientation}}),
    )
}

#[test]
fn explicit_start_rejects_opposite_unavailable_and_unsupported_committed_geometry_and_releases() {
    for (measurement, expected) in [
        (
            vec![
                Reply::json(200, &json!({"value":"PORTRAIT"})),
                measured(600, 900, "portrait"),
            ],
            "remote_orientation_mismatch",
        ),
        (
            vec![
                Reply::json(200, &json!({"value":"LANDSCAPE"})),
                Reply::json(
                    500,
                    &json!({"value":{"error":"javascript error","message":"page unavailable"}}),
                ),
            ],
            "orientation_unverified",
        ),
        (
            vec![Reply::json(
                500,
                &json!({"value":{"error":"unknown command","message":"unsupported"}}),
            )],
            "orientation_unsupported",
        ),
    ] {
        let mut replies = vec![Reply::json(200, &json!({"value":"document"}))];
        replies.extend(measurement);
        replies.push(Reply::json(200, &json!({"value":null})));
        let classic = Server::start(replies);
        let (link, worker) = bidi_fixture(false, false);
        let mut driver = explicit_driver(&classic, link);
        let config = driver.config.clone();
        let (tx, rx) = mpsc::channel();
        let events = BrowserEventSender { tx, ..events() };
        assert!(!driver.prepare_ready(&config, &config.frame_slot, &events, &AtomicBool::new(false)));
        let observed: Vec<_> = rx.try_iter().collect();
        assert!(!observed.iter().any(|e| matches!(e, BrowserEvent::Ready)));
        assert!(observed.iter().any(|e| matches!(e,BrowserEvent::RemoteSession(crate::RemoteSessionEvent::OrientationRejected {code,released:crate::RemoteReleaseOutcome::Released,..}) if *code==expected)));
        assert_eq!(
            *driver.remote_release.lock().unwrap(),
            Some(crate::RemoteReleaseOutcome::Released)
        );
        let deletes: Vec<_> = classic
            .recorded()
            .into_iter()
            .filter(|r| r.method == "DELETE")
            .collect();
        assert_eq!(deletes.len(), 1);
        assert_eq!(deletes[0].path, "/session/test");
        drop(driver);
        assert!(worker.join().unwrap().is_empty());
    }
}

#[test]
fn explicit_start_waiting_for_navigation_is_refused_and_preserves_uncertain_release() {
    let mut replies = vec![
        Reply::json(200, &json!({"value":"old-document"})),
        Reply::json(200, &json!({"value":null})),
        Reply::json(500, &json!({"value":{"error":"timeout","message":"still loading"}})),
        Reply::json(200, &json!({"value":null})),
    ];
    replies.extend((0..3).map(|_| Reply::json(200, &json!({"value":{}}))));
    let classic = Server::start(replies);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = explicit_driver(&classic, link);
    driver.config.initial_url = Some("https://example.test/slow".into());
    let config = driver.config.clone();
    let (tx, rx) = mpsc::channel();
    let events = BrowserEventSender { tx, ..events() };
    assert!(!driver.prepare_ready(&config, &config.frame_slot, &events, &AtomicBool::new(false)));
    let observed: Vec<_> = rx.try_iter().collect();
    assert!(!observed.iter().any(|e| matches!(e, BrowserEvent::Ready)));
    assert!(observed.iter().any(|e| matches!(
        e,
        BrowserEvent::RemoteSession(crate::RemoteSessionEvent::OrientationRejected {
            code: "orientation_unverified",
            released: crate::RemoteReleaseOutcome::ReleaseUnknown { attempts: 3, .. },
            ..
        })
    )));
    assert!(matches!(
        *driver.remote_release.lock().unwrap(),
        Some(crate::RemoteReleaseOutcome::ReleaseUnknown { attempts: 3, .. })
    ));
    let commands = classic.recorded();
    assert_eq!(
        commands.len(),
        7,
        "pending page must not be measured as the previous document"
    );
    assert!(
        commands[4..]
            .iter()
            .all(|r| r.method == "DELETE" && r.path == "/session/test")
    );
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn explicit_start_success_publishes_matching_geometry_before_ready_and_does_not_release() {
    let classic = Server::start(vec![
        Reply::json(200, &json!({"value":"old-document"})),
        Reply::json(200, &json!({"value":null})),
        Reply::json(200, &json!({"value":null})),
        Reply::json(200, &json!({"value":"https://example.test/ready"})),
        Reply::json(200, &json!({"value":null})),
        Reply::json(200, &json!({"value":"https://example.test/ready"})),
        Reply::json(200, &json!({"value":"Orientation demo"})),
        Reply::json(200, &json!({"value":"new-document"})),
        Reply::json(200, &json!({"value":"LANDSCAPE"})),
        measured(900, 600, "landscape"),
    ]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = explicit_driver(&classic, link);
    driver.config.initial_url = Some("https://example.test/ready".into());
    let config = driver.config.clone();
    let (tx, rx) = mpsc::channel();
    let events = BrowserEventSender { tx, ..events() };
    assert!(driver.prepare_ready(&config, &config.frame_slot, &events, &AtomicBool::new(false)));
    let observed: Vec<_> = rx.try_iter().collect();
    let verified = observed
        .iter()
        .position(
            |e| matches!(e,BrowserEvent::OrientationChanged(v) if v.state.applied==Some(RemoteOrientation::Landscape)),
        )
        .unwrap();
    let ready = observed.iter().position(|e| matches!(e, BrowserEvent::Ready)).unwrap();
    assert!(verified < ready);
    assert_eq!(
        driver.remote_orientation.unwrap().applied,
        Some(RemoteOrientation::Landscape)
    );
    assert!(classic.recorded().iter().all(|r| r.method != "DELETE"));
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn explicit_failed_navigation_releases_without_probing_the_old_document() {
    let classic = Server::start(vec![
        Reply::json(200, &json!({"value":"old-document"})),
        Reply::json(200, &json!({"value":null})),
        Reply::json(
            500,
            &json!({"value":{"error":"unknown error","message":"navigation failed"}}),
        ),
        Reply::json(200, &json!({"value":null})),
        Reply::json(200, &json!({"value":null})),
    ]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = explicit_driver(&classic, link);
    driver.config.initial_url = Some("https://example.test/failure".into());
    let config = driver.config.clone();
    let (tx, rx) = mpsc::channel();
    let events = BrowserEventSender { tx, ..events() };
    assert!(!driver.prepare_ready(&config, &config.frame_slot, &events, &AtomicBool::new(false)));
    let observed: Vec<_> = rx.try_iter().collect();
    assert!(!observed.iter().any(|e| matches!(e, BrowserEvent::Ready)));
    assert!(observed.iter().any(|e| matches!(
        e,
        BrowserEvent::RemoteSession(crate::RemoteSessionEvent::OrientationRejected {
            code: "orientation_unverified",
            ..
        })
    )));
    assert_eq!(classic.recorded().len(), 5);
    assert_eq!(classic.recorded()[4].method, "DELETE");
    assert!(classic.recorded().iter().all(|r| !r.path.ends_with("/orientation")));
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn cancellation_during_committed_measurement_releases_and_never_publishes_ready() {
    let (release_tx, release_rx) = mpsc::channel();
    let classic = Server::start(vec![
        Reply::json(200, &json!({"value":"document"})),
        Reply::json(200, &json!({"value":"LANDSCAPE"})),
        measured(900, 600, "landscape").blocked_until(release_rx),
        Reply::json(200, &json!({"value":null})),
    ]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = explicit_driver(&classic, link);
    let stopped = Arc::new(AtomicBool::new(false));
    let driver_stop = Arc::clone(&stopped);
    let (tx, rx) = mpsc::channel();
    let events = BrowserEventSender { tx, ..events() };
    let preparation = std::thread::spawn(move || {
        let config = driver.config.clone();
        let accepted = driver.prepare_ready(&config, &config.frame_slot, &events, &driver_stop);
        (driver, accepted)
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while classic.recorded().len() < 3 {
        assert!(Instant::now() < deadline, "measurement must reach the mock transport");
        std::thread::sleep(Duration::from_millis(1));
    }
    stopped.store(true, Ordering::Release);
    release_tx.send(()).unwrap();
    let (driver, accepted) = preparation.join().unwrap();
    assert!(!accepted);
    let observed: Vec<_> = rx.try_iter().collect();
    assert!(!observed.iter().any(|e| matches!(e, BrowserEvent::Ready)));
    assert!(observed.iter().any(|e| matches!(
        e,
        BrowserEvent::RemoteSession(crate::RemoteSessionEvent::OrientationRejected {
            code: "browser_unavailable",
            released: crate::RemoteReleaseOutcome::Released,
            ..
        })
    )));
    assert_eq!(classic.recorded().iter().filter(|r| r.method == "DELETE").count(), 1);
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn default_start_without_observable_geometry_remains_ready_and_unverified() {
    let classic = Server::start(vec![
        Reply::json(200, &json!({"value":"document"})),
        Reply::json(200, &json!({"value":"LANDSCAPE"})),
        Reply::json(
            500,
            &json!({"value":{"error":"javascript error","message":"not measurable"}}),
        ),
    ]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = explicit_driver(&classic, link);
    driver
        .config
        .remote
        .as_mut()
        .unwrap()
        .capabilities
        .as_object_mut()
        .unwrap()
        .remove("appium:orientation");
    let config = driver.config.clone();
    let (tx, rx) = mpsc::channel();
    let events = BrowserEventSender { tx, ..events() };
    assert!(driver.prepare_ready(&config, &config.frame_slot, &events, &AtomicBool::new(false)));
    let observed: Vec<_> = rx.try_iter().collect();
    assert!(observed.iter().any(|e| matches!(e, BrowserEvent::Ready)));
    assert_eq!(driver.remote_orientation.unwrap().applied, None);
    assert_eq!(
        driver.remote_orientation.unwrap().support,
        OrientationSupport::Supported
    );
    assert!(classic.recorded().iter().all(|r| r.method != "DELETE"));
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn explicit_start_cancelled_before_preparation_releases_without_probing() {
    let classic = Server::start(vec![Reply::json(200, &json!({"value":null}))]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = explicit_driver(&classic, link);
    let config = driver.config.clone();
    let (tx, rx) = mpsc::channel();
    let events = BrowserEventSender { tx, ..events() };
    assert!(!driver.prepare_ready(&config, &config.frame_slot, &events, &AtomicBool::new(true)));
    let observed: Vec<_> = rx.try_iter().collect();
    assert!(!observed.iter().any(|e| matches!(e, BrowserEvent::Ready)));
    assert!(observed.iter().any(|e| matches!(
        e,
        BrowserEvent::RemoteSession(crate::RemoteSessionEvent::OrientationRejected {
            code: "browser_unavailable",
            released: crate::RemoteReleaseOutcome::Released,
            ..
        })
    )));
    assert_eq!(classic.recorded().len(), 1);
    assert_eq!(classic.recorded()[0].method, "DELETE");
    assert_eq!(classic.recorded()[0].path, "/session/test");
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}
