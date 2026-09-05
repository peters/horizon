use super::*;

fn persistent_request() -> InteractiveWorkerRequest {
    let mut request = request();
    request.target.lifetime = WorkerLifetime::Persistent;
    request
}

#[test]
fn persistent_worker_is_reused_after_controller_drop_without_an_expiry() {
    let request = persistent_request();
    let fake = FakeDocker::default();
    let original = provider("local", fake.clone());
    let created = original.ensure_worker(&request).expect("create persistent worker");
    assert!(matches!(created, InteractiveWorkerEnsure::Created(_)));
    let status = created.into_status();
    assert_eq!(status.worker.lifetime, InteractiveWorkerLifetime::Persistent);
    assert!(status.is_ready_for(&request, time::OffsetDateTime::UNIX_EPOCH + time::Duration::days(3_650)));
    {
        let state = fake.state();
        let container = state.container.as_ref().expect("running container");
        assert!(!container.labels.contains_key(TERMINATE_LABEL));
        assert!(lifetime_matches(container, None));
    }
    let encoded = serde_json::to_string(&status.worker).expect("persist handle");
    let worker: InteractiveWorker = serde_json::from_str(&encoded).expect("restore handle");
    drop(original);
    let reopened = provider("local", fake.clone());
    assert_eq!(reopened.inspect_worker(&worker), Ok(Some(status.clone())));
    assert_eq!(
        reopened.ensure_worker(&request),
        Ok(InteractiveWorkerEnsure::Reused(status))
    );
    {
        let state = fake.state();
        assert_eq!((state.create_calls, state.delete_calls), (1, 0));
    }
    assert_eq!(reopened.delete_worker(&worker), Ok(Deleted));
    assert_eq!(reopened.delete_worker(&worker), Ok(AlreadyAbsent));
}

#[test]
fn a_stopped_or_missing_persistent_worker_is_not_recreated_by_inspection() {
    let request = persistent_request();
    let fake = FakeDocker::default();
    let provider = provider("local", fake.clone());
    let worker = provider.ensure_worker(&request).expect("create").into_status().worker;
    {
        let mut state = fake.state();
        let container = state.container.as_mut().expect("container");
        container.running = false;
        container.state = "exited".into();
    }
    let stopped = provider
        .inspect_worker(&worker)
        .expect("inspect stopped")
        .expect("retained");
    assert_eq!(stopped.lifecycle, InteractiveWorkerLifecycle::Stopped);
    assert!(stopped.ssh.is_none());
    assert_eq!(stopped.worker, worker);
    assert_eq!(
        provider.ensure_worker(&request),
        Ok(InteractiveWorkerEnsure::Reused(stopped))
    );
    fake.state().container = None;
    assert_eq!(provider.inspect_worker(&worker), Ok(None));
    let state = fake.state();
    assert_eq!((state.create_calls, state.delete_calls), (1, 0));
}

#[test]
fn persistent_metadata_drift_never_authorizes_reuse_or_deletion() {
    for drift in 0..6 {
        let request = persistent_request();
        let fake = FakeDocker::default();
        let provider = provider("local", fake.clone());
        let worker = provider.ensure_worker(&request).expect("create").into_status().worker;
        {
            let mut state = fake.state();
            let container = state.container.as_mut().expect("container");
            match drift {
                0 => container.environment.push(format!("{TERMINATE_ENV}=")),
                1 => container
                    .environment
                    .push(format!("{TERMINATE_ENV}=2020-01-01T00:00:00Z")),
                2 => container.environment.push(TERMINATE_ENV.into()),
                3 => {
                    container.labels.insert(TERMINATE_LABEL.into(), String::new());
                }
                4 => {
                    container
                        .labels
                        .insert(TERMINATE_LABEL.into(), "2020-01-01T00:00:00Z".into());
                }
                _ => {
                    container.labels.insert(
                        TARGET_LABEL.into(),
                        serde_json::to_string(&super::request().target).expect("time-limited target"),
                    );
                }
            }
        }
        assert_eq!(provider.ensure_worker(&request), Err(ResourceIdentityMismatch));
        assert_eq!(provider.inspect_worker(&worker), Err(ResourceIdentityMismatch));
        assert_eq!(provider.delete_worker(&worker), Err(ResourceIdentityMismatch));
        let state = fake.state();
        assert_eq!((state.create_calls, state.delete_calls), (1, 0));
        assert!(state.container.is_some());
    }
}

#[test]
fn a_timed_worker_is_never_adopted_as_persistent_or_vice_versa() {
    for initial in [request(), persistent_request()] {
        let fake = FakeDocker::default();
        let provider = provider("local", fake.clone());
        provider.ensure_worker(&initial).expect("create original policy");
        let mut changed = initial;
        changed.target.lifetime = match changed.target.lifetime {
            WorkerLifetime::Persistent => WorkerLifetime::TimeLimited { seconds: 900 },
            WorkerLifetime::TimeLimited { .. } => WorkerLifetime::Persistent,
        };
        assert_eq!(provider.ensure_worker(&changed), Err(ResourceIdentityMismatch));
        let state = fake.state();
        assert_eq!((state.create_calls, state.delete_calls), (1, 0));
    }
}

#[test]
fn persistent_lost_create_response_recovers_the_same_exact_worker() {
    let fake = FakeDocker::default();
    fake.state().fail_create_after_insert = true;
    let provider = provider("local", fake.clone());
    let result = provider
        .ensure_worker(&persistent_request())
        .expect("reconcile creation");
    assert!(matches!(result, InteractiveWorkerEnsure::Reused(_)));
    assert_eq!(result.status().worker.lifetime, InteractiveWorkerLifetime::Persistent);
    let state = fake.state();
    assert_eq!((state.create_calls, state.delete_calls), (1, 0));
}

#[test]
fn unexpected_image_expiry_retains_the_created_identity_for_manual_inspection() {
    for entry in [
        TERMINATE_ENV.to_string(),
        format!("{TERMINATE_ENV}="),
        format!("{TERMINATE_ENV}=2020-01-01T00:00:00Z"),
    ] {
        let fake = FakeDocker::default();
        fake.state().image_environment.push(entry);
        let result = provider("local", fake.clone()).ensure_worker(&persistent_request());
        assert_eq!(
            result,
            Err(PersistentLifetimeMetadataConflict {
                resource_id: "a".repeat(64)
            })
        );
        let state = fake.state();
        assert_eq!((state.create_calls, state.delete_calls), (1, 0));
        assert_eq!(
            state.container.as_ref().expect("retained for manual inspection").id,
            "a".repeat(64)
        );
    }
}

#[test]
fn timed_lifetime_rejects_bare_or_duplicate_expiry_without_deletion() {
    for bare in [true, false] {
        let fake = FakeDocker::default();
        let provider = provider("local", fake.clone());
        let request = request();
        let worker = provider
            .ensure_worker(&request)
            .expect("create timed worker")
            .into_status()
            .worker;
        {
            let mut state = fake.state();
            let container = state.container.as_mut().expect("container");
            let extra = if bare {
                TERMINATE_ENV.to_string()
            } else {
                format!(
                    "{TERMINATE_ENV}={}",
                    required_environment(container, TERMINATE_ENV).expect("original expiry")
                )
            };
            container.environment.push(extra);
        }
        assert_eq!(provider.ensure_worker(&request), Err(ResourceIdentityMismatch));
        assert_eq!(provider.inspect_worker(&worker), Err(ResourceIdentityMismatch));
        assert_eq!(provider.delete_worker(&worker), Err(ResourceIdentityMismatch));
        assert_eq!(fake.state().delete_calls, 0);
    }
}

#[test]
fn a_persistent_create_race_never_deletes_the_recovered_peers_worker() {
    let request = persistent_request();
    let fake = FakeDocker::default();
    let provider = provider("local", fake.clone());
    let worker = provider
        .ensure_worker(&request)
        .expect("peer creates worker")
        .into_status()
        .worker;
    {
        let mut state = fake.state();
        state.hidden_inspections_after_create = 1;
        state.reject_create = true;
        state.container.as_mut().expect("peer worker").ssh_bindings[0].host = "0.0.0.0".into();
    }
    assert_eq!(
        provider.ensure_worker(&request),
        Err(PersistentCreationReconciliationRequired {
            resource_id: worker.identity.resource_id.clone()
        })
    );
    let state = fake.state();
    assert_eq!((state.create_calls, state.delete_calls), (2, 0));
    assert_eq!(
        state.container.as_ref().expect("peer worker retained").id,
        worker.identity.resource_id
    );
}
