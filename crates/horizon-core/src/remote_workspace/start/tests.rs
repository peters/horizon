mod configured_azure;

use super::*;
use crate::{
    cloud_run::{
        CloudProvider,
        interactive_worker::{
            InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerIdentity,
            InteractiveWorkerLifetime, InteractiveWorkerProvider, InteractiveWorkerRequest,
            InteractiveWorkerSshEndpoint, InteractiveWorkerStatus,
        },
        interactive_worker_stop::{
            InteractiveWorkerStop, InteractiveWorkerStopExpectation, InteractiveWorkerStopObservation,
            InteractiveWorkerStopObserver, InteractiveWorkerStopProvider,
        },
    },
    remote_workspace::{
        RemoteCleanupIntent, RemoteCleanupReason, RemoteWorkspaceState,
        stop::{RemoteWorkspaceStopError, confirm_remote_workspace_stop, stop_remote_workspace},
    },
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::Mutex;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";
type Error = RemoteWorkspaceStartError;

fn key(byte: u8) -> String {
    let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
    blob.extend([byte; 32]);
    format!("ssh-ed25519 {}", STANDARD.encode(blob))
}

fn pin() -> InteractiveWorkerSshEndpoint {
    InteractiveWorkerSshEndpoint {
        host: "203.0.113.9".into(),
        port: 2222,
        username: "root".into(),
        host_key: key(9),
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    store: CloudWorkflowStore,
}

impl Fixture {
    /// A retained persistent worker with an optional complete pin, driven to a saved
    /// Stopped record through the real Stop coordinator.
    fn stopped(pinned: bool) -> Self {
        let fixture = Self::retained(pinned);
        stop_remote_workspace(&fixture.store, &Provider::stopping(), fixture.current().workspace()).expect("stopped");
        assert!(matches!(fixture.phase(), RemoteRuntimePhase::Stopped { .. }));
        fixture
    }

    fn retained(pinned: bool) -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        let store = CloudWorkflowStore::open_path(directory.path().join("control/workflows.sqlite3")).expect("store");
        let state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version":1, "spec":{
                "workspace_local_id":"workspace", "working_directory":".", "generation":0, "panels":[],
                "target":{"provider":"local_docker", "profile":"development", "disk_gib":20,
                    "lifetime":"persistent", "image":format!("example/worker@sha256:{}", "a".repeat(64))},
                "repository":{"repository":"example/project", "commit":"b".repeat(40)}
            }
        }))
        .expect("state");
        let saved = store.create_remote_workspace(OWNER, &state).expect("workspace");
        let allocation = store.allocate_remote_runtime(&saved, i64::MAX).expect("allocation");
        let reserved = store
            .reserve_remote_worker_request(&allocation, &key(7))
            .expect("public request");
        let request = reserved.worker_request().expect("request");
        let status = InteractiveWorkerStatus {
            worker: worker_for(&request),
            lifecycle: if pinned {
                InteractiveWorkerLifecycle::Ready
            } else {
                InteractiveWorkerLifecycle::Provisioning
            },
            ssh: pinned.then(pin),
        };
        store
            .record_remote_worker_recovery(&reserved, Some(&status))
            .expect("retained public observation");
        Self { directory, store }
    }

    fn current(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation")
    }

    fn phase(&self) -> RemoteRuntimePhase {
        self.current()
            .workspace()
            .state()
            .runtime
            .as_ref()
            .expect("runtime")
            .phase
    }

    fn start(&self, provider: &Provider) -> Result<RemoteWorkspaceStart, Error> {
        start_remote_workspace(&self.store, provider, &self.current())
    }
}

fn worker_for(request: &InteractiveWorkerRequest) -> InteractiveWorker {
    InteractiveWorker {
        identity: InteractiveWorkerIdentity {
            provider: CloudProvider::LocalDocker,
            workflow_id: request.workflow_id,
            job_id: request.job_id,
            resource_id: "a".repeat(64),
        },
        target: request.target.clone(),
        ssh_public_key: request.ssh_public_key.clone(),
        lifetime: InteractiveWorkerLifetime::Persistent,
    }
}

type StartHook = Box<dyn Fn(&InteractiveWorker) + Send + Sync>;
type StartScript = Box<dyn Fn(&InteractiveWorker) -> Result<InteractiveWorkerStart, &'static str> + Send + Sync>;

/// Answers a start with a scripted result built from the worker it was asked about.
struct Provider {
    kind: CloudProvider,
    start: Option<StartScript>,
    stop: Result<InteractiveWorkerStop, &'static str>,
    calls: Mutex<[usize; 2]>,
    on_start: Option<StartHook>,
}

impl Provider {
    fn stopping() -> Self {
        Self {
            kind: CloudProvider::LocalDocker,
            start: None,
            stop: Ok(InteractiveWorkerStop::Stopped),
            calls: Mutex::new([0; 2]),
            on_start: None,
        }
    }

    fn starting(
        start: impl Fn(&InteractiveWorker) -> Result<InteractiveWorkerStart, &'static str> + Send + Sync + 'static,
    ) -> Self {
        Self {
            kind: CloudProvider::LocalDocker,
            start: Some(Box::new(start)),
            stop: Err("no stop"),
            calls: Mutex::new([0; 2]),
            on_start: None,
        }
    }

    fn started(
        worker: &InteractiveWorker,
        lifecycle: InteractiveWorkerLifecycle,
        ssh: Option<InteractiveWorkerSshEndpoint>,
    ) -> InteractiveWorkerStart {
        InteractiveWorkerStart::Started(InteractiveWorkerStatus {
            worker: worker.clone(),
            lifecycle,
            ssh,
        })
    }

    fn counts(&self) -> [usize; 2] {
        *self.calls.lock().expect("counts")
    }
}

impl InteractiveWorkerProvider for Provider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        self.kind
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("no creation")
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("no generic inspection")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("no setup recovery")
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("no deletion")
    }
}

impl InteractiveWorkerStopProvider for Provider {
    fn stop_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerStop, Self::Error> {
        self.calls.lock().expect("calls")[1] += 1;
        self.stop.map_err(std::io::Error::other)
    }
}

impl InteractiveWorkerStopObserver for Provider {
    fn observe_worker_stop(
        &self,
        _: InteractiveWorkerStopExpectation<'_>,
    ) -> Result<InteractiveWorkerStopObservation, Self::Error> {
        panic!("no observation reaches a record without Stop intent")
    }
}

impl InteractiveWorkerStartProvider for Provider {
    fn start_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerStart, Self::Error> {
        self.calls.lock().expect("calls")[0] += 1;
        if let Some(hook) = &self.on_start {
            hook(worker);
        }
        let start = self.start.as_ref().expect("a start was scripted");
        start(worker).map_err(std::io::Error::other)
    }
}

#[test]
fn start_records_intent_before_dispatch_then_reconciling_with_the_same_identity() {
    for (already_running, lifecycle) in [
        (false, InteractiveWorkerLifecycle::Ready),
        (false, InteractiveWorkerLifecycle::Provisioning),
        (true, InteractiveWorkerLifecycle::Ready),
    ] {
        let fixture = Fixture::stopped(true);
        let before = fixture.current();
        let store = fixture.store.clone();
        let mut provider = Provider::starting(move |worker| {
            let status = InteractiveWorkerStatus {
                worker: worker.clone(),
                lifecycle,
                ssh: (lifecycle == InteractiveWorkerLifecycle::Ready).then(pin),
            };
            Ok(if already_running {
                InteractiveWorkerStart::AlreadyRunning(status)
            } else {
                InteractiveWorkerStart::Started(status)
            })
        });
        provider.on_start = Some(Box::new(move |worker| {
            let current = store
                .load_remote_allocation(OWNER, "workspace")
                .expect("read")
                .expect("allocation");
            let runtime = current.workspace().state().runtime.as_ref().expect("runtime");
            assert!(
                matches!(runtime.phase, RemoteRuntimePhase::Starting { .. }),
                "intent is durable before the provider is asked"
            );
            assert_eq!(runtime.worker.as_ref(), Some(worker));
        }));
        let started = fixture.start(&provider).expect("verified start");
        let after = fixture.current();
        assert_eq!(started.allocation, after);
        assert_eq!(
            (started.lifecycle, started.already_running),
            (lifecycle, already_running)
        );
        assert_eq!(fixture.phase(), RemoteRuntimePhase::Reconciling);
        let mut permitted = before.workspace().state().clone();
        permitted.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Reconciling;
        assert_eq!(after.workspace().state(), &permitted, "only the phase changed");
        assert_eq!(after.workflow(), before.workflow());
        assert_eq!(after.workspace().revision(), before.workspace().revision() + 2);
        assert_eq!(provider.counts(), [1, 0]);
        // A record without a saved Stop is never started again.
        assert_eq!(fixture.start(&provider), Err(Error::NotStopped));
        assert_eq!(fixture.current(), after);
        assert_eq!(provider.counts(), [1, 0]);
    }
}

#[test]
fn uncertainty_absence_and_foreign_observations_retain_start_intent_for_an_explicit_retry() {
    let moved_pin = || InteractiveWorkerSshEndpoint {
        host: "203.0.113.10".into(),
        ..pin()
    };
    let rekeyed_pin = || InteractiveWorkerSshEndpoint {
        host_key: key(11),
        ..pin()
    };
    let outcomes: [(&str, Provider, Error); 5] = [
        (
            "provider failure",
            Provider::starting(|_| Err("private-provider-marker")),
            Error::ProviderUnavailable,
        ),
        (
            "absence",
            Provider::starting(|_| Ok(InteractiveWorkerStart::AlreadyAbsent)),
            Error::ResourceAbsent,
        ),
        (
            "another worker",
            Provider::starting(|worker| {
                let mut other = worker.clone();
                other.identity.resource_id = "b".repeat(64);
                Ok(Provider::started(
                    &other,
                    InteractiveWorkerLifecycle::Ready,
                    Some(pin()),
                ))
            }),
            Error::IdentityMismatch,
        ),
        (
            "moved address",
            Provider::starting(move |worker| {
                Ok(Provider::started(
                    worker,
                    InteractiveWorkerLifecycle::Ready,
                    Some(moved_pin()),
                ))
            }),
            Error::IdentityMismatch,
        ),
        (
            "replacement host key",
            Provider::starting(move |worker| {
                Ok(Provider::started(
                    worker,
                    InteractiveWorkerLifecycle::Ready,
                    Some(rekeyed_pin()),
                ))
            }),
            Error::IdentityMismatch,
        ),
    ];
    for (label, provider, expected) in outcomes {
        let fixture = Fixture::stopped(true);
        let before = fixture.current();
        let error = fixture.start(&provider).expect_err(label);
        assert_eq!(error, expected, "{label}");
        assert!(!format!("{error:?} {error}").contains("private-provider-marker"));
        let retained = fixture.current();
        let RemoteRuntimePhase::Starting { requested_at_millis } = fixture.phase() else {
            panic!("{label}: intent is retained: {:?}", fixture.phase());
        };
        let mut permitted = before.workspace().state().clone();
        permitted.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Starting { requested_at_millis };
        assert_eq!(
            retained.workspace().state(),
            &permitted,
            "{label}: identity and pin unchanged"
        );
        assert_eq!(retained.workspace().revision(), before.workspace().revision() + 1);
        assert_eq!(provider.counts(), [1, 0]);
        // An explicit retry from a fresh client reuses the original request and posts
        // nothing that already runs.
        let reopened =
            CloudWorkflowStore::open_existing_without_migration_path(fixture.store.path()).expect("fresh client");
        let retry = Provider::starting(|worker| {
            Ok(InteractiveWorkerStart::AlreadyRunning(InteractiveWorkerStatus {
                worker: worker.clone(),
                lifecycle: InteractiveWorkerLifecycle::Ready,
                ssh: Some(pin()),
            }))
        });
        let started = start_remote_workspace(&reopened, &retry, &retained).expect("retry");
        assert!(started.already_running);
        assert_eq!(fixture.phase(), RemoteRuntimePhase::Reconciling);
        assert_eq!(
            started.allocation.workspace().revision(),
            retained.workspace().revision() + 1
        );
        assert_eq!(
            fixture.directory.path().read_dir().expect("fixture root").count(),
            1,
            "only the control store"
        );
    }
}

#[test]
fn only_a_saved_stop_with_a_retained_pinned_worker_is_started_and_admission_precedes_dispatch() {
    let never = || Provider::starting(|_| panic!("no provider work"));
    // No saved Stop: a retained running record and a Stop still in flight.
    let fixture = Fixture::retained(true);
    assert_eq!(fixture.start(&never()), Err(Error::NotStopped));
    let fixture = Fixture::retained(true);
    let mut uncertain = Provider::stopping();
    uncertain.stop = Err("uncertain");
    assert_eq!(
        stop_remote_workspace(&fixture.store, &uncertain, fixture.current().workspace()),
        Err(RemoteWorkspaceStopError::ProviderUnavailable)
    );
    assert!(matches!(fixture.phase(), RemoteRuntimePhase::Stopping { .. }));
    assert_eq!(fixture.start(&never()), Err(Error::NotStopped));
    // No complete pin: the saved Stop exists, but there is no trust to compare against.
    let fixture = Fixture::stopped(false);
    assert_eq!(fixture.start(&never()), Err(Error::MissingTrust));
    // Wrong provider, stale snapshot and a foreign owner.
    let fixture = Fixture::stopped(true);
    let before = fixture.current();
    let mut foreign = never();
    foreign.kind = CloudProvider::RunPod;
    assert_eq!(fixture.start(&foreign), Err(Error::ProviderMismatch));
    let mut workflow = before.workflow().workflow().clone();
    workflow.updated_at_millis += 1;
    fixture
        .store
        .replace(before.workflow(), &workflow)
        .expect("workflow drift");
    assert_eq!(
        start_remote_workspace(&fixture.store, &never(), &before),
        Err(Error::StateChanged)
    );
    assert!(
        matches!(fixture.phase(), RemoteRuntimePhase::Stopped { .. }),
        "no intent was recorded by any refusal"
    );
}

#[test]
fn generic_writes_stop_recovery_and_checks_cannot_touch_start_intent() {
    let fixture = Fixture::stopped(true);
    let stopped = fixture.current();
    let observed_at = match fixture.phase() {
        RemoteRuntimePhase::Stopped { observed_at_millis, .. } => observed_at_millis,
        other => panic!("{other:?}"),
    };
    // A generic writer cannot introduce Start intent over a saved Stop.
    let mut next = stopped.workspace().state().clone();
    next.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Starting {
        requested_at_millis: observed_at,
    };
    assert!(matches!(
        fixture.store.replace_remote_workspace(stopped.workspace(), &next),
        Err(RemoteWorkspaceStoreError::RuntimeStartCoordinationRequired)
    ));
    // Nor may the coordinator's writer rewind before the retention observation.
    assert_eq!(
        fixture
            .store
            .record_remote_start_phase(
                &stopped,
                RemoteRuntimePhase::Starting {
                    requested_at_millis: observed_at - 1
                }
            )
            .err(),
        Some(Error::NotStopped)
    );
    assert_eq!(fixture.current(), stopped);
    // With intent recorded, generic writes cannot erase, resolve or retarget it, Stop
    // refuses, recovery refuses, and the saved-Stop check sees no Stop intent.
    let _ = fixture.start(&Provider::starting(|_| Err("uncertain")));
    let starting = fixture.current();
    let RemoteRuntimePhase::Starting { requested_at_millis } = fixture.phase() else {
        panic!("intent");
    };
    for replacement in [
        None,
        Some(RemoteRuntimePhase::Reconciling),
        Some(RemoteRuntimePhase::Ready),
        Some(RemoteRuntimePhase::Stopping {
            requested_at_millis: requested_at_millis + 1,
        }),
        Some(RemoteRuntimePhase::Starting {
            requested_at_millis: requested_at_millis + 1,
        }),
        Some(RemoteRuntimePhase::Stopped {
            requested_at_millis,
            observed_at_millis: requested_at_millis,
        }),
    ] {
        let mut next = starting.workspace().state().clone();
        if let Some(phase) = replacement {
            next.runtime.as_mut().expect("runtime").phase = phase;
        } else {
            next.runtime = None;
        }
        assert!(
            fixture
                .store
                .replace_remote_workspace(starting.workspace(), &next)
                .is_err(),
            "{replacement:?}"
        );
        assert_eq!(fixture.current(), starting);
    }
    assert_eq!(
        stop_remote_workspace(&fixture.store, &Provider::stopping(), starting.workspace()),
        Err(RemoteWorkspaceStopError::ManagementConflict)
    );
    assert_eq!(
        confirm_remote_workspace_stop(&fixture.store, &Provider::stopping(), &starting),
        Err(RemoteWorkspaceStopError::MissingStopIntent)
    );
    assert!(matches!(
        fixture.store.record_remote_worker_recovery(&starting, None),
        Err(RemoteWorkspaceStoreError::RuntimeRecoveryUnavailable)
    ));
    assert_eq!(fixture.current(), starting);
    // Management intent cannot be added over Start intent either.
    let mut next = starting.workspace().state().clone();
    next.runtime.as_mut().expect("runtime").cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::Cancelled,
        requested_at_millis: 1,
    });
    assert!(
        fixture
            .store
            .replace_remote_workspace(starting.workspace(), &next)
            .is_err()
    );
    assert_eq!(fixture.current(), starting);
}

#[test]
fn start_phase_serialization_round_trips_and_rejects_malformed_intent() {
    let fixture = Fixture::stopped(true);
    let mut state = fixture.current().workspace().state().clone();
    state.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Starting { requested_at_millis: 1 };
    let json = serde_json::to_value(&state).expect("encode");
    assert_eq!(
        json["runtime"]["phase"],
        serde_json::json!({"starting":{"requested_at_millis":1}})
    );
    assert_eq!(
        serde_json::from_value::<RemoteWorkspaceState>(json.clone()).expect("round trip"),
        state
    );
    for phase in [
        serde_json::json!({"starting":{"requested_at_millis":-1}}),
        serde_json::json!({"starting":{}}),
        serde_json::json!({"starting":{"requested_at_millis":1,"unexpected":"private-marker"}}),
    ] {
        let mut encoded = json.clone();
        encoded["runtime"]["phase"] = phase;
        assert!(serde_json::from_value::<RemoteWorkspaceState>(encoded).is_err());
    }
    state.runtime.as_mut().expect("runtime").cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::Cancelled,
        requested_at_millis: 1,
    });
    assert!(state.validate().is_err(), "Start intent excludes management intent");
    state.runtime.as_mut().expect("runtime").cleanup = None;
    state.runtime.as_mut().expect("runtime").ssh = None;
    assert!(state.validate().is_err(), "Start intent requires the saved pin");
    state.runtime.as_mut().expect("runtime").ssh = Some(pin());
    state.runtime.as_mut().expect("runtime").worker = None;
    assert!(state.validate().is_err(), "Start intent requires the retained worker");
}
