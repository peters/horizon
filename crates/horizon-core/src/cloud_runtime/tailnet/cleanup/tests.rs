use super::*;

mod deletion_proof;
mod missing_identity;
use crate::cloud_runtime::tailnet::receipt::{
    AllocationRequestId, CaptureAttempt, DeviceId, EnrollmentLedger, EnrollmentProvider, TailnetId, WorkerId,
};
use horizon_cloud::{provider::Kind, tailnet::RetentionReason};
use horizon_cloud_protocol::OperationId;

fn provider_id(kind: Kind) -> String {
    horizon_cloud::provider::ALL
        .into_iter()
        .find(|provider| provider.kind == kind)
        .unwrap()
        .id
        .into()
}

fn fixture() -> (tempfile::TempDir, state::Store, state::Deployment) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("cloud");
    let store = state::Store::lock(&root).unwrap();
    let mut state: state::Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"cloud","repository":"/synthetic","revision":"a",
        "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
        "stage":"Deleted","operation":{"state":"terminated","worker_id":"worker"},
        "worker":null,"spec":{"operation_id":"cloud","image_digest":"sha256:synthetic",
            "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
            "public_key":"synthetic","registry_auth_id":null,"gpu_types":[],"cpu_flavors":[],"data_centers":[]},"sessions":[]
    }))
    .unwrap();
    add(&mut state.tailnet_enrollment, "n-exact-recorded");
    store.save(&state).unwrap();
    state = store.load().unwrap().unwrap();
    (directory, store, state)
}
fn add(ledger: &mut EnrollmentLedger, device: &str) {
    let attempt = CaptureAttempt {
        attempt_id: OperationId::generate(),
        tailnet_id: TailnetId::new("work").unwrap(),
        provider: EnrollmentProvider::new(Kind::RunPod),
        worker_id: WorkerId::new("worker").unwrap(),
        allocation_request_id: AllocationRequestId::new("cloud").unwrap(),
    };
    ledger.arm(attempt.clone()).unwrap();
    ledger
        .record(attempt.attempt_id, DeviceId::new(device).unwrap())
        .unwrap();
}

#[test]
fn confirmed_ids_remain_durable_and_a_retry_does_not_delete_them_again() {
    let (_directory, store, mut state) = fixture();
    run(&store, &mut state, &|_| {}, |receipt, budget| {
        assert_eq!(receipt.device_id.as_str(), "n-exact-recorded");
        assert!(budget <= Duration::from_secs(30));
        NodeDeletion::ConfirmedDeleted
    })
    .unwrap();
    let mut saved = store.load().unwrap().unwrap();
    assert!(saved.tailnet_enrollment.receipts()[0].node_deleted);
    assert_eq!(
        saved.tailnet_enrollment.receipts()[0].device_id.as_str(),
        "n-exact-recorded"
    );
    assert!(!saved.tailnet_enrollment.cleanup_pending());
    run(&store, &mut saved, &|_| {}, |_, _| {
        panic!("confirmed ID must not be sent again")
    })
    .unwrap();
}

#[test]
fn one_exact_node_retained_across_worker_replacements_is_deleted_only_once() {
    let (_directory, store, mut state) = fixture();
    let attempt = CaptureAttempt {
        attempt_id: OperationId::generate(),
        tailnet_id: TailnetId::new("work").unwrap(),
        provider: EnrollmentProvider::new(Kind::RunPod),
        worker_id: WorkerId::new("replacement-worker").unwrap(),
        allocation_request_id: AllocationRequestId::new("cloud").unwrap(),
    };
    state.tailnet_enrollment.arm(attempt.clone()).unwrap();
    state
        .tailnet_enrollment
        .record(attempt.attempt_id, DeviceId::new("n-exact-recorded").unwrap())
        .unwrap();
    store.save(&state).unwrap();
    let mut calls = 0;
    run(&store, &mut state, &|_| {}, |_, _| {
        calls += 1;
        NodeDeletion::ConfirmedDeleted
    })
    .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(state.tailnet_enrollment.receipts().len(), 2);
    assert!(
        store
            .load()
            .unwrap()
            .unwrap()
            .tailnet_enrollment
            .receipts()
            .iter()
            .all(|entry| entry.node_deleted)
    );
}

#[test]
fn encoded_confirmation_must_fit_before_any_irreversible_request() {
    let (_directory, store, mut state) = fixture();
    let receipts: Vec<_> = (0..128)
        .map(|index| {
            serde_json::json!({
                "tailnet_id":"work", "provider":"runpod", "worker_id":format!("worker{index}"),
                "allocation_request_id":"cloud", "device_id":"n"
            })
        })
        .collect();
    let mut value = serde_json::json!({"receipts":receipts,"attempts":[]});
    let mut remaining = 64 * 1024 - serde_json::to_vec(&value).unwrap().len();
    for receipt in value["receipts"].as_array_mut().unwrap() {
        let added = remaining.min(510);
        receipt["device_id"] = serde_json::json!(format!("n{}{}", "\"".repeat(added / 2), "q".repeat(added % 2)));
        remaining -= added;
    }
    assert_eq!(remaining, 0);
    state.tailnet_enrollment = serde_json::from_value(value).unwrap();
    let before = state.tailnet_enrollment.clone();
    store.save(&state).unwrap();
    assert!(
        run(&store, &mut state, &|_| {}, |_, _| panic!(
            "confirmation cannot be persisted"
        ))
        .is_err()
    );
    assert_eq!(state.tailnet_enrollment, before);
    assert_eq!(store.load().unwrap().unwrap().tailnet_enrollment, before);
}

#[test]
fn all_retained_api_outcomes_name_the_exact_id_and_preserve_retry_evidence() {
    for reason in [
        RetentionReason::MissingCredential,
        RetentionReason::UnavailableCredential,
        RetentionReason::Transport,
        RetentionReason::Authorization,
        RetentionReason::NotFoundUnverified,
        RetentionReason::UnexpectedResponse,
    ] {
        let (_directory, store, mut state) = fixture();
        let before = state.tailnet_enrollment.clone();
        let error = run(&store, &mut state, &|_| {}, |_, _| NodeDeletion::Retained(reason)).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("n-exact-recorded") && text.contains("manually") && text.contains("retry"));
        assert!(text.contains(reason.message()));
        assert_eq!(store.load().unwrap().unwrap().tailnet_enrollment, before);
        assert_eq!(state.tailnet_enrollment, before);
    }
}

#[test]
fn partial_confirmation_is_saved_before_the_next_failure_and_retry_only_sends_pending_ids() {
    let (_directory, store, mut state) = fixture();
    add(&mut state.tailnet_enrollment, "n-next-recorded");
    store.save(&state).unwrap();
    assert!(
        run(&store, &mut state, &|_| {}, |receipt, _| {
            if receipt.device_id.as_str() == "n-exact-recorded" {
                NodeDeletion::ConfirmedDeleted
            } else {
                NodeDeletion::Retained(RetentionReason::Transport)
            }
        })
        .is_err()
    );
    let mut state = store.load().unwrap().unwrap();
    assert!(state.tailnet_enrollment.receipts()[0].node_deleted);
    assert!(!state.tailnet_enrollment.receipts()[1].node_deleted);
    run(&store, &mut state, &|_| {}, |receipt, _| {
        assert_eq!(receipt.device_id.as_str(), "n-next-recorded");
        NodeDeletion::ConfirmedDeleted
    })
    .unwrap();
    assert_eq!(state.tailnet_enrollment.receipts().len(), 2);
}

#[test]
fn wrong_root_live_compute_or_allocation_binding_refuses_before_any_api_request() {
    for change in 0..3 {
        let (_directory, store, mut state) = fixture();
        match change {
            0 => state.cloud_id = "foreign".into(),
            1 => state.stage = Stage::Stopped,
            _ => {
                state.operation = CreateState::Bound {
                    worker_id: "worker".into(),
                }
            }
        }
        assert!(run(&store, &mut state, &|_| {}, |_, _| panic!("foreign/active record")).is_err());
    }
    let (_directory, store, mut state) = fixture();
    state.tailnet_enrollment = super::super::receipt::tests::populated();
    assert!(run(&store, &mut state, &|_| {}, |_, _| panic!("foreign allocation")).is_err());
}

#[test]
fn an_api_success_without_durable_local_confirmation_remains_uncertain_with_the_exact_id() {
    let (_directory, store, mut state) = fixture();
    let before = state.tailnet_enrollment.clone();
    let error = run(&store, &mut state, &|_| {}, |_, _| {
        std::fs::rename(store.root().join("deployment.json"), store.root().join("saved.json")).unwrap();
        std::fs::create_dir(store.root().join("deployment.json")).unwrap();
        NodeDeletion::ConfirmedDeleted
    })
    .unwrap_err();
    assert!(error.to_string().contains("n-exact-recorded") && error.to_string().contains("uncertain"));
    assert_eq!(state.tailnet_enrollment, before);
    std::fs::remove_dir(store.root().join("deployment.json")).unwrap();
    std::fs::rename(store.root().join("saved.json"), store.root().join("deployment.json")).unwrap();
    assert_eq!(store.load().unwrap().unwrap().tailnet_enrollment, before);
}

#[test]
fn legacy_missing_identity_and_unknown_attempts_never_infer_a_device() {
    let (_directory, store, mut state) = fixture();
    state.tailnet_enrollment = EnrollmentLedger::default();
    state.version = state.record_version();
    store.save(&state).unwrap();
    super::super::Selection {
        tailnet: Some("work".into()),
    }
    .commit(store.root())
    .unwrap();
    assert!(
        run(&store, &mut state, &|_| {}, |_, _| panic!("missing identity"))
            .unwrap_err()
            .to_string()
            .contains("not recorded")
    );
    state
        .tailnet_enrollment
        .arm(super::super::receipt::tests::populated().attempts()[0].clone())
        .unwrap();
    store.save(&state).unwrap();
    let before = state.tailnet_enrollment.clone();
    assert!(run(&store, &mut state, &|_| {}, |_, _| panic!("unknown attempt")).is_err());
    assert_eq!(store.load().unwrap().unwrap().tailnet_enrollment, before);
}

#[test]
fn confirming_a_known_receipt_never_deletes_or_confirms_an_unresolved_attempt() {
    let (_directory, store, mut state) = fixture();
    let attempt = CaptureAttempt {
        attempt_id: OperationId::generate(),
        tailnet_id: TailnetId::new("work").unwrap(),
        provider: EnrollmentProvider::new(Kind::RunPod),
        worker_id: WorkerId::new("unsettled-worker").unwrap(),
        allocation_request_id: AllocationRequestId::new("cloud").unwrap(),
    };
    state.tailnet_enrollment.arm(attempt.clone()).unwrap();
    store.save(&state).unwrap();
    let mut calls = 0;
    let error = run(&store, &mut state, &|_| {}, |receipt, _| {
        calls += 1;
        assert_eq!(receipt.device_id.as_str(), "n-exact-recorded");
        assert_eq!(receipt.worker_id.as_str(), "worker");
        NodeDeletion::ConfirmedDeleted
    })
    .unwrap_err();
    assert_eq!(calls, 1);
    assert!(error.to_string().contains("not recorded"));
    let mut saved = store.load().unwrap().unwrap();
    assert!(saved.tailnet_enrollment.receipts()[0].node_deleted);
    assert_eq!(saved.tailnet_enrollment.attempts(), &[attempt]);
    let before = saved.tailnet_enrollment.clone();
    assert!(
        run(&store, &mut saved, &|_| {}, |_, _| panic!(
            "an unresolved attempt supplies no device ID for API dispatch"
        ))
        .is_err()
    );
    assert_eq!(store.load().unwrap().unwrap().tailnet_enrollment, before);
}

#[test]
fn production_delete_with_disabled_api_binding_retains_the_exact_id_for_retry() {
    let (directory, store, state) = fixture();
    let before = state.tailnet_enrollment.clone();
    drop(store);
    let settings_root = directory.path();
    std::fs::write(
        settings_root.join("tailnets.json"),
        r#"{"tailnets":[{"id":"work","name":"Synthetic work"}]}"#,
    )
    .unwrap();
    std::fs::write(
        settings_root.join("tailnet-api-bindings.json"),
        r#"{"bindings":{"work":{"state":"disabled"}}}"#,
    )
    .unwrap();
    let root = directory.path().join("cloud");
    let settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file":root.join("absent-provider-key"), "ssh_identity_file":root.join("absent-ssh-key"),
        "docker_config":root.join("absent-docker-config"), "registry_pull_auth_id":null,
        "cpu_flavors":[], "gpu_types":[]
    }))
    .unwrap();
    for _ in 0..2 {
        let text = crate::cloud_runtime::deployment::terminate(
            &root,
            &settings,
            &horizon_cloud::Cancellation::default(),
            &|_| {},
        )
        .unwrap_err()
        .to_string();
        assert!(text.contains("n-exact-recorded") && text.contains("manually") && text.contains("retry"));
        assert!(text.contains(RetentionReason::MissingCredential.message()));
        assert_eq!(
            state::Store::lock(&root)
                .unwrap()
                .load()
                .unwrap()
                .unwrap()
                .tailnet_enrollment,
            before
        );
    }
}

#[test]
fn an_already_deleted_none_cloud_retries_without_provider_or_api_credentials() {
    let (directory, store, mut state) = fixture();
    state.tailnet_enrollment = EnrollmentLedger::default();
    state.version = state.record_version();
    store.save(&state).unwrap();
    drop(store);
    let root = directory.path().join("cloud");
    let settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file":root.join("absent-provider-key"), "ssh_identity_file":root.join("absent-ssh-key"),
        "docker_config":root.join("absent-docker-config"), "registry_pull_auth_id":null,
        "cpu_flavors":[], "gpu_types":[]
    }))
    .unwrap();
    crate::cloud_runtime::deployment::terminate(&root, &settings, &horizon_cloud::Cancellation::default(), &|_| {
        panic!("no request or progress needed")
    })
    .unwrap();
    assert!(
        !state::Store::lock(&root)
            .unwrap()
            .load()
            .unwrap()
            .unwrap()
            .tailnet_enrollment
            .cleanup_pending()
    );
}

#[test]
fn a_deleted_stage_with_unsettled_owned_journals_retains_exact_ids_before_http() {
    for file in [
        "workspace-volume.required",
        "workspace-volume.json",
        "docker-host.json",
        "hetzner.json",
    ] {
        let (_directory, store, mut state) = fixture();
        let before = state.tailnet_enrollment.clone();
        std::fs::write(store.root().join(file), "{}").unwrap();
        let error = run(&store, &mut state, &|_| {}, |_, _| {
            panic!("unconfirmed resources must not reach HTTP")
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("work") && error.contains("n-exact-recorded") && error.contains("no API request"));
        assert_eq!(state.tailnet_enrollment, before);
        assert_eq!(store.load().unwrap().unwrap().tailnet_enrollment, before);
    }
}

#[test]
fn another_original_provider_receipt_cannot_use_the_current_providers_absence() {
    let (_directory, store, mut state) = fixture();
    let mut ledger = serde_json::to_value(&state.tailnet_enrollment).unwrap();
    ledger["receipts"][0]["provider"] = serde_json::json!("hetzner");
    state.tailnet_enrollment = serde_json::from_value(ledger).unwrap();
    let before = state.tailnet_enrollment.clone();
    store.save(&state).unwrap();
    let error = run(&store, &mut state, &|_| {}, |_, _| {
        panic!("another provider must remain manual")
    })
    .unwrap_err();
    assert!(error.to_string().contains("work") && error.to_string().contains("n-exact-recorded"));
    assert_eq!(store.load().unwrap().unwrap().tailnet_enrollment, before);
}

#[test]
fn one_saved_tailnet_confirmation_does_not_claim_the_same_device_in_another_saved_binding() {
    let (_directory, store, mut state) = fixture();
    let attempt = CaptureAttempt {
        attempt_id: OperationId::generate(),
        tailnet_id: TailnetId::new("other-saved-tailnet").unwrap(),
        provider: EnrollmentProvider::new(Kind::RunPod),
        worker_id: WorkerId::new("replacement-worker").unwrap(),
        allocation_request_id: AllocationRequestId::new("cloud").unwrap(),
    };
    state.tailnet_enrollment.arm(attempt.clone()).unwrap();
    state
        .tailnet_enrollment
        .record(attempt.attempt_id, DeviceId::new("n-exact-recorded").unwrap())
        .unwrap();
    store.save(&state).unwrap();
    let mut calls = Vec::new();
    assert!(
        run(&store, &mut state, &|_| {}, |receipt, _| {
            calls.push(receipt.tailnet_id.as_str().to_owned());
            if receipt.tailnet_id.as_str() == "work" {
                NodeDeletion::ConfirmedDeleted
            } else {
                NodeDeletion::Retained(RetentionReason::NotFoundUnverified)
            }
        })
        .is_err()
    );
    assert_eq!(calls, ["work", "other-saved-tailnet"]);
    let retained = store.load().unwrap().unwrap().tailnet_enrollment;
    assert!(retained.receipts()[0].node_deleted);
    assert!(!retained.receipts()[1].node_deleted);
    assert_eq!(retained.receipts()[0].device_id, retained.receipts()[1].device_id);
}

#[test]
fn default_provider_receipts_use_the_same_automatic_cleanup_after_durable_absence() {
    for provider in [Kind::RunPod, Kind::Hetzner, Kind::DockerHost] {
        let (_directory, store, mut state) = fixture();
        state.profile.provider = provider_id(provider);
        state.spec.as_mut().unwrap().profile = state.profile.clone();
        let mut ledger = serde_json::to_value(&state.tailnet_enrollment).unwrap();
        ledger["receipts"][0]["provider"] = serde_json::json!(EnrollmentProvider::new(provider));
        state.tailnet_enrollment = serde_json::from_value(ledger).unwrap();
        match provider {
            Kind::RunPod => {}
            Kind::Hetzner => {
                let journal = horizon_cloud::hetzner::cloud::Journal {
                    location: Some("fixture-location".into()),
                    deleting: true,
                    volume: CreateState::Terminated {
                        worker_id: "1234".into(),
                    },
                    ..Default::default()
                };
                std::fs::write(store.root().join("hetzner.json"), serde_json::to_vec(&journal).unwrap()).unwrap();
            }
            Kind::DockerHost => std::fs::write(store.root().join("docker-host.json"), "{}").unwrap(),
        }
        store.save(&state).unwrap();
        let mut calls = 0;
        run(&store, &mut state, &|_| {}, |receipt, _| {
            assert_eq!(receipt.provider, EnrollmentProvider::new(provider));
            calls += 1;
            NodeDeletion::ConfirmedDeleted
        })
        .unwrap();
        assert_eq!(calls, 1);
        assert!(store.load().unwrap().unwrap().tailnet_enrollment.receipts()[0].node_deleted);
    }
}

#[test]
fn cancelled_deleted_retry_preserves_receipts_before_api_or_provider_access() {
    for operation in [
        CreateState::Prepared,
        CreateState::Terminated {
            worker_id: "worker".into(),
        },
    ] {
        let (_directory, store, mut state) = fixture();
        state.operation = operation;
        store.save(&state).unwrap();
        let before = state.tailnet_enrollment.clone();
        let root = store.root().to_owned();
        drop(store);
        let settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":root.join("absent-provider-key"),"ssh_identity_file":root.join("absent-ssh-key"),
            "docker_config":root.join("absent-docker-config"),"registry_pull_auth_id":null,
            "cpu_flavors":[],"gpu_types":[]
        }))
        .unwrap();
        let cancel = horizon_cloud::Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            crate::cloud_runtime::deployment::terminate(&root, &settings, &cancel, &|_| panic!("cancelled dispatch")),
            Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
        ));
        assert_eq!(
            state::Store::lock(&root)
                .unwrap()
                .load()
                .unwrap()
                .unwrap()
                .tailnet_enrollment,
            before
        );
    }
}

#[test]
fn original_record_removal_requires_node_confirmation_and_preserves_uncertain_attempts() {
    let (_temporary, store, mut state) = fixture();
    assert!(matches!(
        crate::cloud_runtime::lifecycle::can_remove(&store, &state),
        Err(Error::TailnetNodeCleanup { .. })
    ));
    let receipt = state.tailnet_enrollment.receipts()[0].clone();
    state.tailnet_enrollment.confirm_removed(&receipt).unwrap();
    store.save(&state).unwrap();
    assert!(crate::cloud_runtime::lifecycle::can_remove(&store, &state).unwrap());
    let attempt = CaptureAttempt {
        attempt_id: OperationId::generate(),
        tailnet_id: TailnetId::new("work").unwrap(),
        provider: EnrollmentProvider::new(Kind::RunPod),
        worker_id: WorkerId::new("unsettled").unwrap(),
        allocation_request_id: AllocationRequestId::new("cloud").unwrap(),
    };
    state.tailnet_enrollment.arm(attempt.clone()).unwrap();
    store.save(&state).unwrap();
    assert!(crate::cloud_runtime::lifecycle::can_remove(&store, &state).is_err());
    assert_eq!(store.load().unwrap().unwrap().tailnet_enrollment.attempts(), &[attempt]);
}
