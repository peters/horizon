use super::*;
use crate::webdriver::{
    remote::tests::request,
    test_server::{Reply, Server},
};
use serde_json::json;
use std::time::{Duration, Instant};

#[test]
fn restarted_allocation_probes_only_its_bound_session_and_persists_release() {
    let server = Server::start(vec![Reply::json(404, &json!({"value":{"error":"invalid session id"}}))]);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("identity");
    let request = request(&server.endpoint("/wd/hub"));
    request.recovery.retain_journal(&path, &request).unwrap();
    let host = RemoteHost::connect(&request).unwrap();
    request
        .recovery
        .identify(host.transport, "exact-session".into(), host.report)
        .unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(!contents.contains("c2VjcmV0"));
    let restored = RemoteAllocation::restore_journal(&path, &request).unwrap();
    assert_eq!(restored.reference(), request.recovery.reference());
    assert_eq!(restored.status(), RemoteRecoveryStatus::Unresolved);
    restored.record_admission("restarted-host", "original-owner", "cloud");
    assert!(!restored.reconcile_for("restarted-host", "other-owner", "cloud", false));
    assert!(restored.reconcile_for("restarted-host", "original-owner", "cloud", true));
    let deadline = Instant::now() + Duration::from_secs(3);
    while restored.status() == RemoteRecoveryStatus::Reconciling {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(restored.is_released());
    assert!(
        RemoteAllocation::restore_journal(&path, &request)
            .unwrap()
            .is_released()
    );
    let seen = server.recorded();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "GET");
    assert_eq!(seen[0].path, "/wd/hub/session/exact-session/url");
}

#[test]
fn unavailable_identity_and_changed_account_never_release_or_allocate() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("identity");
    let mut request = request("http://127.0.0.1:1/wd/hub");
    request.recovery.retain_journal(&path, &request).unwrap();
    let restored = RemoteAllocation::restore_journal(&path, &request).unwrap();
    restored.reconcile();
    assert_eq!(restored.status(), RemoteRecoveryStatus::IdentityUnavailable);
    request.quota_key = "different-account".into();
    assert!(RemoteAllocation::restore_journal(&path, &request).is_err());
}

#[test]
fn failure_to_save_received_identity_prevents_session_use() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("identity");
    let request = request("http://127.0.0.1:1/wd/hub");
    request.recovery.retain_journal(&path, &request).unwrap();
    std::fs::create_dir(path.with_extension("pending")).unwrap();
    let host = RemoteHost::connect(&request).unwrap();
    assert!(
        request
            .recovery
            .identify(host.transport, "exact-session".into(), host.report)
            .is_err()
    );
    let restored = RemoteAllocation::restore_journal(&path, &request).unwrap();
    assert_eq!(restored.status(), RemoteRecoveryStatus::IdentityUnavailable);
}
