use super::*;
use crate::cloud_run::interactive_worker_stop::{InteractiveWorkerStop, InteractiveWorkerStopProvider};

#[derive(Clone, Copy, Default)]
pub(super) enum StopBehavior {
    #[default]
    Stops,
    FailsActive,
    FailsAfterStop,
    Disappears,
    ChangesIdentity,
    KeepsRunning,
    UnknownState,
    FailsInspection,
    ChangesRetention,
}

pub(super) fn stop_fake(state: &mut FakeState, resource_id: &str) -> DockerResult<()> {
    state.stop_calls += 1;
    let container = state.container.as_mut().expect("existing worker");
    assert_eq!(resource_id, container.id);
    if matches!(
        state.stop_behavior,
        StopBehavior::FailsActive | StopBehavior::KeepsRunning
    ) {
        return if matches!(state.stop_behavior, StopBehavior::FailsActive) {
            Err(CommandFailed {
                operation: "container stop",
            })
        } else {
            Ok(())
        };
    }
    container.running = false;
    container.state = "exited".into();
    container.exit_code = 137;
    match state.stop_behavior {
        StopBehavior::Stops | StopBehavior::FailsActive | StopBehavior::KeepsRunning => {}
        StopBehavior::FailsAfterStop => {
            return Err(CommandTimedOut {
                operation: "container stop",
            });
        }
        StopBehavior::Disappears => state.container = None,
        StopBehavior::ChangesIdentity => container.name = "foreign-worker".into(),
        StopBehavior::UnknownState => container.state = "unknown-fixture-state".into(),
        StopBehavior::FailsInspection => state.failed_inspections_after_create = 1,
        StopBehavior::ChangesRetention => container.auto_remove = Some(true),
    }
    Ok(())
}

fn fixture() -> (FakeDocker, LocalDockerInteractiveWorkerProvider, InteractiveWorker) {
    let fake = FakeDocker::default();
    let mut request = request();
    request.target.lifetime = WorkerLifetime::Persistent;
    let provider = provider_for("local", fake.clone(), &request);
    let worker = provider
        .ensure_worker(&request)
        .expect("create fixture")
        .into_status()
        .worker;
    {
        let mut state = fake.state();
        state.inspect_calls = 0;
        state.host_key_calls = 0;
        state.create_calls = 0;
    }
    (fake, provider, worker)
}

fn no_mutation(fake: &FakeDocker) {
    let state = fake.state();
    assert_eq!(
        (
            state.stop_calls,
            state.create_calls,
            state.delete_calls,
            state.host_key_calls
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn stop_retains_exact_identity_without_create_delete_ssh_or_repeat_signal() {
    let (fake, provider, worker) = fixture();
    let before = fake.state().container.clone().expect("container");
    assert_eq!(provider.stop_worker(&worker), Ok(InteractiveWorkerStop::Stopped));
    assert_eq!(provider.stop_worker(&worker), Ok(InteractiveWorkerStop::Stopped));
    let state = fake.state();
    let mut expected = before;
    expected.running = false;
    expected.state = "exited".into();
    expected.exit_code = 137;
    assert_eq!(state.container.as_ref(), Some(&expected));
    assert_eq!(
        (
            state.stop_calls,
            state.create_calls,
            state.delete_calls,
            state.host_key_calls
        ),
        (1, 0, 0, 0)
    );
    assert_eq!(state.inspect_calls, 3);
}

#[test]
fn known_active_states_receive_one_exact_stop_and_require_retained_inactive_state() {
    for status in ["running", "paused", "restarting"] {
        let (fake, provider, worker) = fixture();
        fake.state().container.as_mut().expect("container").state = status.into();
        assert_eq!(provider.stop_worker(&worker), Ok(InteractiveWorkerStop::Stopped));
        let state = fake.state();
        assert_eq!(state.stop_calls, 1);
        assert_eq!(state.inspect_calls, 2);
        assert_eq!(
            state.container.as_ref().expect("retained").id,
            worker.identity.resource_id
        );
        assert!(!state.container.as_ref().expect("retained").running);
        assert_eq!(
            (state.create_calls, state.delete_calls, state.host_key_calls),
            (0, 0, 0)
        );
    }
}

#[test]
fn malformed_or_foreign_handles_fail_before_provider_io() {
    let (fake, provider, worker) = fixture();
    let mut changes = vec![worker.clone(); 5];
    changes[0].identity.provider = CloudProvider::RunPod;
    changes[1].target.profile = "different-profile".into();
    changes[2].identity.resource_id = "-unsafe-resource".into();
    changes[3].ssh_public_key = "invalid-private-marker".into();
    changes[4].target.lifetime = WorkerLifetime::TimeLimited { seconds: 30 };
    for changed in changes {
        assert_eq!(provider.stop_worker(&changed), Err(InvalidPersistedWorker));
    }
    assert_eq!(fake.state().inspect_calls, 0);
    no_mutation(&fake);
}

#[test]
fn exact_identity_and_lifetime_mismatches_refuse_to_stop() {
    let (fake, provider, worker) = fixture();
    let before = fake.state().container.clone().expect("container");
    let mut changed = vec![before.clone(); 7];
    changed[0].name = "another-worker".into();
    changed[1].image = "another-image".into();
    changed[2].labels.insert(JOB_LABEL.into(), "another-job".into());
    changed[3].labels.insert(TARGET_LABEL.into(), "{}".into());
    changed[4].environment.push(format!("{SSH_PUBLIC_KEY_ENV}=invalid-key"));
    changed[5].restart_policy = "always".into();
    changed[6]
        .labels
        .insert(TERMINATE_LABEL.into(), "unexpected-deadline".into());
    for container in changed {
        fake.state().container = Some(container);
        assert_eq!(provider.stop_worker(&worker), Err(ResourceIdentityMismatch));
    }
    no_mutation(&fake);
}

#[test]
fn automatic_removal_must_be_explicitly_disabled_even_for_inactive_workers() {
    let (fake, provider, worker) = fixture();
    for auto_remove in [None, Some(true)] {
        for running in [true, false] {
            let mut state = fake.state();
            let container = state.container.as_mut().expect("container");
            container.auto_remove = auto_remove;
            container.running = running;
            container.state = if running { "running" } else { "exited" }.into();
            drop(state);
            assert_eq!(provider.stop_worker(&worker), Err(StopRetentionUnverified));
        }
    }
    no_mutation(&fake);
}

#[test]
fn known_inactive_and_absent_workers_do_not_receive_stop_commands() {
    let (fake, provider, worker) = fixture();
    for status in ["created", "exited"] {
        let mut state = fake.state();
        let container = state.container.as_mut().expect("container");
        container.running = false;
        container.state = status.into();
        drop(state);
        assert_eq!(provider.stop_worker(&worker), Ok(InteractiveWorkerStop::Stopped));
    }
    fake.state().container = None;
    assert_eq!(provider.stop_worker(&worker), Ok(InteractiveWorkerStop::AlreadyAbsent));
    no_mutation(&fake);
}

#[test]
fn deleting_dead_unknown_and_inconsistent_states_fail_before_stop() {
    let (fake, provider, worker) = fixture();
    for (status, running) in [
        ("removing", false),
        ("dead", false),
        ("unknown", true),
        ("running", false),
        ("exited", true),
    ] {
        let mut state = fake.state();
        let container = state.container.as_mut().expect("container");
        container.running = running;
        container.state = status.into();
        drop(state);
        assert_eq!(provider.stop_worker(&worker), Err(StopStateUnverified));
    }
    no_mutation(&fake);
}

#[test]
fn lost_response_is_success_only_after_exact_retained_inactive_verification() {
    let (fake, provider, worker) = fixture();
    fake.state().stop_behavior = StopBehavior::FailsAfterStop;
    assert_eq!(provider.stop_worker(&worker), Ok(InteractiveWorkerStop::Stopped));
    assert_eq!(fake.state().stop_calls, 1);
}

#[test]
fn stop_failures_and_postflight_identity_retention_or_state_drift_fail_closed() {
    for (behavior, error) in [
        (
            StopBehavior::FailsActive,
            CommandFailed {
                operation: "container stop",
            },
        ),
        (StopBehavior::KeepsRunning, StopVerificationFailed),
        (StopBehavior::Disappears, StopResourceLost),
        (StopBehavior::ChangesIdentity, ResourceIdentityMismatch),
        (StopBehavior::UnknownState, StopStateUnverified),
        (StopBehavior::FailsInspection, invalid_response("container inspection")),
        (StopBehavior::ChangesRetention, StopRetentionUnverified),
    ] {
        let (fake, provider, worker) = fixture();
        fake.state().stop_behavior = behavior;
        assert_eq!(provider.stop_worker(&worker), Err(error));
        let state = fake.state();
        assert_eq!(
            (
                state.stop_calls,
                state.create_calls,
                state.delete_calls,
                state.host_key_calls
            ),
            (1, 0, 0, 0)
        );
    }
}
