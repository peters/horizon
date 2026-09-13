mod configured;
mod configured_azure;
mod configured_confirmation;
mod configured_runpod;
mod confirmation;

use super::*;
use crate::{
    HorizonHome,
    cloud_run::{
        ArtifactDigest, CloudProvider, WorkerLifetime,
        interactive_worker::{
            InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerIdentity,
            InteractiveWorkerLease, InteractiveWorkerLifetime, InteractiveWorkerProvider, InteractiveWorkerRequest,
            InteractiveWorkerStatus,
        },
    },
    remote_environment_observation::observe_remote_environment,
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason, RemoteWorkspaceState, RepositoryCheckpoint},
    remote_workspace_recovery::{RemoteWorkspaceRecoveryError, recover_remote_workspace},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::Mutex;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";
type Error = RemoteWorkspaceStopError;

struct Fixture {
    directory: tempfile::TempDir,
    store: CloudWorkflowStore,
}

impl Fixture {
    fn new() -> Self {
        Self::with_lifetime(InteractiveWorkerLifetime::Persistent)
    }

    fn with_lifetime(lifetime: InteractiveWorkerLifetime) -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        let store = CloudWorkflowStore::open_path(directory.path().join("control/workflows.sqlite3")).expect("store");
        let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version":1, "spec":{
                "workspace_local_id":"workspace", "working_directory":".", "generation":0,
                "target":{"provider":"local_docker", "profile":"development", "disk_gib":20,
                    "lifetime":"persistent", "image":format!("example/worker@sha256:{}", "a".repeat(64))},
                "repository":{"repository":"example/project", "commit":"b".repeat(40)},
                "panels":[{"panel_local_id":"terminal", "kind":"command", "task_handoff":"private-task-marker",
                    "command":{"program":"printf", "args":["literal $()", "", "æøå"]}}]
            }
        }))
        .expect("state");
        if matches!(lifetime, InteractiveWorkerLifetime::TimeLimited(_)) {
            state.spec.target.lifetime = WorkerLifetime::TimeLimited { seconds: 900 };
        }
        let dormant = store.create_remote_workspace(OWNER, &state).expect("dormant");
        let allocation = store.allocate_remote_runtime(&dormant, i64::MAX).expect("allocate");
        let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
        blob.extend([7; 32]);
        let key = format!("ssh-ed25519 {}", STANDARD.encode(blob));
        let reserved = store.reserve_remote_worker_request(&allocation, &key).expect("reserve");
        let request = reserved.worker_request().expect("request");
        let mut next = reserved.workspace().state().clone();
        let runtime = next.runtime.as_mut().expect("runtime");
        runtime.phase = RemoteRuntimePhase::Reconciling;
        runtime.worker = Some(InteractiveWorker {
            identity: InteractiveWorkerIdentity {
                provider: request.target.provider,
                workflow_id: request.workflow_id,
                job_id: request.job_id,
                resource_id: "a".repeat(64),
            },
            target: request.target,
            ssh_public_key: request.ssh_public_key,
            lifetime,
        });
        next.checkpoint = Some(RepositoryCheckpoint {
            workspace_local_id: next.spec.workspace_local_id.clone(),
            base_commit: next.spec.repository.commit.clone(),
            manifest_digest: ArtifactDigest::parse_sha256("c".repeat(64)).expect("digest"),
            runtime_generation: 1,
            generation: 1,
            captured_at_millis: 1,
            recovery_artifact: Some(ArtifactDigest::parse_sha256("d".repeat(64)).expect("artifact")),
        });
        store
            .replace_remote_workspace(reserved.workspace(), &next)
            .expect("observed fixture");
        Self { directory, store }
    }

    fn current(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation")
    }

    fn stop(&self, provider: &Provider) -> Result<StoredRemoteAllocation, Error> {
        stop_remote_workspace(&self.store, provider, self.current().workspace())
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
}

type StopHook = Box<dyn Fn(&InteractiveWorker) + Send + Sync>;

struct Provider {
    kind: CloudProvider,
    result: Result<InteractiveWorkerStop, &'static str>,
    calls: Mutex<[usize; 5]>,
    on_stop: Option<StopHook>,
}

impl Provider {
    fn new(result: Result<InteractiveWorkerStop, &'static str>) -> Self {
        Self {
            kind: CloudProvider::LocalDocker,
            result,
            calls: Mutex::new([0; 5]),
            on_stop: None,
        }
    }

    fn counts(&self) -> [usize; 5] {
        *self.calls.lock().expect("counts")
    }
}

impl InteractiveWorkerProvider for Provider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        self.kind
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        self.calls.lock().expect("calls")[0] += 1;
        Err(std::io::Error::other("no creation"))
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        self.calls.lock().expect("calls")[1] += 1;
        Ok(None)
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        self.calls.lock().expect("calls")[2] += 1;
        Ok(None)
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        self.calls.lock().expect("calls")[3] += 1;
        Err(std::io::Error::other("no deletion"))
    }
}

impl InteractiveWorkerStopProvider for Provider {
    fn stop_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerStop, Self::Error> {
        self.calls.lock().expect("calls")[4] += 1;
        if let Some(action) = &self.on_stop {
            action(worker);
        }
        self.result.map_err(std::io::Error::other)
    }
}

#[test]
fn explicit_stop_is_durable_before_provider_io_and_preserves_all_other_state() {
    let fixture = Fixture::new();
    let before = fixture.current();
    let store = fixture.store.clone();
    let mut provider = Provider::new(Ok(InteractiveWorkerStop::Stopped));
    provider.on_stop = Some(Box::new(move |worker| {
        let saved = store
            .load_remote_allocation(OWNER, "workspace")
            .expect("read")
            .expect("saved");
        let runtime = saved.workspace().state().runtime.as_ref().expect("runtime");
        assert!(matches!(runtime.phase, RemoteRuntimePhase::Stopping { .. }));
        assert_eq!(runtime.worker.as_ref(), Some(worker));
    }));
    let stopped = fixture.stop(&provider).expect("stop");
    assert!(
        matches!(fixture.phase(), RemoteRuntimePhase::Stopped { requested_at_millis, observed_at_millis }
        if requested_at_millis > 0 && observed_at_millis >= requested_at_millis)
    );
    let mut original = before.workspace().state().clone();
    original.runtime.as_mut().expect("runtime").phase = fixture.phase();
    assert_eq!(stopped.workspace().state(), &original);
    assert_eq!(stopped.workflow(), before.workflow());
    assert_eq!(stopped.workspace().revision(), before.workspace().revision() + 2);
    assert_eq!(provider.counts(), [0, 0, 0, 0, 1]);
}

#[test]
fn provider_failure_and_absence_retain_intent_for_explicit_fresh_client_retry() {
    for (result, error) in [
        (Err("private-provider-marker"), Error::ProviderUnavailable),
        (Ok(InteractiveWorkerStop::AlreadyAbsent), Error::ResourceAbsent),
    ] {
        let fixture = Fixture::new();
        let before = fixture.current();
        let provider = Provider::new(result);
        let rejected = fixture.stop(&provider).expect_err("unverified Stop");
        assert!(!rejected.to_string().contains("private-provider-marker"));
        assert_eq!(rejected, error);
        let pending = fixture.current();
        let request = fixture.phase().stop_requested_at_millis().expect("durable intent");
        assert!(matches!(fixture.phase(), RemoteRuntimePhase::Stopping { .. }));
        assert_eq!(pending.workflow(), before.workflow());
        assert_eq!(pending.workspace().state().spec, before.workspace().state().spec);
        assert_eq!(
            pending.workspace().state().checkpoint,
            before.workspace().state().checkpoint
        );
        let reopened = CloudWorkflowStore::open_path(fixture.store.path()).expect("reopen");
        let retry = Provider::new(Ok(InteractiveWorkerStop::Stopped));
        let completed = stop_remote_workspace(&reopened, &retry, pending.workspace()).expect("explicit retry");
        assert_eq!(fixture.phase().stop_requested_at_millis(), Some(request));
        assert_eq!(
            stop_remote_workspace(&reopened, &retry, completed.workspace()),
            Ok(completed.clone())
        );
        assert_eq!(fixture.current(), completed);
        assert_eq!(retry.counts(), [0, 0, 0, 0, 2]);
        assert_eq!(provider.counts(), [0, 0, 0, 0, 1]);
    }
}

#[test]
fn time_limited_allocations_reject_durable_stop_without_intent_or_provider_changes() {
    let future = time::OffsetDateTime::now_utc() + time::Duration::seconds(900);
    for deadline in [future, time::OffsetDateTime::UNIX_EPOCH] {
        for saved_phase in [
            None,
            Some(RemoteRuntimePhase::Stopping { requested_at_millis: 1 }),
            Some(RemoteRuntimePhase::Stopped {
                requested_at_millis: 1,
                observed_at_millis: 1,
            }),
        ] {
            let fixture = Fixture::with_lifetime(InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
                terminate_after: deadline
                    .format(&time::format_description::well_known::Rfc3339)
                    .expect("valid deadline"),
            }));
            if let Some(phase) = saved_phase {
                fixture
                    .store
                    .record_remote_stop_phase(&fixture.current(), phase)
                    .expect("previously saved timed intent");
            }
            let before = fixture.current();
            assert_eq!(
                before.workspace().state().spec.target.lifetime,
                WorkerLifetime::TimeLimited { seconds: 900 }
            );
            let worker = before
                .workspace()
                .state()
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.worker.as_ref())
                .expect("retained worker");
            assert!(worker.is_valid_for(CloudProvider::LocalDocker));
            assert!(worker.lifetime.as_time_limited().is_some());
            let provider = Provider::new(Ok(InteractiveWorkerStop::Stopped));
            assert_eq!(fixture.stop(&provider), Err(Error::UnsupportedLifetime));
            assert_eq!(provider.counts(), [0; 5]);
            assert_eq!(fixture.current(), before);
        }
    }
}

#[test]
fn stale_foreign_wrong_provider_and_competing_legacy_intent_fail_before_stop() {
    let fixture = Fixture::new();
    let original = fixture.current();
    let mut provider = Provider::new(Ok(InteractiveWorkerStop::Stopped));
    provider.kind = CloudProvider::RunPod;
    assert_eq!(fixture.stop(&provider), Err(Error::ProviderMismatch));
    assert_eq!(fixture.current(), original);
    provider.kind = CloudProvider::LocalDocker;
    let foreign = Fixture::new();
    assert_eq!(
        stop_remote_workspace(&fixture.store, &provider, foreign.current().workspace()),
        Err(Error::StateChanged)
    );
    let mut next = original.workspace().state().clone();
    next.spec.working_directory = "nested".into();
    let changed = fixture
        .store
        .replace_remote_workspace(original.workspace(), &next)
        .expect("changed");
    assert_eq!(
        stop_remote_workspace(&fixture.store, &provider, original.workspace()),
        Err(Error::StateChanged)
    );
    next.runtime.as_mut().expect("runtime").cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::ApplicationExit,
        requested_at_millis: 1,
    });
    fixture
        .store
        .replace_remote_workspace(&changed, &next)
        .expect("legacy intent");
    assert_eq!(fixture.stop(&provider), Err(Error::ManagementConflict));
    assert_eq!(provider.counts(), [0; 5]);
}

#[test]
fn post_stop_workspace_or_workflow_drift_cannot_record_false_completion() {
    for change_workflow in [false, true] {
        let fixture = Fixture::new();
        let store = fixture.store.clone();
        let mut provider = Provider::new(Ok(InteractiveWorkerStop::Stopped));
        provider.on_stop = Some(Box::new(move |_| {
            let current = store
                .load_remote_allocation(OWNER, "workspace")
                .expect("read")
                .expect("allocation");
            if change_workflow {
                let mut next = current.workflow().workflow().clone();
                next.updated_at_millis += 1;
                store.replace(current.workflow(), &next).expect("workflow update");
            } else {
                let mut next = current.workspace().state().clone();
                next.spec.panels.clear();
                store
                    .replace_remote_workspace(current.workspace(), &next)
                    .expect("panel detached");
            }
        }));
        assert_eq!(fixture.stop(&provider), Err(Error::StateChanged));
        assert!(matches!(fixture.phase(), RemoteRuntimePhase::Stopping { .. }));
        assert_eq!(provider.counts(), [0, 0, 0, 0, 1]);
    }
}

#[test]
fn recovery_and_creation_remain_fenced_but_saved_inventory_is_observable() {
    for result in [Err("lost response"), Ok(InteractiveWorkerStop::Stopped)] {
        let fixture = Fixture::new();
        let _ = fixture.stop(&Provider::new(result));
        let saved = fixture.current();
        let provider = Provider::new(Ok(InteractiveWorkerStop::Stopped));
        let identities = RemoteSshIdentityStore::new(&HorizonHome::from_root(fixture.directory.path().join("no-keys")));
        assert_eq!(
            recover_remote_workspace(&fixture.store, &identities, &provider, OWNER, "workspace")
                .expect_err("management before identity access"),
            RemoteWorkspaceRecoveryError::ManagementPending
        );
        let request = saved.worker_request().expect("retained request");
        assert!(
            fixture
                .store
                .claim_worker_creation(request.workflow_id, request.job_id, &request.target, "no-worker")
                .is_err()
        );
        assert_eq!(provider.counts(), [0; 5]);
        let observed =
            observe_remote_environment(&fixture.store, &provider, saved.workspace()).expect("read-only inventory");
        assert_eq!(observed.saved, saved.workspace().environment_summary());
        assert!(observed.worker.is_none());
        assert_eq!(fixture.current(), saved);
        assert_eq!(provider.counts(), [0, 1, 0, 0, 0]);
        assert!(!fixture.directory.path().join("no-keys").exists());
    }
}

#[test]
fn late_recovery_results_cannot_erase_a_new_stop_request() {
    let fixture = Fixture::new();
    let before = fixture.current();
    assert_eq!(
        fixture.stop(&Provider::new(Err("uncertain response"))),
        Err(Error::ProviderUnavailable)
    );
    let stopping = fixture.current();
    assert!(matches!(
        fixture.store.record_remote_worker_recovery(&before, None),
        Err(RemoteWorkspaceStoreError::SnapshotConflict)
    ));
    assert!(matches!(
        fixture.store.record_remote_worker_recovery(&stopping, None),
        Err(RemoteWorkspaceStoreError::RuntimeRecoveryUnavailable)
    ));
    assert_eq!(fixture.current(), stopping);
}

#[test]
fn generic_updates_cannot_manufacture_stop_completion() {
    let fixture = Fixture::new();
    for begin_stop in [false, true] {
        if begin_stop {
            assert_eq!(
                fixture.stop(&Provider::new(Err("uncertain"))),
                Err(Error::ProviderUnavailable)
            );
        }
        let original = fixture.current();
        let request = fixture.phase().stop_requested_at_millis().unwrap_or(1);
        let mut next = original.workspace().state().clone();
        next.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Stopped {
            requested_at_millis: request,
            observed_at_millis: request,
        };
        assert!(matches!(
            fixture.store.replace_remote_workspace(original.workspace(), &next),
            Err(RemoteWorkspaceStoreError::RuntimeStopConfirmationRequired)
        ));
        assert_eq!(fixture.current(), original);
    }
    assert!(fixture.stop(&Provider::new(Ok(InteractiveWorkerStop::Stopped))).is_ok());
}

#[test]
fn generic_writes_cannot_erase_retarget_or_rewind_stop_intent() {
    for result in [Err("uncertain"), Ok(InteractiveWorkerStop::Stopped)] {
        let fixture = Fixture::new();
        let _ = fixture.stop(&Provider::new(result));
        let original = fixture.current();
        let request = fixture.phase().stop_requested_at_millis().expect("request");
        for replacement in [
            None,
            Some(RemoteRuntimePhase::Reconciling),
            Some(RemoteRuntimePhase::Provisioning),
            Some(RemoteRuntimePhase::Stopping {
                requested_at_millis: request + 1,
            }),
            Some(RemoteRuntimePhase::Stopped {
                requested_at_millis: request + 1,
                observed_at_millis: request + 1,
            }),
        ] {
            let mut next = original.workspace().state().clone();
            if let Some(phase) = replacement {
                next.runtime.as_mut().expect("runtime").phase = phase;
            } else {
                next.runtime = None;
            }
            assert!(
                fixture
                    .store
                    .replace_remote_workspace(original.workspace(), &next)
                    .is_err()
            );
            assert_eq!(fixture.current(), original);
        }
        if matches!(fixture.phase(), RemoteRuntimePhase::Stopped { .. }) {
            let mut next = original.workspace().state().clone();
            next.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Stopping {
                requested_at_millis: request,
            };
            assert!(
                fixture
                    .store
                    .replace_remote_workspace(original.workspace(), &next)
                    .is_err()
            );
        }
    }
}

#[test]
fn phase_serialization_preserves_legacy_meanings_and_rejects_malformed_stop_state() {
    for name in [
        "provisioning",
        "reconciling",
        "materializing",
        "ready",
        "checkpointing",
        "cancelling",
        "deleting",
        "failed",
    ] {
        let encoded = serde_json::json!(name);
        let phase: RemoteRuntimePhase = serde_json::from_value(encoded.clone()).expect("legacy phase");
        assert_eq!(serde_json::to_value(phase).expect("unchanged representation"), encoded);
        assert!(phase.stop_requested_at_millis().is_none());
    }
    let fixture = Fixture::new();
    for phase in [
        serde_json::json!({"stopping":{"requested_at_millis":-1}}),
        serde_json::json!({"stopped":{"requested_at_millis":2,"observed_at_millis":1}}),
        serde_json::json!({"stopping":{}}),
        serde_json::json!({"stopped":{"requested_at_millis":1}}),
        serde_json::json!({"stopping":{"requested_at_millis":1,"unexpected":"private-marker"}}),
    ] {
        let mut encoded = serde_json::to_value(fixture.current().workspace().state()).expect("encode");
        encoded["runtime"]["phase"] = phase;
        assert!(serde_json::from_value::<RemoteWorkspaceState>(encoded).is_err());
    }
    for phase in [
        RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
        RemoteRuntimePhase::Stopped {
            requested_at_millis: 1,
            observed_at_millis: 2,
        },
    ] {
        let mut state = fixture.current().workspace().state().clone();
        state.runtime.as_mut().expect("runtime").phase = phase;
        let json = serde_json::to_value(&state).expect("encode");
        assert_eq!(
            serde_json::from_value::<RemoteWorkspaceState>(json).expect("round trip"),
            state
        );
        state.runtime.as_mut().expect("runtime").cleanup = Some(RemoteCleanupIntent {
            reason: RemoteCleanupReason::Cancelled,
            requested_at_millis: 1,
        });
        assert!(state.validate().is_err());
        state.runtime.as_mut().expect("runtime").cleanup = None;
        state.runtime.as_mut().expect("runtime").worker = None;
        assert!(state.validate().is_err());
    }
}

#[test]
fn a_missing_worker_or_future_request_never_reaches_provider_io() {
    let fixture = Fixture::new();
    let provider = Provider::new(Ok(InteractiveWorkerStop::Stopped));
    let original = fixture.current();
    let mut next = original.workspace().state().clone();
    next.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Stopping {
        requested_at_millis: i64::MAX,
    };
    fixture
        .store
        .replace_remote_workspace(original.workspace(), &next)
        .expect("future saved request");
    assert_eq!(fixture.stop(&provider), Err(Error::InvalidTimestamp));
    assert_eq!(provider.counts(), [0; 5]);
    let mut dormant = next;
    dormant.spec.workspace_local_id = "dormant".into();
    dormant.runtime = None;
    dormant.checkpoint = None;
    let saved = fixture.store.create_remote_workspace(OWNER, &dormant).expect("dormant");
    assert_eq!(
        stop_remote_workspace(&fixture.store, &provider, &saved),
        Err(Error::MissingAllocation)
    );
    let unobserved = fixture
        .store
        .allocate_remote_runtime(&saved, i64::MAX)
        .expect("reserved allocation");
    assert_eq!(
        stop_remote_workspace(&fixture.store, &provider, unobserved.workspace()),
        Err(Error::MissingWorker)
    );
    assert_eq!(provider.counts(), [0; 5]);
}
