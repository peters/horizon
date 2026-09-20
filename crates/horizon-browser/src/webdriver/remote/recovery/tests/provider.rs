use super::*;

fn recovery(hub: &Server, report: &Server) -> RemoteAllocation {
    let allocation = RemoteAllocation::default();
    let client = |server: &Server| {
        Arc::new(
            RemoteHttpClient::new(
                &server.endpoint(""),
                Some(RemoteAuthorizationHeader::new("Bearer original-secret".into()).expect("header")),
            )
            .expect("client"),
        )
    };
    allocation.identify(client(hub), "exact-session".into(), Some(client(report)));
    allocation.finish(None);
    allocation
}

fn record(execution: &str) -> serde_json::Value {
    json!({"automation_session": {
        "hashed_id": "exact-session", "status": "passed", "browserstack_status": execution,
        "reason": "CLIENT_STOPPED_SESSION"
    }})
}

#[test]
fn completed_provider_execution_releases_only_the_original_exact_session() {
    let hub = Server::start(vec![Reply::json(500, &json!({"value":{"error":"unknown error"}}))]);
    let report = Server::start(vec![Reply::json(200, &record("done"))]);
    let allocation = recovery(&hub, &report);
    allocation.reconcile();
    assert_eq!(wait(&allocation), RemoteRecoveryStatus::Released);
    allocation.reconcile();
    let requests = report.recorded();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/automate/sessions/exact-session.json");
    assert_eq!(requests[0].authorization.as_deref(), Some("Bearer original-secret"));
    assert!(allocation.state.lock().expect("state").identity.is_none());
}

#[test]
fn provider_timeouts_and_errors_release_only_matching_execution_records() {
    for execution in ["timeout", "error"] {
        for matches in [true, false] {
            let mut body = record(execution);
            if !matches {
                body["automation_session"]["hashed_id"] = json!("another-session");
            }
            let hub = Server::start(vec![Reply::json(500, &json!({"value": null}))]);
            let report = Server::start(vec![Reply::json(200, &body)]);
            let allocation = recovery(&hub, &report);
            allocation.reconcile();
            let expected = if matches {
                RemoteRecoveryStatus::Released
            } else {
                RemoteRecoveryStatus::UnsupportedResponse
            };
            assert_eq!(wait(&allocation), expected);
            assert_eq!(allocation.is_released(), matches);
        }
    }
}

#[test]
fn timeout_or_error_test_metadata_cannot_release_running_or_unreported_execution() {
    for verdict in ["timeout", "error"] {
        for running in [true, false] {
            let mut body = record("running");
            body["automation_session"]["status"] = json!(verdict);
            if !running {
                body["automation_session"]
                    .as_object_mut()
                    .expect("record")
                    .remove("browserstack_status");
            }
            let hub = Server::start(vec![Reply::json(500, &json!({"value": null}))]);
            let report = Server::start(vec![Reply::json(200, &body)]);
            let allocation = recovery(&hub, &report);
            allocation.reconcile();
            let expected = if running {
                RemoteRecoveryStatus::Active
            } else {
                RemoteRecoveryStatus::UnsupportedResponse
            };
            assert_eq!(wait(&allocation), expected);
            assert!(!allocation.is_released());
        }
    }
}

#[test]
fn reporting_never_uses_test_verdicts_missing_ids_or_account_counts_as_release_proof() {
    let mut wrong_id = record("done");
    wrong_id["automation_session"]["hashed_id"] = json!("different-session");
    for body in [
        wrong_id,
        json!({"automation_session":{"status":"done","hashed_id":"exact-session"}}),
        json!({"automation_session":{"status":"done","browserstack_status":"done"}}),
        json!({"running_sessions":0,"queued_sessions":0}),
        record("passed"),
        record("failed"),
        record("unknown"),
    ] {
        let hub = Server::start(vec![Reply::json(500, &json!({"value":null}))]);
        let report = Server::start(vec![Reply::json(200, &body)]);
        let allocation = recovery(&hub, &report);
        allocation.reconcile();
        assert_eq!(wait(&allocation), RemoteRecoveryStatus::UnsupportedResponse);
        assert!(!allocation.is_released());
    }
}

#[test]
fn report_authentication_ambiguity_and_active_execution_keep_capacity() {
    for (status, body, expected) in [
        (401, record("done"), RemoteRecoveryStatus::AuthenticationRequired),
        (403, record("done"), RemoteRecoveryStatus::AuthenticationRequired),
        (404, record("done"), RemoteRecoveryStatus::UnsupportedResponse),
        (500, record("done"), RemoteRecoveryStatus::ProviderUnavailable),
        (200, record("running"), RemoteRecoveryStatus::Active),
    ] {
        let hub = Server::start(vec![Reply::json(500, &json!({"value":null}))]);
        let report = Server::start(vec![Reply::json(status, &body)]);
        let allocation = recovery(&hub, &report);
        allocation.reconcile();
        assert_eq!(wait(&allocation), expected);
    }
}

#[test]
fn active_hub_or_rejected_credentials_cannot_be_overridden_by_reporting() {
    for (reply, expected) in [
        (
            Reply::json(200, &json!({"value":"https://example.test"})),
            RemoteRecoveryStatus::Active,
        ),
        (
            Reply::json(401, &record("done")),
            RemoteRecoveryStatus::AuthenticationRequired,
        ),
        (
            Reply::json(403, &record("done")),
            RemoteRecoveryStatus::AuthenticationRequired,
        ),
    ] {
        let hub = Server::start(vec![reply]);
        let report = Server::start(vec![]);
        let allocation = recovery(&hub, &report);
        allocation.reconcile();
        assert_eq!(wait(&allocation), expected);
        assert!(report.recorded().is_empty());
    }
}

#[test]
fn transport_failures_keep_the_hold_even_if_reporting_could_claim_completion() {
    let mut truncated = Reply::json(200, &json!({"value":null}));
    truncated.declared_length = Some(4096);
    let hub = Server::start(vec![truncated]);
    let report = Server::start(vec![]);
    let allocation = recovery(&hub, &report);
    allocation.reconcile();
    assert_eq!(wait(&allocation), RemoteRecoveryStatus::ProviderUnavailable);
    assert!(report.recorded().is_empty());
}

#[test]
fn reporting_gateway_outages_are_distinct_from_unsupported_session_formats() {
    for status in [500, 501, 502, 503, 504, 599] {
        for body in [b"<html>unavailable</html>".to_vec(), b"{}".to_vec()] {
            let hub = Server::start(vec![Reply::json(500, &json!({"value":null}))]);
            let mut reply = Reply::json(status, &json!({}));
            reply.body = body;
            let report = Server::start(vec![reply]);
            let allocation = recovery(&hub, &report);
            allocation.reconcile();
            assert_eq!(wait(&allocation), RemoteRecoveryStatus::ProviderUnavailable);
        }
    }
}

#[test]
fn lifecycle_retains_the_original_reporting_binding_after_failed_release() {
    let hub = Server::start(vec![
        Reply::json(200, &json!({"value":{"sessionId":"exact-session","capabilities":{}}})),
        Reply::json(500, &json!({"value":{"error":"unknown error","message":"ended"}})),
        Reply::json(500, &json!({"value":{"error":"unknown error","message":"ended"}})),
    ]);
    let report = Server::start(vec![
        Reply::json(200, &record("running")),
        Reply::json(200, &record("done")),
    ]);
    let mut request = crate::webdriver::remote::tests::request(&hub.endpoint(""));
    request.evidence = crate::webdriver::remote::identity::DeviceEvidenceSource::BrowserstackSession {
        api_endpoint: report.endpoint(""),
    };
    let mut host = RemoteHost::connect(&request).expect("host");
    host.allocate(&request).expect("allocation");
    let outcome = host.release("exact-session");
    assert!(matches!(outcome, RemoteReleaseOutcome::Failed { .. }));
    request.recovery.finish(Some(&outcome));
    request.evidence = crate::webdriver::remote::identity::DeviceEvidenceSource::Capabilities;
    request.authorization = None;
    drop(host);
    request.recovery.reconcile();
    assert_eq!(wait(&request.recovery), RemoteRecoveryStatus::Released);
    let seen = report.recorded();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].authorization.as_deref(), Some("Basic c2VjcmV0"));
    assert_eq!(seen[1].path, "/automate/sessions/exact-session.json");
}
