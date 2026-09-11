use super::*;

fn selection() -> RunPodNetworkVolumeExpectation {
    RunPodNetworkVolumeExpectation {
        volume_id: "volume_synthetic".into(),
        data_center_id: "DC-synthetic".into(),
        minimum_size_gb: 10,
    }
}

fn allocate(f: &Fixture) -> StoredRemoteAllocation {
    allocate_start(&f.store, &f.dormant, i64::MAX, Some(&selection())).expect("selected allocation")
}

fn finish(
    f: &Fixture,
    allocation: &StoredRemoteAllocation,
) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
    finish_start(
        &f.store,
        &f.identities,
        &f.key,
        &f.profile,
        allocation,
        |trust, request, selected| {
            assert_eq!(selected, Some(&selection()));
            assert_eq!(*request, f.allocation().worker_request().expect("request"));
            Ok(f.factory(&trust))
        },
    )
}

fn resume(f: &Fixture, operation: Operation) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
    super::super::dispatch(
        &f.store,
        &f.identities,
        &f.key,
        &f.profile,
        &f.allocation(),
        operation,
        |trust, request, selected| {
            assert_eq!(selected, Some(&selection()));
            assert_eq!(*request, f.allocation().recovery_request().expect("request"));
            Ok(f.factory(&trust))
        },
    )
}

#[test]
fn selection_precedes_identity_intent_and_claim_and_survives_retained_recovery() {
    let f = Fixture::new();
    let allocation = allocate(&f);
    assert_eq!(f.counts(), [1, 1, 0, 0]);
    assert!(f.keys().is_empty());
    assert_eq!(
        f.store
            .load_remote_network_volume_selection(&allocation)
            .expect("selection"),
        Some(selection())
    );
    assert_eq!(f.allocation(), allocation, "selection does not revise allocation");
    let saved = finish(&f, &allocation).expect("start");
    let before = f.snapshot();
    let mut replacement = selection();
    replacement.volume_id = "replacement".into();
    assert!(matches!(
        f.store.record_remote_network_volume_selection(&saved, &replacement),
        Err(RemoteWorkspaceStoreError::ReplacementIdentityMismatch)
    ));
    for operation in [Operation::Retry, Operation::Recover] {
        assert_eq!(resume(&f, operation).expect("retained"), saved);
    }
    assert_eq!(f.snapshot(), before);
    assert_eq!(f.counts(), [1, 1, 1, 0]);
    assert_eq!(*f.remote.calls.lock().expect("calls"), [1, 1, 0, 2, 0]);
    assert_eq!(
        f.store.load_remote_network_volume_selection(&saved).expect("selection"),
        Some(selection())
    );
}

#[test]
fn public_invalid_selection_does_not_allocate_and_a_valid_start_remains_available() {
    let f = Fixture::new();
    let mut invalid = selection();
    invalid.volume_id = "private-selection-sentinel/invalid".into();
    let error = start_task_free_runpod_workspace_with_network_volume(
        &f.store,
        &f.identities,
        &f.key,
        &f.profile,
        &f.dormant,
        i64::MAX,
        &invalid,
    )
    .expect_err("invalid selection");
    assert_eq!(error, RunPodWorkspaceSetupError::InvalidNetworkVolumeSelection);
    assert!(!format!("{error:?} {error}").contains("private-selection-sentinel"));
    assert_eq!(f.counts(), [0; 4]);
    assert!(f.keys().is_empty());
    assert!(
        f.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("lookup")
            .is_none()
    );
    let allocation = allocate(&f);
    finish(&f, &allocation).expect("valid selected start");
    assert_eq!(f.counts(), [1, 1, 1, 0]);
}

#[test]
fn invalid_network_profile_refuses_before_allocation_and_binding_errors_are_redacted() {
    let mut f = Fixture::new();
    f.profile.data_center_id = Some("private-profile-sentinel".into());
    let error = start_task_free_runpod_workspace_with_network_volume(
        &f.store,
        &f.identities,
        &f.key,
        &f.profile,
        &f.dormant,
        i64::MAX,
        &selection(),
    )
    .expect_err("conflict");
    assert_eq!(error, RunPodWorkspaceSetupError::NetworkVolumeBindingRejected);
    assert!(!format!("{error:?} {error}").contains("private-profile-sentinel"));
    assert_eq!(f.counts(), [0; 4]);
    assert!(f.keys().is_empty());
    let allocation = allocate(&f);
    let saved = prepare_identity(&f.store, &f.identities, &allocation).expect("identity");
    let request = saved.worker_request().expect("request");
    let trust = RunPodHostTrust::initial_task_free(&f.key, &request).expect("trust");
    assert!(matches!(
        provider(
            &f.store,
            &f.key,
            &f.profile,
            TrustSelection::Initial(trust),
            &request,
            Some(&selection())
        ),
        Err(RunPodWorkspaceSetupError::NetworkVolumeBindingRejected)
    ));
    assert_eq!(f.counts(), [1, 1, 0, 0]);
}

#[test]
fn pre_intent_interruptions_never_backfill_selection_or_prepare_a_replacement_key() {
    for stage in 0..3 {
        let f = Fixture::new();
        let allocation =
            allocate_start(&f.store, &f.dormant, i64::MAX, (stage != 0).then_some(&selection())).expect("allocate");
        if stage == 2 {
            prepare_identity(&f.store, &f.identities, &allocation).expect("identity");
        }
        let before = f.snapshot();
        for operation in [Operation::Retry, Operation::Recover] {
            assert_eq!(
                resume(&f, operation),
                Err(if stage == 2 {
                    RunPodWorkspaceSetupError::FirstPinIntentUnavailable
                } else {
                    RemoteWorkspaceRecoveryError::MissingRequest.into()
                })
            );
        }
        assert_eq!(f.snapshot(), before);
        assert_eq!(
            f.store
                .load_remote_network_volume_selection(&f.allocation())
                .expect("selection"),
            (stage != 0).then_some(selection())
        );
        assert_eq!(*f.remote.calls.lock().expect("calls"), [0; 5]);
    }
}

#[test]
fn lost_response_reopens_with_saved_selection_and_never_ensures_twice() {
    let f = Fixture::new();
    let allocation = allocate(&f);
    f.remote.lose_response.store(true, Ordering::SeqCst);
    assert_eq!(
        finish(&f, &allocation),
        Err(RemoteWorkspaceSetupError::ProviderUnavailable.into())
    );
    let before = f.snapshot();
    let reopened = CloudWorkflowStore::open_path(f.store.path()).expect("reopen");
    super::super::dispatch(
        &reopened,
        &f.identities,
        &f.key,
        &f.profile,
        &before.allocation,
        Operation::Retry,
        |trust, request, selected| {
            assert_eq!(selected, Some(&selection()));
            assert_eq!(*request, before.allocation.recovery_request().expect("original"));
            Ok(f.factory(&trust))
        },
    )
    .expect("noncreating recovery");
    resume(&f, Operation::Retry).expect("retained retry");
    assert_eq!(*f.remote.calls.lock().expect("calls"), [1, 1, 1, 1, 0]);
    assert_eq!(f.keys(), before.keys);
    assert_eq!(f.counts(), [1, 1, 1, 0]);
}

#[test]
fn corrupt_selection_is_not_absence_or_an_ordinary_provider_fallback() {
    let f = Fixture::new();
    let allocation = allocate(&f);
    let allocation = prepare_identity(&f.store, &f.identities, &allocation).expect("identity");
    f.store.record_remote_first_pin_intent(&allocation).expect("intent");
    let raw = rusqlite::Connection::open(f.store.path()).expect("database");
    let trigger: String = raw
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='remote_network_volume_selections_no_update'",
            [],
            |row| row.get(0),
        )
        .expect("trigger");
    raw.execute_batch(
        "DROP TRIGGER remote_network_volume_selections_no_update;
        UPDATE remote_network_volume_selections SET volume_id='volume_synthetic'||char(0)||'private-sentinel';",
    )
    .expect("corrupt fixture");
    raw.execute_batch(&trigger).expect("restore schema guard");
    let before = f.snapshot();
    for operation in [Operation::Retry, Operation::Recover] {
        let error = resume(&f, operation).expect_err("corrupt selection");
        assert_eq!(error, RemoteWorkspaceRecoveryError::StorageUnavailable.into());
        assert!(!format!("{error:?} {error}").contains("private-sentinel"));
    }
    assert_eq!(f.snapshot(), before);
    assert_eq!(*f.remote.calls.lock().expect("calls"), [0; 5]);
    assert_eq!(*f.remote.selections.lock().expect("selections"), [0; 2]);
}

#[test]
fn management_race_after_selected_factory_prevents_ensure_and_keeps_selection() {
    let f = Fixture::new();
    let allocation = allocate(&f);
    let allocation = prepare_identity(&f.store, &f.identities, &allocation).expect("identity");
    f.store.record_remote_first_pin_intent(&allocation).expect("intent");
    let keys = f.keys();
    let result = super::super::dispatch(
        &f.store,
        &f.identities,
        &f.key,
        &f.profile,
        &allocation,
        Operation::Retry,
        |trust, _, selected| {
            assert_eq!(selected, Some(&selection()));
            record_management(&f.store, &allocation);
            Ok(f.factory(&trust))
        },
    );
    assert_eq!(result, Err(RemoteWorkspaceRecoveryError::StateChanged.into()));
    assert_eq!(*f.remote.calls.lock().expect("calls"), [0; 5]);
    assert_eq!(f.keys(), keys);
    assert_eq!(
        f.store
            .load_remote_network_volume_selection(&f.allocation())
            .expect("selection"),
        Some(selection())
    );
}

#[test]
fn expired_selected_setup_and_factory_failure_never_renew_creation_authority() {
    let f = Fixture::new();
    let allocation = allocate(&f);
    let allocation = prepare_identity(&f.store, &f.identities, &allocation).expect("identity");
    f.store.record_remote_first_pin_intent(&allocation).expect("intent");
    let before = f.snapshot();
    let failed = super::super::dispatch(
        &f.store,
        &f.identities,
        &f.key,
        &f.profile,
        &allocation,
        Operation::Retry,
        |_, _, _| -> Result<Provider, _> { Err(RunPodWorkspaceSetupError::NetworkVolumeBindingRejected) },
    );
    assert_eq!(failed, Err(RunPodWorkspaceSetupError::NetworkVolumeBindingRejected));
    assert_eq!(f.snapshot(), before);
    assert_eq!(*f.remote.calls.lock().expect("calls"), [0; 5]);
    let mut workflow = allocation.workflow().workflow().clone();
    workflow.created_at_millis = 1000;
    workflow.updated_at_millis = 1000;
    workflow.retain_until_millis = 2000;
    rusqlite::Connection::open(f.store.path()).expect("database").execute(
        "UPDATE cloud_workflows SET created_at_millis=1000,updated_at_millis=1000,retain_until_millis=2000,snapshot=?1 WHERE workflow_id=?2",
        rusqlite::params![serde_json::to_vec(&workflow).expect("snapshot"), workflow.id.to_string()],
    ).expect("expire fixture");
    resume(&f, Operation::Retry).expect("noncreating expired recovery");
    assert_eq!(f.keys(), before.keys);
    assert_eq!(*f.remote.calls.lock().expect("calls"), [0, 0, 1, 0, 0]);
    assert_eq!(f.counts(), [1, 1, 0, 1]);
    assert_eq!(
        f.store
            .load_remote_network_volume_selection(&f.allocation())
            .expect("selection"),
        Some(selection())
    );
}
