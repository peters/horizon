use super::*;
use crate::BrowserControlValue;
use crate::webdriver::session::viewport::tests::{bidi_fixture, fixture_driver};
use crate::webdriver::test_server::{Reply, Server};

fn sample(url: &str, root: &str) -> Vec<Reply> {
    vec![
        Reply::json(200, &json!({"value":url})),
        Reply::json(200, &json!({"value":{"element-6066-11e4-a52e-4f735466cecf":root}})),
    ]
}

#[test]
fn copied_page_marker_cannot_preserve_refs_across_same_url_root_replacement() {
    let scan = json!({"nodes":[{"selector":"#button","role":"button","name":"Test","visible":true,"enabled":true}],"documentIdentity":"copied-page-token"});
    let classic = Server::start(vec![
        Reply::native_document("first-root"),
        Reply::json(200, &json!({"value":scan})),
        Reply::native_document("first-root"),
        Reply::native_document("second-root"),
        Reply::json(200, &json!({"value":scan})),
        Reply::native_document("second-root"),
    ]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    let first = driver.semantic_query("#button", 1).unwrap();
    let BrowserControlValue::Nodes { generation, nodes, .. } = first else {
        panic!("nodes")
    };
    let old_ref = nodes[0].reference.clone();
    let second = driver.semantic_query("#button", 1).unwrap();
    let BrowserControlValue::Nodes { generation: next, .. } = second else {
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
    assert_eq!(calls.len(), 18);
    assert!(
        calls
            .iter()
            .filter(|call| call.path.ends_with("/element"))
            .all(|call| call.body.contains(":root"))
    );
    assert!(
        calls
            .iter()
            .filter(|call| call.path.ends_with("/execute/sync"))
            .all(|call| !call.body.contains("documentIdentity") && !call.body.contains("Symbol.for"))
    );
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}

#[test]
fn replacement_during_scan_is_never_registered_under_the_new_anchor() {
    let classic = Server::start(vec![
        Reply::native_document("first-root"),
        Reply::json(
            200,
            &json!({"value":{"nodes":[],"documentIdentity":"copied-page-token"}}),
        ),
        Reply::native_document("second-root"),
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
fn mixed_url_root_samples_invalidate_even_before_any_identity_was_cached() {
    for root_changed in [false, true] {
        let mut replies = sample("https://example.test/", "first-root");
        replies.extend(sample(
            if root_changed {
                "https://example.test/"
            } else {
                "https://example.test/next"
            },
            if root_changed { "second-root" } else { "first-root" },
        ));
        let classic = Server::start(replies);
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
}

#[test]
fn malformed_native_anchor_and_provider_errors_are_sanitized() {
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
    let classic = Server::start(vec![Reply::json(
        500,
        &json!({"value":{"error":"unknown error","message":"private provider detail"}}),
    )]);
    let (link, worker) = bidi_fixture(false, false);
    let mut driver = fixture_driver(&classic, link);
    let failure = driver
        .refresh_classic_document_identity_within(Duration::from_secs(1))
        .unwrap_err();
    assert_eq!(failure.code, "invalid_result");
    assert!(!failure.message.contains("private provider detail"));
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
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
    let start = Instant::now();
    assert_eq!(
        driver
            .refresh_classic_document_identity_within(Duration::from_millis(120))
            .unwrap_err()
            .code,
        "document_observation_timeout"
    );
    assert!(start.elapsed() < Duration::from_millis(500));
    assert!(driver.classic_document_identity.is_none());
    assert_eq!(classic.recorded().len(), 2);
    drop(driver);
    assert!(worker.join().unwrap().is_empty());
}
