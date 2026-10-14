use super::*;
use crate::cloud_runtime::WorkerContract;
use std::cell::Cell;

#[test]
fn selected_entrypoint_requires_capture_while_none_keeps_existing_behavior() {
    for selected in [None, Some("saved-test-network".to_owned())] {
        let (root, store, mut state, _) = fixture();
        let prior = state.tailnet_enrollment.clone();
        let selection = super::super::Selection {
            tailnet: selected.clone(),
        };
        let selection_path = root.path().join("tailnet.json");
        std::fs::write(selection_path, serde_json::to_vec(&selection).unwrap()).unwrap();
        let connection = Connection {
            host: "unused.invalid".into(),
            port: 22,
            identity: root.path().join("unused-key"),
            known_hosts: root.path().join("unused-hosts"),
            host_key_alias: "unused".into(),
        };
        let cancel = horizon_cloud::Cancellation::default();
        cancel.cancel();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| panic!("transport must not run"),
            secrets: vec![],
        };
        let contract = WorkerContract::reported("horizon-tailnet-contract=4");
        let result = super::super::configure_and_record(&connection, &store, &mut state, &contract, &runner);
        if selected.is_some() {
            assert!(matches!(result, Err(Error::Invalid(_))));
        } else {
            result.unwrap();
            assert!(state.tailnet_device.is_none());
        }
        assert_eq!(store.load().unwrap().unwrap().tailnet_enrollment, prior);
    }
}

fn fixture() -> (tempfile::TempDir, state::Store, state::Deployment, CaptureAttempt) {
    let root = tempfile::tempdir().unwrap();
    let store = state::Store::lock(root.path()).unwrap();
    let mut state: state::Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"synthetic","repository":"/synthetic","revision":"a",
        "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
        "stage":"Provision","operation":{"state":"bound","worker_id":"synthetic-worker"},
        "spec":null,"worker":null,"sessions":[]
    }))
    .unwrap();
    state.tailnet_enrollment = super::super::receipt::tests::populated();
    let attempt = CaptureAttempt {
        attempt_id: OperationId::generate(),
        tailnet_id: TailnetId::new("saved-test-network").unwrap(),
        provider: EnrollmentProvider::new(horizon_cloud::provider::Kind::RunPod),
        worker_id: WorkerId::new("synthetic-worker").unwrap(),
        allocation_request_id: AllocationRequestId::new("synthetic-allocation").unwrap(),
    };
    store.save(&state).unwrap();
    (root, store, state, attempt)
}

#[test]
fn capture_arms_before_enrollment_and_persists_before_any_later_stage() {
    let (_root, store, mut state, attempt) = fixture();
    let prior = state.tailnet_enrollment.clone();
    let configure_called = Cell::new(false);
    let id = attempt.attempt_id;
    capture(
        &store,
        &mut state,
        attempt,
        || {
            let saved = store.load()?.unwrap();
            assert!(
                saved
                    .tailnet_enrollment
                    .attempts()
                    .iter()
                    .any(|entry| entry.attempt_id == id)
            );
            configure_called.set(true);
            Ok(())
        },
        || {
            assert!(configure_called.get());
            Ok(DeviceId::new("nCapturedSyntheticOnly").unwrap())
        },
    )
    .unwrap();
    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.version, state::ENROLLMENT_RECORD_VERSION);
    assert_eq!(saved.tailnet_enrollment.receipts().len(), prior.receipts().len() + 1);
    assert_eq!(saved.tailnet_enrollment.attempts(), prior.attempts());
    assert_eq!(saved.tailnet_enrollment, state.tailnet_enrollment);
    assert!(saved.tailnet_device.is_none());
}

#[test]
fn repeated_capture_deduplicates_same_identity_and_retains_replaced_identity() {
    let (_root, store, mut state, template) = fixture();
    let prior = state.tailnet_enrollment.clone();
    for (device, added) in [("nCurrent", 1), ("nCurrent", 1), ("nReplacement", 2)] {
        let mut attempt = template.clone();
        attempt.attempt_id = OperationId::generate();
        capture(
            &store,
            &mut state,
            attempt,
            || Ok(()),
            || Ok(DeviceId::new(device).unwrap()),
        )
        .unwrap();
        let saved = store.load().unwrap().unwrap();
        assert_eq!(
            saved.tailnet_enrollment.receipts().len(),
            prior.receipts().len() + added
        );
        assert_eq!(saved.tailnet_enrollment.attempts(), prior.attempts());
    }
}

#[test]
fn failed_initial_save_prevents_enrollment_and_failed_configure_preserves_unknown_attempt() {
    let (root, store, mut state, attempt) = fixture();
    let calls = Cell::new(0);
    std::fs::remove_file(root.path().join("deployment.json")).unwrap();
    std::fs::create_dir(root.path().join("deployment.json")).unwrap();
    assert!(
        capture(
            &store,
            &mut state,
            attempt,
            || {
                calls.set(calls.get() + 1);
                Ok(())
            },
            || unreachable!()
        )
        .is_err()
    );
    assert_eq!(calls.get(), 0);
    let (_root, store, mut state, attempt) = fixture();
    let id = attempt.attempt_id;
    assert!(
        capture(
            &store,
            &mut state,
            attempt,
            || Err(Error::Invalid("synthetic refusal")),
            || unreachable!()
        )
        .is_err()
    );
    assert!(
        store
            .load()
            .unwrap()
            .unwrap()
            .tailnet_enrollment
            .attempts()
            .iter()
            .any(|entry| entry.attempt_id == id)
    );
}

#[test]
fn failed_capture_or_receipt_save_retains_prior_receipts_and_unknown_attempts() {
    for fail_save in [false, true] {
        let (root, store, mut state, attempt) = fixture();
        let prior = state.tailnet_enrollment.clone();
        let result = capture(
            &store,
            &mut state,
            attempt,
            || Ok(()),
            || {
                if !fail_save {
                    return Err(Error::Invalid("synthetic capture failure"));
                }
                std::fs::rename(root.path().join("deployment.json"), root.path().join("armed.json"))?;
                std::fs::create_dir(root.path().join("deployment.json"))?;
                Ok(DeviceId::new("nSyntheticOnly").unwrap())
            },
        );
        assert!(result.is_err());
        if fail_save {
            std::fs::remove_dir(root.path().join("deployment.json")).unwrap();
            std::fs::rename(root.path().join("armed.json"), root.path().join("deployment.json")).unwrap();
        }
        let saved = store.load().unwrap().unwrap();
        assert_eq!(saved.tailnet_enrollment.receipts(), prior.receipts());
        assert_eq!(saved.tailnet_enrollment.attempts().len(), prior.attempts().len() + 1);
        assert_eq!(state.tailnet_enrollment, saved.tailnet_enrollment);
    }
}

#[test]
fn projected_reports_are_strict_bounded_and_bound_to_the_saved_selection() {
    let selected = TailnetId::new("saved-test-network").unwrap();
    let valid = br#"{"version":1,"tailnet_id":"saved-test-network","device_id":"nSyntheticOnly"}"#;
    assert_eq!(observed(valid, &selected).unwrap().as_str(), "nSyntheticOnly");
    for invalid in [
        br#"{"version":2,"tailnet_id":"saved-test-network","device_id":"nSyntheticOnly"}"#.as_slice(),
        br#"{"version":1,"tailnet_id":"other","device_id":"nSyntheticOnly"}"#,
        br#"{"version":1,"tailnet_id":"saved-test-network","device_id":""}"#,
        br#"{"version":1,"tailnet_id":"saved-test-network","device_id":"one","device_id":"two"}"#,
        br#"{"version":1,"tailnet_id":"saved-test-network","device_id":"one","Peer":[]}"#,
        &[b'x'; 1025],
    ] {
        assert_eq!(observed(invalid, &selected).unwrap_err().to_string(), CAPTURE_ERROR);
    }
}

#[test]
fn private_projection_transport_cancellation_never_emits_output() {
    let cancel = horizon_cloud::Cancellation::default();
    cancel.cancel();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| panic!("private output escaped"),
        secrets: Vec::new(),
    };
    assert!(
        runner
            .private_exchange(
                &mut std::process::Command::new("missing-command"),
                PROJECTION.as_bytes(),
                Duration::from_secs(1)
            )
            .is_err()
    );
}

#[test]
fn cancellation_during_capture_keeps_the_armed_attempt_and_prior_receipts() {
    let (_root, store, mut state, attempt) = fixture();
    let prior = state.tailnet_enrollment.clone();
    let cancel = horizon_cloud::Cancellation::default();
    let signal = cancel.clone();
    let cancel_task = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        signal.cancel();
    });
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| panic!("private output escaped"),
        secrets: Vec::new(),
    };
    let result = capture(
        &store,
        &mut state,
        attempt,
        || Ok(()),
        || {
            runner.private_exchange(
                std::process::Command::new("sh").args(["-c", "sleep 30"]),
                PROJECTION.as_bytes(),
                Duration::from_secs(2),
            )?;
            unreachable!()
        },
    );
    cancel_task.join().unwrap();
    assert!(matches!(
        result,
        Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
    ));
    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.tailnet_enrollment.receipts(), prior.receipts());
    assert_eq!(saved.tailnet_enrollment.attempts().len(), prior.attempts().len() + 1);
}

#[test]
fn capture_binding_requires_the_exact_worker_endpoint_and_owning_cloud_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("synthetic");
    let connection = Connection {
        host: "127.0.0.1".into(),
        port: 2222,
        identity: root.join("unused-private-identity"),
        known_hosts: root.join("known-hosts-synthetic-worker"),
        host_key_alias: "horizon-cloud-synthetic-worker".into(),
    };
    assert!(validate_binding(&connection, &root, "synthetic", "synthetic-worker", "127.0.0.1", 2222).is_ok());
    for (cloud, worker, host, port) in [
        ("other-cloud", "synthetic-worker", "127.0.0.1", 2222),
        ("synthetic", "other-worker", "127.0.0.1", 2222),
        ("synthetic", "synthetic-worker", "other.invalid", 2222),
        ("synthetic", "synthetic-worker", "127.0.0.1", 22),
    ] {
        assert!(validate_binding(&connection, &root, cloud, worker, host, port).is_err());
    }
    assert!(
        validate_binding(
            &connection,
            &directory.path().join("another-root/synthetic"),
            "synthetic",
            "synthetic-worker",
            "127.0.0.1",
            2222
        )
        .is_err()
    );
}
