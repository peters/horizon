use super::*;
use crate::cloud_run::interactive_worker::InteractiveWorkerSshEndpoint;
use crate::cloud_run::interactive_worker_stop::{
    InteractiveWorkerStopExpectation, InteractiveWorkerStopObservation as Observation, InteractiveWorkerStopObserver,
};

struct Observer {
    kind: CloudProvider,
    result: Result<Observation, &'static str>,
    calls: Mutex<usize>,
    hook: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Observer {
    fn new(result: Result<Observation, &'static str>) -> Self {
        Self {
            kind: CloudProvider::LocalDocker,
            result,
            calls: Mutex::new(0),
            hook: None,
        }
    }
    fn calls(&self) -> usize {
        *self.calls.lock().expect("calls")
    }
}

impl InteractiveWorkerProvider for Observer {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        self.kind
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("confirmation must not create")
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("generic lifecycle is not retained Stop proof")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("confirmation must not reconcile creation")
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("confirmation must not delete")
    }
}

impl InteractiveWorkerStopObserver for Observer {
    fn observe_worker_stop(&self, expected: InteractiveWorkerStopExpectation<'_>) -> Result<Observation, Self::Error> {
        assert!(expected.ssh.is_complete());
        assert!(expected.worker.is_valid_for(self.kind));
        assert!(expected.network_volume.is_none());
        *self.calls.lock().expect("calls") += 1;
        if let Some(hook) = &self.hook {
            hook();
        }
        self.result.map_err(std::io::Error::other)
    }
}

fn retain_pin(fixture: &Fixture) {
    let current = fixture.current();
    let mut next = current.workspace().state().clone();
    let runtime = next.runtime.as_mut().expect("runtime");
    runtime.ssh = Some(InteractiveWorkerSshEndpoint {
        host: "worker.example".into(),
        port: 2222,
        username: "root".into(),
        host_key: runtime.worker.as_ref().expect("worker").ssh_public_key.clone(),
    });
    fixture
        .store
        .replace_remote_workspace(current.workspace(), &next)
        .expect("retained public pin");
}

fn intent(fixture: &Fixture) -> StoredRemoteAllocation {
    fixture
        .store
        .record_remote_stop_phase(
            &fixture.current(),
            RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
        )
        .expect("existing synthetic intent, no provider Stop")
}

#[test]
fn pending_absence_and_errors_do_not_write_or_consume_existing_intent() {
    let fixture = Fixture::new();
    retain_pin(&fixture);
    let pending = intent(&fixture);
    for outcome in [
        Ok(Observation::Pending),
        Ok(Observation::Absent),
        Err("private-provider-marker"),
    ] {
        let observer = Observer::new(outcome);
        let result = confirm_remote_workspace_stop(&fixture.store, &observer, &pending);
        if let Ok(observation) = outcome {
            let result = result.expect("point-in-time observation");
            assert_eq!(result.observation, observation);
            assert_eq!(result.allocation, pending);
        } else {
            let error = result.expect_err("unverified");
            assert_eq!(error, Error::ProviderUnavailable);
            assert!(!format!("{error:?} {error}").contains("private-provider-marker"));
        }
        assert_eq!(fixture.current(), pending);
        assert_eq!(observer.calls(), 1);
    }
}

#[test]
fn delayed_confirmation_preserves_identity_and_original_times_across_fresh_store() {
    let fixture = Fixture::new();
    retain_pin(&fixture);
    let pending = intent(&fixture);
    let observer = Observer::new(Ok(Observation::RetainedStopped));
    let reopened = CloudWorkflowStore::open_path(fixture.store.path()).expect("fresh controller");
    let completed = confirm_remote_workspace_stop(&reopened, &observer, &pending).expect("confirmed");
    let mut expected = pending.workspace().state().clone();
    let phase = fixture.phase();
    assert!(
        matches!(phase, RemoteRuntimePhase::Stopped { requested_at_millis: 1, observed_at_millis } if observed_at_millis >= 1)
    );
    expected.runtime.as_mut().expect("runtime").phase = phase;
    assert_eq!(completed.allocation.workspace().state(), &expected);
    assert_eq!(completed.allocation.workflow(), pending.workflow());
    assert_eq!(
        completed.allocation.workspace().revision(),
        pending.workspace().revision() + 1
    );
    for outcome in [Observation::RetainedStopped, Observation::Pending, Observation::Absent] {
        let result = confirm_remote_workspace_stop(&reopened, &Observer::new(Ok(outcome)), &completed.allocation)
            .expect("already-stopped observation");
        assert_eq!(result.allocation, completed.allocation);
        assert_eq!(result.observation, outcome);
        assert_eq!(fixture.current(), completed.allocation);
    }
}

#[test]
fn missing_intent_pin_wrong_provider_and_future_request_refuse_before_observation() {
    let fixture = Fixture::new();
    let observer = Observer::new(Ok(Observation::RetainedStopped));
    let untouched = fixture.current();
    assert_eq!(
        confirm_remote_workspace_stop(&fixture.store, &observer, &untouched),
        Err(Error::MissingStopIntent)
    );
    assert_eq!(fixture.current(), untouched);
    let pending = intent(&fixture);
    assert_eq!(
        confirm_remote_workspace_stop(&fixture.store, &observer, &pending),
        Err(Error::MissingTrust)
    );
    assert_eq!(fixture.current(), pending);
    retain_pin(&fixture);
    let mut wrong = Observer::new(Ok(Observation::RetainedStopped));
    wrong.kind = CloudProvider::RunPod;
    assert_eq!(
        confirm_remote_workspace_stop(&fixture.store, &wrong, &fixture.current()),
        Err(Error::ProviderMismatch)
    );
    let future = Fixture::new();
    retain_pin(&future);
    let future_request = future
        .store
        .record_remote_stop_phase(
            &future.current(),
            RemoteRuntimePhase::Stopping {
                requested_at_millis: i64::MAX,
            },
        )
        .expect("future request");
    assert_eq!(
        confirm_remote_workspace_stop(&future.store, &observer, &future_request),
        Err(Error::InvalidTimestamp)
    );
    assert_eq!(future.current(), future_request);
    assert_eq!(observer.calls() + wrong.calls(), 0);
}

fn drift(store: &CloudWorkflowStore, workflow: bool) {
    let current = store
        .load_remote_allocation(OWNER, "workspace")
        .expect("read")
        .expect("allocation");
    if workflow {
        let mut next = current.workflow().workflow().clone();
        next.updated_at_millis += 1;
        store.replace(current.workflow(), &next).expect("workflow drift");
    } else {
        let mut next = current.workspace().state().clone();
        next.spec.panels.clear();
        store
            .replace_remote_workspace(current.workspace(), &next)
            .expect("workspace drift");
    }
}

#[test]
fn full_snapshot_drift_is_fenced_before_and_after_each_observation_outcome() {
    for workflow in [false, true] {
        for before_call in [false, true] {
            for outcome in [
                Ok(Observation::RetainedStopped),
                Ok(Observation::Pending),
                Ok(Observation::Absent),
                Err("lost read"),
            ] {
                let fixture = Fixture::new();
                retain_pin(&fixture);
                let expected = intent(&fixture);
                let mut observer = Observer::new(outcome);
                if before_call {
                    drift(&fixture.store, workflow);
                } else {
                    let store = fixture.store.clone();
                    observer.hook = Some(Box::new(move || drift(&store, workflow)));
                }
                assert_eq!(
                    confirm_remote_workspace_stop(&fixture.store, &observer, &expected),
                    Err(Error::StateChanged)
                );
                assert_eq!(fixture.phase(), RemoteRuntimePhase::Stopping { requested_at_millis: 1 });
                assert_eq!(observer.calls(), usize::from(!before_call));
            }
        }
    }
}

#[test]
fn time_limited_intent_cannot_be_promoted_to_retained_stop() {
    let fixture = Fixture::with_lifetime(InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
        terminate_after: (time::OffsetDateTime::now_utc() + time::Duration::minutes(15))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("lease"),
    }));
    retain_pin(&fixture);
    let expected = intent(&fixture);
    let observer = Observer::new(Ok(Observation::RetainedStopped));
    assert_eq!(
        confirm_remote_workspace_stop(&fixture.store, &observer, &expected),
        Err(Error::UnsupportedLifetime)
    );
    assert_eq!(fixture.current(), expected);
    assert_eq!(observer.calls(), 0);
}
