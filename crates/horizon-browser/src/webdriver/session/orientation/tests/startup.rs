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
    driver.prepare_ready(&config, &frame_slot, &event_tx, &AtomicBool::new(false));
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
