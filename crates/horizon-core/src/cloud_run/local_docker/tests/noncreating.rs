use super::*;
use crate::cloud_run::CloudJobId;

pub(super) fn seeded(request: &InteractiveWorkerRequest) -> FakeDocker {
    let fake = FakeDocker::default();
    let name = container_name(request.workflow_id, request.job_id);
    fake.create(&DockerCreateRequest::new(request, &name).expect("fixture"))
        .expect("seed resource without a saved handle");
    fake.state().create_calls = 0;
    fake
}

pub(super) fn assert_read_only(fake: &FakeDocker) {
    let state = fake.state();
    assert_eq!((state.create_calls, state.delete_calls), (0, 0));
}

#[test]
fn absent_or_disappearing_recovery_never_creates_a_worker() {
    let mut request = request();
    request.target.lifetime = WorkerLifetime::Persistent;
    let empty = FakeDocker::default();
    for _ in 0..3 {
        assert_eq!(provider("local", empty.clone()).reconcile_worker(&request), Ok(None));
    }
    assert_read_only(&empty);

    let fake = seeded(&request);
    fake.state().host_key_read_fault = Some(HostKeyReadFault::Disappear);
    assert_eq!(provider("local", fake.clone()).reconcile_worker(&request), Ok(None));
    assert_eq!(provider("local", fake.clone()).reconcile_worker(&request), Ok(None));
    assert_read_only(&fake);
}

#[test]
fn recovery_preserves_identity_across_controllers_and_nonrunning_states() {
    let mut request = request();
    request.target.lifetime = WorkerLifetime::Persistent;
    let fake = seeded(&request);
    let original = provider("local", fake.clone())
        .reconcile_worker(&request)
        .expect("recover")
        .expect("existing");
    assert!(original.is_ready_for(&request, time::OffsetDateTime::now_utc()));
    for (state_name, exit_code, expected) in [
        ("created", 0, InteractiveWorkerLifecycle::Provisioning),
        ("exited", 0, InteractiveWorkerLifecycle::Stopped),
        ("dead", 1, InteractiveWorkerLifecycle::Failed),
        ("unrecognized", 0, InteractiveWorkerLifecycle::Unknown),
    ] {
        {
            let mut state = fake.state();
            let container = state.container.as_mut().expect("resource");
            container.state = state_name.into();
            container.running = false;
            container.exit_code = exit_code;
        }
        let observed = provider("local", fake.clone())
            .reconcile_worker(&request)
            .expect("recover")
            .expect("retained");
        assert_eq!(observed.worker, original.worker);
        assert_eq!(observed.lifecycle, expected);
        assert!(!observed.is_ready_for(&request, time::OffsetDateTime::now_utc()));
        assert_read_only(&fake);
    }
}

#[test]
fn expired_recovery_retains_identity_without_cleanup_or_attachment() {
    let request = request();
    let fake = seeded(&request);
    {
        let mut state = fake.state();
        let container = state.container.as_mut().expect("resource");
        let expired = "2000-01-01T00:00:00Z";
        container.labels.insert(TERMINATE_LABEL.into(), expired.into());
        container
            .environment
            .retain(|entry| !environment_key_matches(entry, TERMINATE_ENV));
        container.environment.push(format!("{TERMINATE_ENV}={expired}"));
    }
    let observed = provider("local", fake.clone())
        .reconcile_worker(&request)
        .expect("read-only expired recovery")
        .expect("retained");
    assert!(!observed.is_ready_for(&request, time::OffsetDateTime::now_utc()));
    assert_read_only(&fake);
    assert!(fake.state().container.is_some());
}

#[test]
fn malformed_requests_and_resource_drift_cannot_authorize_recovery_effects() {
    let mut request = request();
    request.target.lifetime = WorkerLifetime::Persistent;
    let fake = FakeDocker::default();
    for drift in 0..4 {
        let mut invalid = request.clone();
        match drift {
            0 => invalid.target.provider = CloudProvider::RunPod,
            1 => invalid.target.profile = "different".into(),
            2 => invalid.target.image = "mutable:latest".into(),
            _ => invalid.ssh_public_key = "invalid".into(),
        }
        assert_eq!(
            provider("local", fake.clone()).reconcile_worker(&invalid),
            Err(InvalidTarget)
        );
    }
    assert_eq!(fake.state().inspect_calls, 0);
    assert_read_only(&fake);
    for drift in 0..5 {
        let fake = seeded(&request);
        {
            let mut state = fake.state();
            let container = state.container.as_mut().expect("resource");
            match drift {
                0 => {
                    container.labels.insert(JOB_LABEL.into(), CloudJobId::new().to_string());
                }
                1 => container.image = "different:latest".into(),
                2 => container.environment.push(format!("{TERMINATE_ENV}=")),
                3 => {
                    container.labels.insert(TARGET_LABEL.into(), "{}".into());
                }
                _ => container.environment.push(format!("{SSH_PUBLIC_KEY_ENV}=invalid")),
            }
        }
        assert_eq!(
            provider("local", fake.clone()).reconcile_worker(&request),
            Err(ResourceIdentityMismatch)
        );
        assert_read_only(&fake);
        assert!(fake.state().container.is_some());
    }
}
