use super::*;
use crate::BrowserControlValue;
use crate::webdriver::session::viewport::tests::{bidi_fixture, fixture_driver};
use crate::webdriver::test_server::{Reply, Server};

fn cached(root: &str) -> String {
    serde_json::to_string(&["https://example.test/", root]).unwrap()
}
fn scan() -> Value {
    json!({"nodes":[{"selector":"#button","role":"button","name":"Test","visible":true,"enabled":true}],"documentIdentity":"copied-page-token"})
}

#[test]
fn retained_reference_is_validated_without_comparing_new_provider_aliases() {
    let classic = Server::start(vec![
        Reply::native_document("root-one"),
        Reply::same_document(),
        Reply::same_document(),
    ]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    let generation = driver.semantic.generation();
    for _ in 0..3 {
        assert!(
            !driver
                .refresh_classic_document_identity_within(Duration::from_secs(1))
                .unwrap()
        );
    }
    assert_eq!(driver.semantic.generation(), generation);
    let calls = classic.recorded();
    assert_eq!(calls.len(), 10);
    assert_eq!(calls.iter().filter(|c| c.path.ends_with("/element")).count(), 1);
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.path == "/session/test/element/root-one/name")
            .count(),
        3
    );
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn copied_page_marker_cannot_preserve_refs_across_same_url_root_replacement() {
    let classic = Server::start(vec![
        Reply::native_document("first-root"),
        Reply::json(200, &json!({"value":scan()})),
        Reply::same_document(),
        Reply::replaced_document("second-root"),
        Reply::json(200, &json!({"value":scan()})),
        Reply::same_document(),
    ]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    let BrowserControlValue::Nodes { generation, nodes, .. } = driver.semantic_query("#button", 1).unwrap() else {
        panic!("nodes")
    };
    let old_ref = nodes[0].reference.clone();
    let BrowserControlValue::Nodes { generation: next, .. } = driver.semantic_query("#button", 1).unwrap() else {
        panic!("nodes")
    };
    assert!(next > generation);
    assert_eq!(
        driver
            .semantic
            .resolve(&crate::BrowserTarget::Ref { reference: old_ref })
            .unwrap_err()
            .code,
        "stale_reference"
    );
    let calls = classic.recorded();
    assert_eq!(calls.len(), 17);
    assert_eq!(calls.iter().filter(|c| c.path.ends_with("/element")).count(), 2);
    assert!(
        calls
            .iter()
            .filter(|c| c.path.ends_with("/execute/sync"))
            .all(|c| !c.body.contains("documentIdentity") && !c.body.contains("Symbol.for"))
    );
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn native_staleness_invalidates_even_when_the_provider_reuses_the_same_id() {
    for error in ["stale element reference", "no such element"] {
        let mut reply = Reply::replaced_document("same-id");
        reply.following[0] = Reply::json(404, &json!({"value":{"error":error,"message":"old document"}}));
        let classic = Server::start(vec![reply]);
        let (link, worker) = bidi_fixture(false, false);
        let mut driver = fixture_driver(&classic, link);
        driver.classic_document_identity = Some(cached("same-id"));
        let generation = driver.semantic.generation();
        assert!(
            driver
                .refresh_classic_document_identity_within(Duration::from_secs(1))
                .unwrap()
        );
        assert_eq!(driver.classic_document_identity, Some(cached("same-id")));
        assert!(driver.semantic.generation() > generation);
        assert_eq!(classic.recorded().len(), 5);
        drop(driver);
        assert!(worker.join().unwrap().is_empty());
    }
}

#[test]
fn replacement_during_scan_is_never_registered_under_the_new_anchor() {
    let classic = Server::start(vec![
        Reply::native_document("first-root"),
        Reply::json(200, &json!({"value":scan()})),
        Reply::replaced_document("second-root"),
    ]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    assert_eq!(
        driver.semantic_query("#button", 1).unwrap_err().code,
        "document_navigation_invalidated"
    );
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn a_changed_url_during_native_observation_invalidates_before_adoption() {
    let mut reply = Reply::native_document("first-root");
    *reply.following.last_mut().unwrap() = Reply::json(200, &json!({"value":"https://example.test/next"}));
    let classic = Server::start(vec![reply]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    let generation = driver.semantic.generation();
    assert_eq!(
        driver
            .refresh_classic_document_identity_within(Duration::from_secs(1))
            .unwrap_err()
            .code,
        "document_navigation_invalidated"
    );
    assert!(driver.semantic.generation() > generation);
    assert!(driver.classic_document_identity.is_none());
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn a_replacement_that_is_already_stale_is_never_adopted() {
    let mut reply = Reply::native_document("first-root");
    reply.following[1] = Reply::json(
        404,
        &json!({"value":{"error":"stale element reference","message":"private"}}),
    );
    let classic = Server::start(vec![reply]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    let generation = driver.semantic.generation();
    assert_eq!(
        driver
            .refresh_classic_document_identity_within(Duration::from_secs(1))
            .unwrap_err()
            .code,
        "document_navigation_invalidated"
    );
    assert!(driver.semantic.generation() > generation);
    assert!(driver.classic_document_identity.is_none());
    assert_eq!(classic.recorded().len(), 3);
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn malformed_native_references_and_names_are_refused() {
    for value in [
        json!(null),
        json!({}),
        json!({"ELEMENT":""}),
        json!({"element-6066-11e4-a52e-4f735466cecf":7}),
    ] {
        let classic = Server::start(vec![
            Reply::json(200, &json!({"value":"https://example.test/"})),
            Reply::json(200, &json!({"value":value})),
        ]);
        let (link, worker) = bidi_fixture(false, false);
        let mut driver = fixture_driver(&classic, link);
        assert_eq!(
            driver
                .refresh_classic_document_identity_within(Duration::from_secs(1))
                .unwrap_err()
                .code,
            "invalid_result"
        );
        assert!(driver.classic_document_identity.is_none());
        drop(driver);
        assert!(worker.join().unwrap().is_empty());
    }
    for name in [json!(null), json!(7), json!(""), json!({})] {
        let classic = Server::start(vec![
            Reply::json(200, &json!({"value":"https://example.test/"})),
            Reply::json(200, &json!({"value":name})),
        ]);
        let (link, worker) = bidi_fixture(false, false);
        let mut driver = fixture_driver(&classic, link);
        driver.classic_document_identity = Some(cached("retained"));
        assert_eq!(
            driver
                .refresh_classic_document_identity_within(Duration::from_secs(1))
                .unwrap_err()
                .code,
            "invalid_result"
        );
        assert!(classic.recorded().iter().all(|c| c.method == "GET"));
        drop(driver);
        assert!(worker.join().unwrap().is_empty());
    }
}

#[test]
fn unsupported_and_arbitrary_errors_do_not_renew_the_retained_reference() {
    for error in ["unknown command", "unknown error", "invalid session id"] {
        let classic = Server::start(vec![
            Reply::json(200, &json!({"value":"https://example.test/"})),
            Reply::json(
                500,
                &json!({"value":{"error":error,"message":"stale element reference private provider detail"}}),
            ),
        ]);
        let (link, worker) = bidi_fixture(false, false);
        let mut driver = fixture_driver(&classic, link);
        driver.classic_document_identity = Some(cached("retained"));
        let generation = driver.semantic.generation();
        let failure = driver
            .refresh_classic_document_identity_within(Duration::from_secs(1))
            .unwrap_err();
        assert_eq!(failure.code, "invalid_result");
        assert!(!failure.message.contains("private provider detail"));
        assert_eq!(driver.semantic.generation(), generation);
        assert!(classic.recorded().iter().all(|c| c.method == "GET"));
        drop(driver);
        assert!(worker.join().unwrap().is_empty());
    }
}

#[test]
fn native_components_share_one_deadline_including_a_late_error() {
    let classic = Server::start(vec![
        Reply::json(200, &json!({"value":"https://example.test/"})).delayed(Duration::from_millis(70)),
        Reply::json(500, &json!({"value":{"error":"unknown error","message":"late"}}))
            .delayed(Duration::from_millis(100)),
    ]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    driver.classic_document_identity = Some(cached("retained"));
    let start = Instant::now();
    assert_eq!(
        driver
            .refresh_classic_document_identity_within(Duration::from_millis(120))
            .unwrap_err()
            .code,
        "document_observation_timeout"
    );
    assert!(start.elapsed() < Duration::from_millis(500));
    assert_eq!(classic.recorded().len(), 2);
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn a_known_url_change_invalidates_even_when_replacement_verification_fails() {
    let classic = Server::start(vec![
        Reply::json(200, &json!({"value":"https://example.test/next"})),
        Reply::json(200, &json!({"value":null})),
    ]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    driver.classic_document_identity = Some(cached("retained"));
    driver.remote_orientation = Some(crate::remote::RemoteOrientationState {
        support: crate::remote::OrientationSupport::Supported,
        applied: Some(crate::remote::RemoteOrientation::Landscape),
    });
    let generation = driver.semantic.generation();
    assert_eq!(
        driver
            .refresh_classic_document_identity_within(Duration::from_secs(1))
            .unwrap_err()
            .code,
        "invalid_result"
    );
    assert!(driver.semantic.generation() > generation);
    assert!(driver.remote_orientation.unwrap().applied.is_none());
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}
