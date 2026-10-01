use super::*;
use crate::remote::RemoteOrientationState;

fn remeasurement() -> Vec<Reply> {
    vec![
        Reply::json(200, &json!({"value":"LANDSCAPE"})),
        Reply::json(
            200,
            &json!({"value":{"width":900,"height":600,"visual_width":900,"visual_height":600,"orientation":"landscape"}}),
        ),
    ]
}

#[test]
fn lost_post_reply_remeasures_applied_orientation_without_repeating_mutation() {
    for user in [false, true] {
        let mut replies = vec![baseline(), Reply::json(500, &json!({"value":"response lost"}))];
        replies.extend(remeasurement());
        let classic = Server::start(replies);
        let (link, worker) = bidi_fixture(false, false);
        let mut driver = fixture_driver(&classic, link);
        driver.remote_orientation = Some(RemoteOrientationState {
            support: OrientationSupport::Supported,
            applied: Some(RemoteOrientation::Portrait),
        });
        driver.orientation_document = DocumentOrientation::Clean;
        if user {
            driver.begin_user_orientation(
                "user-rotation".into(),
                RemoteOrientation::Landscape,
                &events(),
                &AtomicBool::new(false),
            );
        } else {
            driver.begin_orientation(&rotation_request(), &events(), &AtomicBool::new(false));
        }
        assert!(driver.pending_orientation.is_none());
        assert_eq!(driver.remote_orientation.unwrap().applied, None);
        assert_eq!(driver.orientation_document, DocumentOrientation::NeedsMeasurement);
        let error = driver.orientation_error.clone().unwrap();
        assert!(error.starts_with("orientation_unverified:"));
        assert_eq!(classic.recorded().len(), 2);

        let (tx, rx) = std::sync::mpsc::channel();
        driver.refresh_document_orientation(&BrowserEventSender { tx, ..events() });
        assert_eq!(
            driver.remote_orientation.unwrap().applied,
            Some(RemoteOrientation::Landscape)
        );
        assert_eq!(driver.orientation_document, DocumentOrientation::Clean);
        assert_eq!(driver.orientation_error.as_deref(), Some(error.as_str()));
        let updates: Vec<_> = rx.try_iter().collect();
        assert!(updates.iter().any(|event| matches!(
            event,
            crate::session::BrowserEvent::OrientationChanged(view)
                if view.state.applied == Some(RemoteOrientation::Landscape) && view.error.as_deref() == Some(error.as_str())
        )));
        let calls = classic.recorded();
        assert_eq!(calls.len(), 4);
        assert_eq!(
            (calls[1].method.as_str(), calls[1].path.as_str()),
            ("POST", "/session/test/orientation")
        );
        assert_eq!(
            (calls[2].method.as_str(), calls[2].path.as_str()),
            ("GET", "/session/test/orientation")
        );
        assert_eq!(calls[3].path, "/session/test/execute/sync");
        drop(driver);
        assert!(worker.join().unwrap().is_empty());
    }
}

#[test]
fn acknowledgement_timeout_defers_read_only_recovery_until_request_settles() {
    let mut replies = vec![baseline(), Reply::json(200, &json!({"value":null}))];
    replies.extend(remeasurement());
    let classic = Server::start(replies);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    driver.remote_orientation = Some(RemoteOrientationState {
        support: OrientationSupport::Supported,
        applied: Some(RemoteOrientation::Portrait),
    });
    driver.orientation_document = DocumentOrientation::Clean;
    driver.begin_orientation(&rotation_request(), &events(), &AtomicBool::new(false));
    assert!(driver.pending_orientation.is_some());
    driver.refresh_document_orientation(&events());
    assert_eq!(classic.recorded().len(), 2, "pending rotation owns measurement");
    assert_eq!(driver.orientation_document, DocumentOrientation::NeedsMeasurement);
    driver.pending_orientation.as_mut().unwrap().deadline = Instant::now();
    driver.tick_orientation(&events(), &AtomicBool::new(false));
    assert!(driver.pending_orientation.is_none());
    assert!(
        driver
            .orientation_error
            .as_ref()
            .unwrap()
            .starts_with("orientation_timeout:")
    );
    driver.refresh_document_orientation(&events());
    assert_eq!(
        driver.remote_orientation.unwrap().applied,
        Some(RemoteOrientation::Landscape)
    );
    assert_eq!(driver.orientation_document, DocumentOrientation::Clean);
    assert!(
        driver
            .orientation_error
            .as_ref()
            .unwrap()
            .starts_with("orientation_timeout:")
    );
    assert_eq!(
        classic.recorded().len(),
        4,
        "recovery must not retry the orientation POST"
    );
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}
