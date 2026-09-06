use super::*;

#[test]
fn missing_requests_wrong_provider_and_stale_workspace_fail_before_io() {
    let fixture = Fixture::with_request(WorkerLifetime::Persistent, false);
    let provider = Provider::new(None);
    assert_eq!(fixture.observe(&provider), Err(Error::MissingRequest));
    assert!(provider.calls().is_empty());

    let fixture = Fixture::new();
    let mut provider = Provider::new(None);
    provider.kind = CloudProvider::RunPod;
    assert_eq!(fixture.observe(&provider), Err(Error::ProviderMismatch));
    assert!(provider.calls().is_empty());
    provider.kind = CloudProvider::LocalDocker;
    let before = fixture.reload();
    let mut state = before.workspace().state().clone();
    state.spec.working_directory = "nested".into();
    fixture
        .store
        .replace_remote_workspace(before.workspace(), &state)
        .expect("changed workspace");
    assert_eq!(
        observe_remote_environment(&fixture.store, &provider, before.workspace()),
        Err(Error::StateChanged)
    );
    assert!(provider.calls().is_empty());
    assert_eq!(fixture.counts(), [1, 1, 0]);
}

#[test]
fn wrong_owner_and_dormant_record_cannot_inspect_another_allocation() {
    let fixture = Fixture::new();
    let provider = Provider::new(None);
    let mut dormant = fixture.reload().workspace().state().clone();
    dormant.runtime = None;
    dormant.spec.generation = 0;
    let other_directory = tempfile::tempdir().expect("other store");
    let other_store =
        CloudWorkflowStore::open_path(other_directory.path().join("control/store.sqlite3")).expect("other store");
    let foreign = other_store
        .create_remote_workspace("00000000-0000-4000-8000-000000000002", &dormant)
        .expect("foreign owner");
    assert_eq!(
        observe_remote_environment(&fixture.store, &provider, &foreign),
        Err(Error::StorageUnavailable)
    );
    dormant.spec.workspace_local_id = "dormant".into();
    let dormant = fixture.store.create_remote_workspace(OWNER, &dormant).expect("dormant");
    assert_eq!(
        observe_remote_environment(&fixture.store, &provider, &dormant),
        Err(Error::MissingAllocation)
    );
    assert!(provider.calls().is_empty());
}

#[test]
fn mismatched_observations_are_rejected_without_saved_identity_changes() {
    let fixture = Fixture::new();
    let original = fixture.status();
    fixture.retain(&original);
    let before = fixture.reload();
    for fault in 0..13 {
        let mut status = original.clone();
        match fault {
            0 => status.worker.identity.provider = CloudProvider::RunPod,
            1 => status.worker.identity.workflow_id = crate::cloud_run::CloudWorkflowId::new(),
            2 => status.worker.identity.job_id = crate::cloud_run::CloudJobId::new(),
            3 => status.worker.identity.resource_id = "another-worker".into(),
            4 => status.worker.target.profile = "another-profile".into(),
            5 => status.worker.target.image = format!("example/worker@sha256:{}", "d".repeat(64)),
            6 => status.worker.target.disk_gib += 1,
            7 => status.worker.ssh_public_key = public_key(2),
            8 => status.ssh.as_mut().expect("ssh").host_key = public_key(8),
            9 => status.ssh.as_mut().expect("ssh").host = "127.0.0.2".into(),
            10 => status.ssh.as_mut().expect("ssh").port = 2223,
            11 => status.ssh.as_mut().expect("ssh").username = "another-user".into(),
            12 => status.ssh = None,
            _ => unreachable!(),
        }
        let provider = Provider::new(Some(status));
        assert_eq!(
            fixture.observe(&provider),
            Err(Error::InvalidObservation),
            "fault {fault}"
        );
        assert_eq!(provider.calls(), ["inspect"]);
        assert_eq!(fixture.reload(), before);
    }
    assert_eq!(fixture.counts(), [1, 1, 0]);
}

#[test]
fn late_workspace_or_workflow_changes_invalidate_presence_and_absence() {
    for workflow_change in [false, true] {
        for absent in [false, true] {
            let fixture = Fixture::new();
            let before = fixture.reload();
            let mut provider = Provider::new((!absent).then(|| fixture.status()));
            let expected = before.clone();
            let store = fixture.store.clone();
            provider.during_read = Some(Box::new(move || {
                if workflow_change {
                    let mut changed = expected.workflow().workflow().clone();
                    changed.updated_at_millis += 1;
                    store.replace(expected.workflow(), &changed).expect("workflow change");
                } else {
                    let mut changed = expected.workspace().state().clone();
                    changed.spec.working_directory = "changed".into();
                    store
                        .replace_remote_workspace(expected.workspace(), &changed)
                        .expect("workspace change");
                }
            }));
            assert_eq!(
                observe_remote_environment(&fixture.store, &provider, before.workspace()),
                Err(Error::StateChanged)
            );
            assert_eq!(provider.calls(), ["reconcile"]);
            assert_ne!(fixture.reload(), before);
            assert_eq!(fixture.counts(), [1, 1, 0]);
        }
    }
}

#[test]
fn provider_failures_are_redacted_and_do_not_become_absence() {
    let fixture = Fixture::new();
    let before = fixture.reload();
    let mut provider = Provider::new(None);
    provider.fail = true;
    let error = fixture.observe(&provider).expect_err("provider failure");
    assert_eq!(error, Error::ProviderUnavailable);
    assert!(!format!("{error} {error:?}").contains("private-provider-payload"));
    assert_eq!(fixture.reload(), before);
    assert_eq!(provider.calls(), ["reconcile"]);
    assert_eq!(fixture.counts(), [1, 1, 0]);
}

#[test]
fn expired_execution_lifetime_is_not_reported_ready_but_stopped_identity_is_visible() {
    let fixture = Fixture::with_request(WorkerLifetime::TimeLimited { seconds: 3600 }, true);
    let mut status = fixture.status();
    let before = fixture.reload();
    for deadline in ["2020-01-01T00:00:00Z", "2099-01-01T00:00:00Z"] {
        status.worker.lifetime = InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
            terminate_after: deadline.into(),
        });
        let provider = Provider::new(Some(status.clone()));
        assert_eq!(fixture.observe(&provider), Err(Error::InvalidObservation));
        assert_eq!(provider.calls(), ["reconcile"]);
        assert_eq!(fixture.reload(), before);
    }
    status.worker.lifetime = InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
        terminate_after: "2020-01-01T00:00:00Z".into(),
    });
    status.lifecycle = InteractiveWorkerLifecycle::Stopped;
    status.ssh = None;
    let provider = Provider::new(Some(status));
    assert_eq!(
        fixture
            .observe(&provider)
            .expect("stopped")
            .worker
            .expect("identity")
            .lifecycle,
        InteractiveWorkerLifecycle::Stopped
    );
    assert_eq!(provider.calls(), ["reconcile"]);
    assert_eq!(fixture.reload(), before);
    assert_eq!(fixture.counts(), [1, 1, 0]);
}
