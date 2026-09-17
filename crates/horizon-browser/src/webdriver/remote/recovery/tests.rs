use super::{RemoteAllocation, RemoteRecoveryStatus};
use crate::webdriver::remote::{RemoteHost, RemoteReleaseOutcome};
use crate::webdriver::remote_http::{RemoteAuthorizationHeader, RemoteHttpClient};
use crate::webdriver::test_server::{Reply, Server};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn wait(allocation: &RemoteAllocation) -> RemoteRecoveryStatus {
    let deadline = Instant::now() + Duration::from_secs(3);
    while allocation.status() == RemoteRecoveryStatus::Reconciling {
        assert!(Instant::now() < deadline, "bounded fixture probe");
        std::thread::sleep(Duration::from_millis(5));
    }
    allocation.status()
}

fn owned(server: &Server) -> RemoteAllocation {
    let allocation = RemoteAllocation::default();
    let header = RemoteAuthorizationHeader::new("Bearer original-secret".into()).expect("header");
    allocation.identify(
        Arc::new(RemoteHttpClient::new(&server.endpoint("/wd/hub"), Some(header)).expect("transport")),
        "private-session".into(),
    );
    allocation.finish(None);
    allocation
}

#[test]
fn lost_teardown_response_is_reconciled_without_reallocating_or_releasing_another_session() {
    let mut truncated = Reply::json(200, &json!({"value":null}));
    truncated.declared_length = Some(1024);
    let mut second_truncated = Reply::json(200, &json!({"value":null}));
    second_truncated.declared_length = Some(1024);
    let mut third_truncated = Reply::json(200, &json!({"value":null}));
    third_truncated.declared_length = Some(1024);
    let server = Server::start(vec![
        Reply::json(200, &json!({"value":{"sessionId":"private-session","capabilities":{}}})),
        truncated,
        second_truncated,
        third_truncated,
        Reply::json(
            404,
            &json!({"value":{"error":"invalid session id","message":"private-session"}}),
        ),
        Reply::json(200, &json!({"value":{"sessionId":"next-session","capabilities":{}}})),
        Reply::json(200, &json!({"value":null})),
    ]);
    let request = super::super::tests::request(&server.endpoint("/wd/hub"));
    let mut host = RemoteHost::connect(&request).expect("host");
    host.allocate(&request).expect("allocated");
    let outcome = host.release("private-session");
    assert!(matches!(
        outcome,
        RemoteReleaseOutcome::ReleaseUnknown { attempts: 3, .. }
    ));
    let allocation = &request.recovery;
    allocation.finish(Some(&outcome));
    assert_eq!(allocation.status(), RemoteRecoveryStatus::Unresolved);
    allocation.reconcile();
    assert_eq!(wait(allocation), RemoteRecoveryStatus::Released);
    allocation.reconcile();
    assert_eq!(server.recorded().len(), 5, "repeated reconciliation is local");
    let next = super::super::tests::request(&server.endpoint("/wd/hub"));
    let mut next_host = RemoteHost::connect(&next).expect("same provider");
    next_host.allocate(&next).expect("next allocation without restart");
    assert_eq!(next_host.release("next-session"), RemoteReleaseOutcome::Released);
    let seen = server.recorded();
    assert_eq!(seen[4].method, "GET");
    assert_eq!(seen[4].path, "/wd/hub/session/private-session/url");
    assert_eq!(seen[4].authorization.as_deref(), Some("Basic c2VjcmV0"));
    let debug = format!("{allocation:?}");
    assert!(!debug.contains("private-session") && !debug.contains("c2VjcmV0"));
}

#[test]
fn active_authentication_errors_outages_and_non_session_404s_keep_the_hold() {
    for (reply, expected) in [
        (
            Reply::json(200, &json!({"value":"https://public.example/"})),
            RemoteRecoveryStatus::Active,
        ),
        (
            Reply::json(401, &json!({"value":{"error":"invalid session id"}})),
            RemoteRecoveryStatus::AuthenticationRequired,
        ),
        (
            Reply::json(403, &json!({"value":{"error":"invalid session id"}})),
            RemoteRecoveryStatus::AuthenticationRequired,
        ),
        (
            Reply::json(404, &json!({"value":{"error":"unknown command"}})),
            RemoteRecoveryStatus::ProviderUnavailable,
        ),
        (
            Reply::json(500, &json!({"value":{"error":"invalid session id"}})),
            RemoteRecoveryStatus::ProviderUnavailable,
        ),
        (
            Reply::json(200, &json!({"value":null})),
            RemoteRecoveryStatus::ProviderUnavailable,
        ),
    ] {
        let server = Server::start(vec![reply]);
        let allocation = owned(&server);
        allocation.reconcile();
        assert_eq!(wait(&allocation), expected);
        assert!(!allocation.is_released());
    }
}

#[test]
fn active_driver_missing_identity_and_concurrent_calls_do_not_start_extra_probes() {
    let allocation = RemoteAllocation::default();
    allocation.reconcile();
    assert_eq!(allocation.status(), RemoteRecoveryStatus::InUse);
    allocation.finish(None);
    allocation.reconcile();
    assert_eq!(allocation.status(), RemoteRecoveryStatus::IdentityUnavailable);
    let server = Server::start(vec![
        Reply::json(404, &json!({"value":{"error":"invalid session id"}})).delayed(Duration::from_millis(100)),
    ]);
    let allocation = owned(&server);
    allocation.reconcile();
    allocation.reconcile();
    assert_eq!(wait(&allocation), RemoteRecoveryStatus::Released);
    assert_eq!(server.recorded().len(), 1);
}

#[test]
fn successful_release_discards_credentials_and_never_probes_again() {
    for outcome in [
        RemoteReleaseOutcome::Released,
        RemoteReleaseOutcome::AlreadyGone,
        RemoteReleaseOutcome::NeverAllocated,
    ] {
        let allocation = RemoteAllocation::default();
        allocation.finish(Some(&outcome));
        allocation.reconcile();
        assert_eq!(allocation.status(), RemoteRecoveryStatus::Released);
    }
}

#[test]
fn identity_is_retained_before_device_validation_can_reject_the_allocation() {
    let server = Server::start(vec![
        Reply::json(200, &json!({"value":{"sessionId":"rejected-device","capabilities":{}}})),
        Reply::json(500, &json!({"value":{"error":"unknown error","message":"refused"}})),
        Reply::json(404, &json!({"value":{"error":"invalid session id"}})),
    ]);
    let mut request = super::super::tests::request(&server.endpoint(""));
    request.device.kind = crate::remote::DeviceKind::Physical;
    let mut host = RemoteHost::connect(&request).expect("host");
    assert!(host.allocate(&request).is_err());
    request.recovery.finish(None);
    request.recovery.reconcile();
    assert_eq!(wait(&request.recovery), RemoteRecoveryStatus::Released);
}

#[test]
fn revoked_credentials_cannot_turn_a_delete_into_release_proof() {
    let server = Server::start(vec![Reply::json(
        401,
        &json!({"value":{"error":"invalid session id","message":"secret fixture-private-id"}}),
    )]);
    let mut host = RemoteHost::connect(&super::super::tests::request(&server.endpoint(""))).expect("host");
    let outcome = host.release("fixture-private-id");
    assert!(matches!(outcome, RemoteReleaseOutcome::Failed { .. }));
    assert!(!format!("{outcome:?}").contains("secret"));
    assert!(!format!("{outcome:?}").contains("fixture-private-id"));
}

#[test]
fn authorization_is_checked_with_retirement_before_probe_admission() {
    let server = Server::start(vec![Reply::json(404, &json!({"value":{"error":"invalid session id"}}))]);
    let allocation = owned(&server);
    allocation.mark_published();
    let mut state = allocation.state.lock().expect("state");
    state.retired = false;
    state.status = RemoteRecoveryStatus::InUse;
    let caller = allocation.clone();
    let worker = std::thread::spawn(move || caller.reconcile_for("host", "old-owner", "workspace", true));
    state.scope = Some(super::RemoteAllocationScope {
        host: "host".into(),
        owner: Some("final-owner".into()),
        workspace: Some("workspace".into()),
    });
    state.retired = true;
    state.status = RemoteRecoveryStatus::Unresolved;
    drop(state);
    assert!(!worker.join().expect("caller"));
    assert!(server.recorded().is_empty());
    assert_eq!(allocation.status_for("host", "old-owner", "workspace", true), None);
    assert!(allocation.reconcile_for("host", "final-owner", "workspace", false));
    assert_eq!(wait(&allocation), RemoteRecoveryStatus::Released);
}
