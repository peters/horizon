use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use super::{RemoteExpiry, RemoteHost, RemoteReleaseOutcome, RemoteSessionRequest, RemoteStartFailure};
use crate::webdriver::remote_http::RemoteAuthorizationHeader;
use crate::webdriver::test_server::{Reply, Server};

fn request(endpoint: &str) -> RemoteSessionRequest {
    RemoteSessionRequest {
        endpoint: endpoint.to_string(),
        authorization: Some(Arc::new(
            RemoteAuthorizationHeader::new("Basic c2VjcmV0".into()).expect("header"),
        )),
        capabilities: json!({"browserName": "safari", "vendor:options": {"realMobile": "true"}}),
        allocation_timeout: Duration::from_millis(400),
        max_session: Duration::from_mins(30),
        idle_release: Duration::from_mins(3),
        label: "ios_phone".into(),
    }
}

#[test]
fn allocation_sends_one_authenticated_new_session_and_parses_the_id() {
    let server = Server::start(vec![Reply::json(
        200,
        &json!({"value": {"sessionId": "abc-123", "capabilities": {"browserName": "safari"}}}),
    )]);
    let request = request(&server.endpoint("/wd/hub"));
    let host = RemoteHost::connect(&request, Instant::now()).expect("connect");
    let session = host.allocate(&request).expect("allocated");
    assert_eq!(session.id, "abc-123");
    assert_eq!(session.capabilities["browserName"], "safari");
    let seen = server.recorded();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        (seen[0].method.as_str(), seen[0].path.as_str()),
        ("POST", "/wd/hub/session")
    );
    assert_eq!(seen[0].authorization.as_deref(), Some("Basic c2VjcmV0"));
    assert!(seen[0].body.contains("alwaysMatch"));
    assert!(seen[0].body.contains("realMobile"));
    assert!(!format!("{host:?}").contains("c2VjcmV0"));
}

#[test]
fn a_webdriver_error_is_a_failed_allocation() {
    let server = Server::start(vec![Reply::json(
        500,
        &json!({"value": {"error": "session not created", "message": "no device available"}}),
    )]);
    let request = request(&server.endpoint(""));
    let host = RemoteHost::connect(&request, Instant::now()).expect("connect");
    assert_eq!(
        host.allocate(&request).expect_err("failed"),
        RemoteStartFailure::AllocationFailed {
            error: "session not created".into(),
            message: "no device available".into(),
        }
    );
}

#[test]
fn an_ambiguous_new_session_is_unknown_and_never_retried() {
    let server = Server::start(vec![
        Reply::json(200, &json!({"value": {"sessionId": "late"}})).delayed(Duration::from_millis(900)),
        Reply::json(200, &json!({"value": {"sessionId": "second"}})),
    ]);
    let request = request(&server.endpoint(""));
    let host = RemoteHost::connect(&request, Instant::now()).expect("connect");
    let failure = host.allocate(&request).expect_err("timed out");
    assert!(
        matches!(failure, RemoteStartFailure::AllocationUnknown { .. }),
        "{failure:?}"
    );
    assert!(failure.to_string().contains("not retried"));
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(server.recorded().len(), 1, "exactly one New Session was ever sent");
}

#[test]
fn a_success_without_a_safe_session_id_is_unknown() {
    let server = Server::start(vec![Reply::json(200, &json!({"value": {"capabilities": {}}}))]);
    let request = request(&server.endpoint(""));
    let host = RemoteHost::connect(&request, Instant::now()).expect("connect");
    assert!(matches!(
        host.allocate(&request).expect_err("no id"),
        RemoteStartFailure::AllocationUnknown { .. }
    ));
}

#[test]
fn endpoint_rules_are_enforced_before_any_request() {
    let mut bad = request("https://user:key@grid.example.net/wd/hub");
    assert!(matches!(
        RemoteHost::connect(&bad, Instant::now()).expect_err("userinfo"),
        RemoteStartFailure::InvalidEndpoint(_)
    ));
    bad.endpoint = "http://grid.example.net/wd/hub".into();
    assert!(
        RemoteHost::connect(&bad, Instant::now()).is_err(),
        "plain http to a remote host"
    );
}

#[test]
fn watchdog_expires_on_the_hard_deadline_or_idle_policy_and_stays_expired() {
    let mut request = request("https://grid.example.net/wd/hub");
    request.max_session = Duration::from_secs(100);
    request.idle_release = Duration::from_secs(30);
    let start = Instant::now();
    let mut host = RemoteHost::connect(&request, start).expect("connect");
    assert_eq!(host.check_expiry(start + Duration::from_secs(10)), None);
    host.note_activity(start + Duration::from_secs(25));
    assert_eq!(
        host.check_expiry(start + Duration::from_secs(50)),
        None,
        "activity resets idle"
    );
    assert_eq!(
        host.check_expiry(start + Duration::from_secs(56)),
        Some(RemoteExpiry::Idle)
    );
    host.note_activity(start + Duration::from_secs(57));
    assert_eq!(
        host.check_expiry(start + Duration::from_secs(58)),
        Some(RemoteExpiry::Idle),
        "sticky"
    );

    let mut host = RemoteHost::connect(&request, start).expect("connect");
    for second in (0..100).step_by(20) {
        host.note_activity(start + Duration::from_secs(second));
    }
    assert_eq!(
        host.check_expiry(start + Duration::from_secs(100)),
        Some(RemoteExpiry::HardDeadline),
        "activity never extends the hard lifetime"
    );
}

#[test]
fn release_is_verified_or_reported_unknown_after_bounded_attempts() {
    let released = Server::start(vec![Reply::json(200, &json!({"value": null}))]);
    let first = request(&released.endpoint(""));
    let host = RemoteHost::connect(&first, Instant::now()).expect("connect");
    assert_eq!(host.release("abc"), RemoteReleaseOutcome::Released);
    assert_eq!(released.recorded()[0].path, "/session/abc");

    let gone = Server::start(vec![Reply::json(
        404,
        &json!({"value": {"error": "invalid session id", "message": "gone"}}),
    )]);
    let host = RemoteHost::connect(&request(&gone.endpoint("")), Instant::now()).expect("connect");
    assert_eq!(host.release("abc"), RemoteReleaseOutcome::AlreadyGone);

    let refused = Server::start(vec![Reply::json(
        500,
        &json!({"value": {"error": "unknown error", "message": "busy"}}),
    )]);
    let host = RemoteHost::connect(&request(&refused.endpoint("")), Instant::now()).expect("connect");
    assert_eq!(
        host.release("abc"),
        RemoteReleaseOutcome::Failed {
            error: "unknown error".into(),
            message: "busy".into(),
        }
    );
    assert_eq!(refused.recorded().len(), 1, "a WebDriver answer is not retried");

    let silent = Server::start(Vec::new());
    let port = silent.port;
    drop(silent);
    let host = RemoteHost::connect(&request(&format!("http://127.0.0.1:{port}")), Instant::now()).expect("connect");
    match host.release("abc") {
        RemoteReleaseOutcome::ReleaseUnknown { attempts, .. } => assert_eq!(attempts, 3),
        other => panic!("unexpected {other:?}"),
    }
}
