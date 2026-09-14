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
        provider: "grid".into(),
        browser: crate::BackendKind::SafariWebDriver,
    }
}

#[test]
fn allocation_sends_one_authenticated_new_session_and_parses_the_id() {
    let server = Server::start(vec![Reply::json(
        200,
        &json!({"value": {"sessionId": "abc-123", "capabilities": {"browserName": "safari"}}}),
    )]);
    let request = request(&server.endpoint("/wd/hub"));
    let mut host = RemoteHost::connect(&request).expect("connect");
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
    let mut host = RemoteHost::connect(&request).expect("connect");
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
    let mut host = RemoteHost::connect(&request).expect("connect");
    let started = Instant::now();
    let failure = host.allocate(&request).expect_err("timed out");
    assert!(
        started.elapsed() < Duration::from_millis(800),
        "the allocation timeout bounds the whole call"
    );
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
    let mut host = RemoteHost::connect(&request).expect("connect");
    assert!(matches!(
        host.allocate(&request).expect_err("no id"),
        RemoteStartFailure::AllocationUnknown { .. }
    ));
}

#[test]
fn endpoint_rules_are_enforced_before_any_request() {
    let mut bad = request("https://user:key@grid.example.net/wd/hub");
    assert!(matches!(
        RemoteHost::connect(&bad).expect_err("userinfo"),
        RemoteStartFailure::InvalidEndpoint(_)
    ));
    bad.endpoint = "http://grid.example.net/wd/hub".into();
    assert!(RemoteHost::connect(&bad).is_err(), "plain http to a remote host");
}

fn allocated(session_id: &str) -> Reply {
    Reply::json(200, &json!({"value": {"sessionId": session_id, "capabilities": {}}}))
}

fn wait_for_expiry(host: &RemoteHost, within: Duration) -> Option<RemoteExpiry> {
    let started = Instant::now();
    while started.elapsed() < within {
        if let Some(expired) = host.check_expiry() {
            return Some(expired);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    host.check_expiry()
}

#[test]
fn the_watchdog_releases_at_the_hard_deadline_by_itself_and_activity_never_extends_it() {
    let server = Server::start(vec![allocated("hard-1"), Reply::json(200, &json!({"value": null}))]);
    let mut request = request(&server.endpoint(""));
    request.max_session = Duration::from_millis(500);
    request.idle_release = Duration::from_millis(300);
    let mut host = RemoteHost::connect(&request).expect("connect");
    assert_eq!(host.check_expiry(), None, "no clock runs before a session exists");
    host.allocate(&request).expect("allocated");
    let started = Instant::now();
    while host.check_expiry().is_none() && started.elapsed() < Duration::from_secs(3) {
        host.note_activity(Instant::now());
        std::thread::sleep(Duration::from_millis(40));
    }
    assert_eq!(host.check_expiry(), Some(RemoteExpiry::HardDeadline));
    assert!(
        started.elapsed() >= Duration::from_millis(450),
        "not before the deadline"
    );
    // The driver was never asked to release; the watchdog already did, and
    // release hands back that settled outcome without a second delete.
    assert_eq!(host.release("hard-1"), RemoteReleaseOutcome::Released);
    let seen = server.recorded();
    assert_eq!(seen.len(), 2, "one New Session and exactly one DELETE");
    assert_eq!(
        (seen[1].method.as_str(), seen[1].path.as_str()),
        ("DELETE", "/session/hard-1")
    );
}

#[test]
fn the_idle_clock_starts_when_allocation_succeeds() {
    let server = Server::start(vec![
        allocated("idle-1").delayed(Duration::from_millis(700)),
        Reply::json(200, &json!({"value": null})),
    ]);
    let mut request = request(&server.endpoint(""));
    request.allocation_timeout = Duration::from_secs(5);
    request.max_session = Duration::from_secs(30);
    request.idle_release = Duration::from_millis(400);
    let mut host = RemoteHost::connect(&request).expect("connect");
    host.allocate(&request)
        .expect("allocation slower than idle_release still succeeds");
    assert_eq!(
        host.check_expiry(),
        None,
        "the allocation wait does not count as idling"
    );
    assert_eq!(wait_for_expiry(&host, Duration::from_secs(3)), Some(RemoteExpiry::Idle));
    assert_eq!(host.release("idle-1"), RemoteReleaseOutcome::Released);
    assert_eq!(server.recorded().len(), 2, "the watchdog's DELETE is the only one");
}

#[test]
fn release_before_expiry_stops_the_watchdog_and_deletes_once() {
    let server = Server::start(vec![
        allocated("early-1"),
        Reply::json(200, &json!({"value": null})),
        Reply::json(200, &json!({"value": null})),
    ]);
    let mut request = request(&server.endpoint(""));
    request.max_session = Duration::from_millis(300);
    request.idle_release = Duration::from_millis(200);
    let mut host = RemoteHost::connect(&request).expect("connect");
    host.allocate(&request).expect("allocated");
    assert_eq!(host.release("early-1"), RemoteReleaseOutcome::Released);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(server.recorded().len(), 2, "no delete after the driver released");
    assert_eq!(host.check_expiry(), None);
}

#[test]
fn release_is_verified_or_reported_unknown_after_bounded_attempts() {
    let released = Server::start(vec![Reply::json(200, &json!({"value": null}))]);
    let first = request(&released.endpoint(""));
    let mut host = RemoteHost::connect(&first).expect("connect");
    assert_eq!(host.release("abc"), RemoteReleaseOutcome::Released);
    assert_eq!(released.recorded()[0].path, "/session/abc");

    let gone = Server::start(vec![Reply::json(
        404,
        &json!({"value": {"error": "invalid session id", "message": "gone"}}),
    )]);
    let mut host = RemoteHost::connect(&request(&gone.endpoint(""))).expect("connect");
    assert_eq!(host.release("abc"), RemoteReleaseOutcome::AlreadyGone);

    let refused = Server::start(vec![Reply::json(
        500,
        &json!({"value": {"error": "unknown error", "message": "busy"}}),
    )]);
    let mut host = RemoteHost::connect(&request(&refused.endpoint(""))).expect("connect");
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
    let mut host = RemoteHost::connect(&request(&format!("http://127.0.0.1:{port}"))).expect("connect");
    match host.release("abc") {
        RemoteReleaseOutcome::ReleaseUnknown { attempts, .. } => assert_eq!(attempts, 3),
        other => panic!("unexpected {other:?}"),
    }
}
