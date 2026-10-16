use super::*;
use crate::cloud_runtime::tailnet::{
    Selection,
    receipt::{
        AllocationRequestId, CaptureAttempt, DeviceId, EnrollmentLedger, EnrollmentProvider, TailnetId, WorkerId,
    },
};
use horizon_cloud::{Cancellation, provider::Kind};
use horizon_cloud_protocol::OperationId;
use std::os::unix::fs::PermissionsExt;

fn ledger(captured: bool) -> EnrollmentLedger {
    let mut ledger = EnrollmentLedger::default();
    let attempt = CaptureAttempt {
        attempt_id: OperationId::generate(),
        tailnet_id: TailnetId::new("work").unwrap(),
        provider: EnrollmentProvider::new(Kind::RunPod),
        worker_id: WorkerId::new("worker1").unwrap(),
        allocation_request_id: AllocationRequestId::new("deleted-cloud").unwrap(),
    };
    ledger.arm(attempt.clone()).unwrap();
    if captured {
        ledger
            .record(attempt.attempt_id, DeviceId::new("nRecorded").unwrap())
            .unwrap();
    }
    ledger
}

fn request(root: &std::path::Path, cloud: &std::path::Path, state: &Deployment) -> Request {
    let key = root.join("runpod");
    let ssh = root.join("ssh");
    for path in [&key, &ssh] {
        std::fs::write(path, b"synthetic-key\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::write(root.join("ssh.pub"), format!("{PUBLIC_KEY}\n")).unwrap();
    let settings: Settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file": key, "ssh_identity_file": ssh, "docker_config": root.join("docker"),
        "registry_pull_auth_id": null, "cpu_flavors": [], "gpu_types": []
    }))
    .unwrap();
    Request::new(
        state.cloud_id.clone(),
        state.repository.clone(),
        state.revision.clone(),
        state.profile.clone(),
        cloud.into(),
        settings,
    )
}

fn evidence(store: &Store) -> Vec<Vec<u8>> {
    ["deployment.json", "workspace-volume.json", "workspace-volume.required"]
        .into_iter()
        .map(|name| std::fs::read(store.root().join(name)).unwrap())
        .collect()
}

#[test]
fn deleted_selected_legacy_cloud_cannot_redeploy_and_erase_its_missing_identity() {
    let root = tempfile::tempdir().unwrap();
    let cloud = root.path().join("deleted-cloud");
    let mut state = deleted(&serde_json::json!({"state":"terminated","worker_id":"worker1"}));
    state.repository = root.path().join("missing-repository");
    state.revision = "a".repeat(40);
    state.registry_generation = None;
    let request = request(root.path(), &cloud, &state);
    {
        let store = Store::lock(&cloud).unwrap();
        write_matching_journal(&cloud, state.spec.as_ref().unwrap(), "deleted");
        state.stage = Stage::DeleteStorage;
        super::super::super::deletion::finish_deletion(state.clone(), &store).unwrap();
        Selection {
            tailnet: Some("work".into()),
        }
        .commit(&cloud)
        .unwrap();
    }
    super::super::super::prepare(&request).unwrap();
    let before = {
        let store = Store::lock(&cloud).unwrap();
        evidence(&store)
    };
    let deletion =
        super::super::super::deletion::terminate(&cloud, &request.settings, &Cancellation::default(), &|_| {})
            .unwrap_err();
    assert!(deletion.to_string().contains("not recorded"), "{deletion}");
    let deployment = super::super::super::deploy(&request, &Cancellation::default(), &|_| {}).unwrap_err();
    assert!(deployment.to_string().contains("not recorded"), "{deployment}");
    let store = Store::lock(&cloud).unwrap();
    assert_eq!(
        evidence(&store),
        before,
        "no journal release, source export or replacement reset"
    );
    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.stage, Stage::Deleted);
    assert!(matches!(saved.operation, CreateState::Terminated { .. }));
    assert_eq!(saved.ready_history, ReadyHistory::Observed);
    assert!(saved.source_ready && saved.tailnet_enrollment.is_empty());
}

#[test]
fn unresolved_receipt_or_attempt_refuses_before_key_validation_or_journal_release() {
    for captured in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut state = deleted(&serde_json::json!({"state":"terminated","worker_id":"worker1"}));
        state.tailnet_enrollment = ledger(captured);
        store.save(&state).unwrap();
        write_matching_journal(root.path(), state.spec.as_ref().unwrap(), "deleted");
        let before = evidence(&store);
        let memory = serde_json::to_vec(&state).unwrap();
        let error = reopen(&store, &mut state, "not-a-key").unwrap_err();
        assert!(
            error
                .to_string()
                .contains(if captured { "unconfirmed" } else { "not recorded" }),
            "{error}"
        );
        assert_eq!(serde_json::to_vec(&state).unwrap(), memory);
        assert_eq!(evidence(&store), before);
    }
}

#[test]
fn confirmed_receipts_fresh_selection_and_no_tailnet_still_reopen() {
    for case in 0..3 {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut state = deleted(&serde_json::json!({"state":"terminated","worker_id":"worker1"}));
        if case == 0 {
            state.tailnet_enrollment = ledger(true);
            let receipt = state.tailnet_enrollment.receipts()[0].clone();
            state.tailnet_enrollment.confirm_removed(&receipt).unwrap();
        } else if case == 1 {
            state.operation = CreateState::Prepared;
            state.ready_history = ReadyHistory::Unobserved;
            state.source_ready = false;
            state.ready_after_seconds = None;
            state.stop_requested = false;
            Selection {
                tailnet: Some("work".into()),
            }
            .commit(root.path())
            .unwrap();
        }
        let retained = state.tailnet_enrollment.clone();
        store.save(&state).unwrap();
        write_matching_journal(root.path(), state.spec.as_ref().unwrap(), "deleted");
        reopen(&store, &mut state, PUBLIC_KEY).unwrap();
        assert_eq!(state.stage, Stage::Validate);
        assert_eq!(state.operation, CreateState::Prepared);
        assert_eq!(state.tailnet_enrollment, retained);
        assert_eq!(store.load().unwrap().unwrap().tailnet_enrollment, retained);
        assert!(!root.path().join("workspace-volume.json").exists());
        assert!(!root.path().join("workspace-volume.required").exists());
    }
}
