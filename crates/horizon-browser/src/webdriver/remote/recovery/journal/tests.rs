use super::*;
use crate::webdriver::remote::RemoteReleaseOutcome;
use crate::webdriver::{
    remote::tests::request,
    test_server::{Reply, Server},
};
use serde_json::json;
use std::time::{Duration, Instant};

#[test]
fn failed_release_write_keeps_identity_and_retries_without_provider_access() {
    for outcome in [
        RemoteReleaseOutcome::Released,
        RemoteReleaseOutcome::AlreadyGone,
        RemoteReleaseOutcome::NeverAllocated,
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("identity");
        let request = request("http://127.0.0.1:1/wd/hub");
        request.recovery.retain_journal(&path, &request).unwrap();
        let identified = !matches!(outcome, RemoteReleaseOutcome::NeverAllocated);
        if identified {
            let host = RemoteHost::connect(&request).unwrap();
            request
                .recovery
                .identify(host.transport, "exact-session".into(), host.report)
                .unwrap();
        }
        std::fs::create_dir(path.with_extension("pending")).unwrap();
        request.recovery.finish(Some(&outcome));
        assert_failed_release(&request.recovery, &path, identified);
        // A failed concurrent probe launch cannot overwrite confirmed release.
        request.recovery.state.lock().unwrap().reconciliation_unavailable();
        assert_failed_release(&request.recovery, &path, identified);
        request.recovery.finish(None);
        request.recovery.reconcile();
        assert_failed_release(&request.recovery, &path, identified);

        std::fs::remove_dir(path.with_extension("pending")).unwrap();
        request.recovery.reconcile();
        assert!(request.recovery.is_released());
        request.recovery.state.lock().unwrap().reconciliation_unavailable();
        assert!(request.recovery.is_released());
        assert!(
            RemoteAllocation::restore_released_journal(&path)
                .unwrap()
                .unwrap()
                .is_released()
        );
        assert!(request.recovery.state.lock().unwrap().identity.is_none());
    }
}

#[test]
fn probe_release_is_not_published_until_journal_commit_succeeds() {
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
    std::fs::create_dir(path.with_extension("pending")).unwrap();
    request.recovery.finish(None);
    request.recovery.reconcile();
    let deadline = Instant::now() + Duration::from_secs(3);
    while request.recovery.status() == RemoteRecoveryStatus::Reconciling {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_failed_release(&request.recovery, &path, true);
    assert_eq!(server.recorded().len(), 1);
    std::fs::remove_dir(path.with_extension("pending")).unwrap();
    request.recovery.reconcile();
    assert!(request.recovery.is_released());
    assert_eq!(server.recorded().len(), 1, "retry must use retained release proof");
    assert!(RemoteAllocation::restore_released_journal(&path).unwrap().is_some());
}

fn assert_failed_release(allocation: &RemoteAllocation, path: &Path, identified: bool) {
    assert_eq!(allocation.status(), RemoteRecoveryStatus::PersistenceUnavailable);
    assert!(!allocation.is_released());
    let state = allocation.state.lock().unwrap();
    assert_eq!(state.identity.is_some(), identified);
    assert!(!state.journal.as_ref().unwrap().record.released);
    drop(state);
    let record = Record::load(path).unwrap();
    assert!(!record.released);
    assert_eq!(record.session.as_deref(), identified.then_some("exact-session"));
    assert!(RemoteAllocation::restore_released_journal(path).unwrap().is_none());
}

#[test]
fn durable_release_restores_without_provider_access() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("identity");
    let request = request("http://127.0.0.1:1/wd/hub");
    request.recovery.retain_journal(&path, &request).unwrap();
    assert!(RemoteAllocation::restore_released_journal(&path).unwrap().is_none());
    let host = RemoteHost::connect(&request).unwrap();
    request
        .recovery
        .identify(host.transport, "exact-session".into(), host.report)
        .unwrap();
    assert!(RemoteAllocation::restore_released_journal(&path).unwrap().is_none());
    request.recovery.finish(Some(&RemoteReleaseOutcome::Released));
    let reference = request.recovery.reference().to_owned();
    drop(request);

    let restored = RemoteAllocation::restore_released_journal(&path).unwrap().unwrap();
    assert_eq!(restored.reference(), reference);
    restored.reconcile();
    assert!(restored.is_released());

    let mut record: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record["version"] = 2.into();
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(RemoteAllocation::restore_released_journal(&path).is_err());
}

#[test]
#[cfg(unix)]
fn released_journals_still_require_private_files() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("identity");
    let request = request("http://127.0.0.1:1/wd/hub");
    request.recovery.retain_journal(&path, &request).unwrap();
    request
        .recovery
        .finish(Some(&crate::webdriver::remote::RemoteReleaseOutcome::Released));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(RemoteAllocation::restore_released_journal(&path).is_err());
}

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
