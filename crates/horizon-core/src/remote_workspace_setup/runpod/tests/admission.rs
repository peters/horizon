use super::*;

#[test]
fn every_public_entry_rejects_invalid_profiles_before_key_or_provider_work() {
    for case in 0..4 {
        let mut f = Fixture::new();
        match case {
            0 => f.profile.name = "private-profile-sentinel".into(),
            1 => f.profile.gpu_count = 0,
            2 => f.profile.ports.clear(),
            _ => f.profile.gpu_type_ids.clear(),
        }
        let error = start_task_free_runpod_workspace(&f.store, &f.identities, &f.key, &f.profile, &f.dormant, i64::MAX)
            .expect_err("invalid profile");
        assert_eq!(error, RunPodWorkspaceSetupError::InvalidProfile);
        assert!(!format!("{error:?} {error}").contains("private-profile-sentinel"));
        assert_eq!(f.counts(), [0; 4]);
        assert!(f.keys().is_empty());
        let saved = f.reserve(true);
        let before = f.snapshot();
        for operation in [retry_runpod_workspace_setup, recover_runpod_workspace] {
            assert_eq!(
                operation(&f.store, &f.identities, &f.key, &f.profile, &saved),
                Err(RunPodWorkspaceSetupError::InvalidProfile)
            );
        }
        assert_eq!(f.snapshot(), before);
    }
}

#[test]
fn invalid_target_stale_selection_and_bad_retention_do_not_allocate_or_prepare() {
    for case in 0..3 {
        let mut f = Fixture::new();
        if case < 2 {
            let mut state = f.dormant.state().clone();
            match case {
                0 => state.spec.target.provider = CloudProvider::LocalDocker,
                _ => state.spec.working_directory = "changed".into(),
            }
            let changed = f
                .store
                .replace_remote_workspace(&f.dormant, &state)
                .expect("fixture edit");
            if case == 0 {
                f.dormant = changed;
            }
        }
        let retained = if case == 2 { 0 } else { i64::MAX };
        assert!(f.start_using(retained, |_| panic!("no factory")).is_err());
        assert_eq!(f.counts(), [0; 4]);
        assert!(f.keys().is_empty());
    }
    let f = Fixture::new();
    let mut state = f.dormant.state().clone();
    state.spec.target.image = "example/worker:mutable".into();
    assert!(f.store.replace_remote_workspace(&f.dormant, &state).is_err());
    assert_eq!(
        validate_profile(&state.spec.target, &f.profile),
        Err(RunPodWorkspaceSetupError::InvalidProfile)
    );
    assert_eq!(f.counts(), [0; 4]);
    assert!(f.keys().is_empty());
}

#[test]
fn pre_intent_interruptions_never_mint_authority_on_retry_or_recovery() {
    for stage in 0..3 {
        let f = Fixture::new();
        if stage == 0 {
            f.store
                .allocate_remote_runtime(&f.dormant, i64::MAX)
                .expect("allocation only");
        } else {
            let reserved = f.reserve(false);
            if stage == 2 {
                let status = status(&reserved.worker_request().expect("request"), true);
                f.store
                    .record_remote_worker_recovery(&reserved, Some(&status))
                    .expect("legacy worker without pin");
            }
        }
        f.assert_refused(&if stage == 0 {
            RemoteWorkspaceRecoveryError::MissingRequest.into()
        } else {
            RunPodWorkspaceSetupError::FirstPinIntentUnavailable
        });
    }
}

#[test]
fn missing_and_mismatched_private_keys_are_not_replaced() {
    for missing in [false, true] {
        let f = Fixture::new();
        let saved = f.reserve(true);
        let request = saved.worker_request().expect("request");
        let identity = f
            .identities
            .recover(request.workflow_id, request.job_id, &request.ssh_public_key)
            .expect("identity");
        if missing {
            std::fs::remove_file(identity.private_key_path()).expect("remove fixture key");
        } else {
            let other = f
                .identities
                .prepare_new(
                    crate::cloud_run::CloudWorkflowId::new(),
                    crate::cloud_run::CloudJobId::new(),
                )
                .expect("other fixture key");
            std::fs::copy(other.private_key_path(), identity.private_key_path()).expect("substitute fixture bytes");
        }
        f.assert_refused(&if missing {
            RemoteSshIdentityError::Missing.into()
        } else {
            RemoteSshIdentityError::Mismatch.into()
        });
    }
}

#[test]
fn management_and_foreign_allocations_refuse_before_the_factory() {
    let f = Fixture::new();
    let saved = f.reserve(true);
    let foreign = Fixture::new().reserve(true);
    assert!(
        f.run_expected(&foreign, Operation::Recover, |_| panic!("foreign dispatch"))
            .is_err()
    );
    record_management(&f.store, &saved);
    f.assert_refused(&RemoteWorkspaceRecoveryError::ManagementPending.into());
}
