use super::*;
use std::sync::{Arc, mpsc};

fn begin(driver: &mut Driver, user: bool, stopped: &AtomicBool) {
    if user {
        driver.begin_user_orientation("user-rotation".into(), RemoteOrientation::Landscape, &events(), stopped);
    } else {
        driver.service_browser_request(&rotation_request(), &events(), stopped);
    }
}

#[test]
fn cached_identity_change_before_rotation_succeeds_but_postdispatch_change_is_refused() {
    for user in [false, true] {
        for changed_after in [false, true] {
            let mut replies = vec![baseline(), Reply::json(200, &json!({"value":null}))];
            replies.extend(observation(4, 2));
            replies.push(Reply::json(
                200,
                &json!({"value":if changed_after {"new-document"} else {"document"}}),
            ));
            replies.push(baseline());
            let classic = Server::start(replies);
            let (link, worker) = bidi_fixture(false, false);
            let mut driver = fixture_driver(&classic, link);
            driver.classic_document_identity = Some("cached-allocation-document".into());
            driver.remote_orientation = Some(crate::remote::RemoteOrientationState::default());
            let generation = driver.semantic.generation();
            begin(&mut driver, user, &AtomicBool::new(false));
            assert!(driver.pending_orientation.is_some());
            assert!(driver.semantic.generation() > generation);
            let calls = classic.recorded();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0].path, "/session/test/execute/sync");
            assert_eq!(calls[1].path, "/session/test/orientation");
            driver.tick_orientation(&events(), &AtomicBool::new(false));
            if !changed_after {
                assert!(driver.pending_orientation.as_ref().unwrap().verified.is_some());
                driver.signal_epoch += 1;
                driver.tick_orientation(&events(), &AtomicBool::new(false));
            }
            assert!(driver.pending_orientation.is_none());
            if changed_after {
                assert!(
                    driver
                        .orientation_error
                        .as_deref()
                        .unwrap()
                        .starts_with("orientation_navigation_invalidated:")
                );
            } else {
                assert!(driver.orientation_error.is_none());
                assert_eq!(
                    driver.remote_orientation.unwrap().applied,
                    Some(RemoteOrientation::Landscape)
                );
            }
            if user {
                assert_eq!(driver.orientation_completed.completed.len(), 1);
                assert_eq!(driver.orientation_completed.completed[0].error.is_some(), changed_after);
            }
            drop(driver);
            assert!(worker.join().unwrap().is_empty());
        }
    }
}

#[test]
fn stop_and_teach_during_baseline_never_dispatch_mutation_and_complete_users_once() {
    for user in [false, true] {
        for teach in [false, true] {
            let (release, blocked) = mpsc::channel();
            let classic = Server::start(vec![baseline().blocked_until(blocked)]);
            let (link, worker) = bidi_fixture(false, false);
            let mut driver = fixture_driver(&classic, link);
            driver.remote_orientation = Some(crate::remote::RemoteOrientationState::default());
            let stopped = Arc::new(AtomicBool::new(false));
            let rotation_stopped = stopped.clone();
            let panel_slot = driver.panel_slot.clone();
            let rotation = std::thread::spawn(move || {
                begin(&mut driver, user, &rotation_stopped);
                driver
            });
            let deadline = Instant::now() + Duration::from_secs(3);
            while classic.recorded().is_empty() {
                assert!(Instant::now() < deadline, "baseline request did not arrive");
                std::thread::sleep(Duration::from_millis(1));
            }
            if teach {
                panel_slot.set_teach_recording(true);
            } else {
                stopped.store(true, Ordering::Release);
            }
            release.send(()).unwrap();
            let driver = rotation.join().unwrap();
            assert!(driver.pending_orientation.is_none());
            let code = if teach {
                "orientation_user_active:"
            } else {
                "browser_unavailable:"
            };
            assert!(driver.orientation_error.as_deref().unwrap().starts_with(code));
            assert_eq!(classic.recorded().len(), 1);
            assert_eq!(classic.recorded()[0].path, "/session/test/execute/sync");
            if user {
                assert_eq!(driver.orientation_completed.completed.len(), 1);
                assert!(
                    driver.orientation_completed.completed[0]
                        .error
                        .as_deref()
                        .unwrap()
                        .starts_with(code)
                );
            }
            drop(driver);
            assert!(worker.join().unwrap().is_empty());
        }
    }
}

#[test]
fn baseline_timeout_keeps_the_request_deadline_and_does_not_post() {
    let classic = Server::start(vec![baseline().delayed(Duration::from_millis(300))]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    driver.remote_orientation = Some(crate::remote::RemoteOrientationState::default());
    let mut request = rotation_request();
    request.action = BrowserControlAction::Orientation {
        orientation: RemoteOrientation::Landscape,
        timeout_millis: 100,
    };
    let start = Instant::now();
    driver.service_browser_request(&request, &events(), &AtomicBool::new(false));
    assert!(start.elapsed() < Duration::from_millis(750));
    assert!(driver.pending_orientation.is_none());
    assert!(
        driver
            .orientation_error
            .as_deref()
            .unwrap()
            .starts_with("orientation_timeout:")
    );
    assert_eq!(classic.recorded().len(), 1);
    assert_eq!(classic.recorded()[0].path, "/session/test/execute/sync");
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn unavailable_baseline_refuses_before_post_and_completes_users_once() {
    for user in [false, true] {
        let classic = Server::start(vec![Reply::json(200, &json!({"value":null}))]);
        let (link, worker) = bidi_fixture(false, false);
        let mut driver = fixture_driver(&classic, link);
        driver.remote_orientation = Some(crate::remote::RemoteOrientationState::default());
        begin(&mut driver, user, &AtomicBool::new(false));
        assert!(driver.pending_orientation.is_none());
        assert!(
            driver
                .orientation_error
                .as_deref()
                .unwrap()
                .starts_with("invalid_result:")
        );
        assert_eq!(classic.recorded().len(), 1);
        assert_eq!(classic.recorded()[0].path, "/session/test/execute/sync");
        if user {
            assert_eq!(driver.orientation_completed.completed.len(), 1);
            assert!(driver.orientation_completed.completed[0].error.is_some());
        }
        drop(driver);
        assert!(worker.join().unwrap().is_empty());
    }
}
