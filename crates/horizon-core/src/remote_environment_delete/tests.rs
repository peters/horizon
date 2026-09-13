use super::*;
use crate::{
    cloud_run::{
        CloudProvider,
        interactive_worker::{
            InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerIdentity, InteractiveWorkerLifecycle,
            InteractiveWorkerLifetime, InteractiveWorkerProvider, InteractiveWorkerRequest, InteractiveWorkerStatus,
        },
    },
    remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason, RemoteWorkspaceState},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

const OWNER: &str = "00000000-0000-4000-8000-000000000001";

struct Fixture {
    _root: tempfile::TempDir,
    store: CloudWorkflowStore,
}

impl Fixture {
    fn new(stopped: bool) -> Self {
        let root = tempfile::tempdir().expect("root");
        let store = CloudWorkflowStore::open_path(root.path().join("control/workflows.sqlite3")).expect("store");
        let state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version":1,"spec":{"workspace_local_id":"workspace","working_directory":".",
            "generation":0,"panels":[],"target":{"provider":"local_docker","profile":"development",
            "disk_gib":20,"lifetime":"persistent","image":format!("example/worker@sha256:{}","a".repeat(64))},
            "repository":{"repository":"example/project","commit":"b".repeat(40)}}
        }))
        .expect("state");
        let saved = store.create_remote_workspace(OWNER, &state).expect("saved");
        let allocation = store.allocate_remote_runtime(&saved, i64::MAX).expect("allocated");
        let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
        blob.extend([7; 32]);
        let key = format!("ssh-ed25519 {}", STANDARD.encode(blob));
        let reserved = store
            .reserve_remote_worker_request(&allocation, &key)
            .expect("reserved");
        let request = reserved.worker_request().expect("request");
        let worker = InteractiveWorker {
            identity: InteractiveWorkerIdentity {
                provider: CloudProvider::LocalDocker,
                workflow_id: request.workflow_id,
                job_id: request.job_id,
                resource_id: "a".repeat(64),
            },
            target: request.target,
            ssh_public_key: request.ssh_public_key,
            lifetime: InteractiveWorkerLifetime::Persistent,
        };
        let allocation = store
            .record_remote_worker_recovery(
                &reserved,
                Some(&InteractiveWorkerStatus {
                    worker,
                    lifecycle: InteractiveWorkerLifecycle::Provisioning,
                    ssh: None,
                }),
            )
            .expect("observed");
        if stopped {
            store
                .record_remote_stop_phase(
                    &allocation,
                    RemoteRuntimePhase::Stopped {
                        requested_at_millis: 1,
                        observed_at_millis: 2,
                    },
                )
                .expect("saved Stop");
        }
        Self { _root: root, store }
    }
    fn current(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation")
    }
    fn delete(&self, provider: &Provider) -> Result<RemoteEnvironmentDeletion, Error> {
        delete_remote_environment(&self.store, provider, &self.current())
    }
}

type Hook = Box<dyn Fn(&InteractiveWorker) + Send + Sync>;
struct Provider {
    present: AtomicBool,
    fail_delete: bool,
    fail_observe: bool,
    calls: Mutex<Vec<&'static str>>,
    on_delete: Option<Hook>,
    on_observe: Option<Hook>,
    kind: CloudProvider,
}
impl Provider {
    fn new(present: bool) -> Self {
        Self {
            present: AtomicBool::new(present),
            fail_delete: false,
            fail_observe: false,
            calls: Mutex::new(Vec::new()),
            on_delete: None,
            on_observe: None,
            kind: CloudProvider::LocalDocker,
        }
    }
    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().expect("calls").clone()
    }
}
impl InteractiveWorkerProvider for Provider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        self.kind
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("no create")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("no reconcile")
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("no SSH readiness")
    }
    fn delete_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        self.calls.lock().expect("calls").push("delete");
        if let Some(hook) = &self.on_delete {
            hook(worker);
        }
        if self.fail_delete {
            Err(std::io::Error::other("private delete payload"))
        } else {
            Ok(InteractiveWorkerCleanup::Deleted)
        }
    }
}
impl InteractiveWorkerDeleteObserver for Provider {
    fn observe_worker_deletion(
        &self,
        worker: &InteractiveWorker,
    ) -> Result<InteractiveWorkerDeletionObservation, Self::Error> {
        self.calls.lock().expect("calls").push("observe");
        if let Some(hook) = &self.on_observe {
            hook(worker);
        }
        if self.fail_observe {
            return Err(std::io::Error::other("private observation payload"));
        }
        Ok(if self.present.load(Ordering::SeqCst) {
            InteractiveWorkerDeletionObservation::Present
        } else {
            InteractiveWorkerDeletionObservation::Absent
        })
    }
}

#[test]
fn intent_precedes_single_delete_and_absence_preserves_tombstone_and_fence() {
    for stopped in [false, true] {
        let fixture = Fixture::new(stopped);
        let before = fixture.current();
        let mut other_spec = before.workspace().state().spec.clone();
        other_spec.workspace_local_id = "unrelated".into();
        other_spec.generation = 0;
        let other = fixture
            .store
            .create_remote_workspace(OWNER, &RemoteWorkspaceState::new(other_spec).expect("other state"))
            .expect("other workspace");
        let store = fixture.store.clone();
        let mut provider = Provider::new(false);
        provider.on_delete = Some(Box::new(move |worker| {
            let saved = store
                .load_remote_allocation(OWNER, "workspace")
                .expect("read")
                .expect("saved");
            let runtime = saved.workspace().state().runtime.as_ref().expect("runtime");
            assert!(matches!(runtime.phase, RemoteRuntimePhase::DeleteRequested { .. }));
            assert_eq!(runtime.worker.as_ref(), Some(worker));
            assert_eq!(
                runtime.cleanup.as_ref().expect("intent").reason,
                RemoteCleanupReason::WorkspaceRemoved
            );
        }));
        let result = fixture.delete(&provider).expect("deleted");
        assert!(result.absence_verified);
        assert_eq!(provider.calls(), ["delete", "observe"]);
        let after = &result.allocation;
        assert_eq!(before.workflow(), after.workflow());
        assert_eq!(before.workspace().state().spec, after.workspace().state().spec);
        assert_eq!(before.workspace().revision() + 2, after.workspace().revision());
        assert_eq!(retained_worker(&before), retained_worker(after));
        assert_eq!(fixture.current(), *after);
        assert!(
            fixture
                .store
                .claim_worker_creation(
                    before.workflow().workflow().id,
                    retained_worker(&before).expect("worker").identity.job_id,
                    &before.workspace().state().spec.target,
                    "synthetic-worker"
                )
                .is_err()
        );
        assert!(
            fixture
                .store
                .list_remote_workspaces(OWNER)
                .expect("workspaces")
                .contains(&other)
        );
        assert!(
            fixture
                .store
                .allocate_remote_runtime(after.workspace(), i64::MAX)
                .is_err()
        );
        assert!(fixture.store.record_remote_worker_recovery(after, None).is_err());
        let again = confirm_remote_environment_deletion(&fixture.store, &provider, after).expect("historical");
        assert_eq!(again, result);
        assert_eq!(provider.calls(), ["delete", "observe"]);
        assert_eq!(fixture.delete(&provider), Err(Error::ManagementConflict));
    }
}

#[test]
fn accepted_or_lost_response_stays_pending_and_explicit_check_never_replays() {
    for fail_delete in [false, true] {
        let fixture = Fixture::new(true);
        let mut provider = Provider::new(true);
        provider.fail_delete = fail_delete;
        let pending = fixture.delete(&provider).expect("pending");
        assert!(!pending.absence_verified);
        assert_eq!(fixture.delete(&provider), Err(Error::ManagementConflict));
        let still =
            confirm_remote_environment_deletion(&fixture.store, &provider, &pending.allocation).expect("present");
        assert_eq!(still, pending);
        provider.present.store(false, Ordering::SeqCst);
        let complete =
            confirm_remote_environment_deletion(&fixture.store, &provider, &pending.allocation).expect("absent");
        assert!(complete.absence_verified);
        assert_eq!(provider.calls(), ["delete", "observe", "observe", "observe"]);
        assert_eq!(
            complete.allocation.workspace().revision(),
            pending.allocation.workspace().revision() + 1
        );
    }
}

#[test]
fn separately_confirmed_retry_checks_before_one_dispatch_and_retains_identity() {
    let fixture = Fixture::new(true);
    let original = fixture.current();
    assert_eq!(
        retry_remote_environment_deletion(&fixture.store, &Provider::new(true), &original),
        Err(Error::MissingDeleteIntent)
    );
    let pending = fixture.delete(&Provider::new(true)).expect("pending").allocation;
    let store = fixture.store.clone();
    let snapshot = pending.clone();
    let mut provider = Provider::new(true);
    provider.on_delete = Some(Box::new(move |_| {
        let admitted = store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("saved");
        assert_eq!(admitted.workspace().revision(), snapshot.workspace().revision() + 1);
        assert_eq!(admitted.workspace().state(), snapshot.workspace().state());
        assert_eq!(admitted.workflow(), snapshot.workflow());
    }));
    let retried = retry_remote_environment_deletion(&fixture.store, &provider, &pending).expect("retry");
    assert!(!retried.absence_verified);
    assert_eq!(provider.calls(), ["observe", "delete", "observe"]);
    assert_eq!(retained_worker(&retried.allocation), retained_worker(&pending));
    assert_eq!(
        retry_remote_environment_deletion(&fixture.store, &provider, &pending),
        Err(Error::StateChanged)
    );
    assert_eq!(provider.calls(), ["observe", "delete", "observe"]);
    assert_eq!(fixture.delete(&provider), Err(Error::ManagementConflict));
}

#[test]
fn retry_observation_absence_or_error_never_dispatches_delete() {
    for absent in [false, true] {
        let fixture = Fixture::new(false);
        let pending = fixture.delete(&Provider::new(true)).expect("pending").allocation;
        let mut provider = Provider::new(false);
        provider.fail_observe = !absent;
        let result = retry_remote_environment_deletion(&fixture.store, &provider, &pending);
        if absent {
            assert!(result.expect("absent").absence_verified);
            assert_eq!(
                fixture.current().workspace().revision(),
                pending.workspace().revision() + 1
            );
        } else {
            assert_eq!(result, Err(Error::ProviderUnavailable));
            assert_eq!(fixture.current(), pending);
        }
        assert_eq!(provider.calls(), ["observe"]);
    }
}

#[test]
fn retry_observation_race_cannot_dispatch_or_rewind_confirmation() {
    let fixture = Fixture::new(false);
    let pending = fixture.delete(&Provider::new(true)).expect("pending").allocation;
    let store = fixture.store.clone();
    let snapshot = pending.clone();
    let mut provider = Provider::new(true);
    provider.on_observe = Some(Box::new(move |_| {
        assert!(
            confirm_remote_environment_deletion(&store, &Provider::new(false), &snapshot)
                .expect("concurrent confirmation")
                .absence_verified
        );
    }));
    assert_eq!(
        retry_remote_environment_deletion(&fixture.store, &provider, &pending),
        Err(Error::StateChanged)
    );
    assert_eq!(provider.calls(), ["observe"]);
    assert!(matches!(
        fixture
            .current()
            .workspace()
            .state()
            .runtime
            .as_ref()
            .expect("runtime")
            .phase,
        RemoteRuntimePhase::Deleted { .. }
    ));
}

#[test]
fn competing_retry_of_same_snapshot_has_one_dispatch_even_with_a_lost_reply() {
    let fixture = Fixture::new(false);
    let pending = fixture.delete(&Provider::new(true)).expect("pending").allocation;
    let store = fixture.store.clone();
    let snapshot = pending.clone();
    let mut winner = Provider::new(true);
    winner.fail_delete = true;
    let winner = std::sync::Arc::new(winner);
    let nested = winner.clone();
    let mut loser = Provider::new(true);
    loser.on_observe = Some(Box::new(move |_| {
        let result = retry_remote_environment_deletion(&store, nested.as_ref(), &snapshot).expect("retry");
        assert!(!result.absence_verified);
    }));
    assert_eq!(
        retry_remote_environment_deletion(&fixture.store, &loser, &pending),
        Err(Error::StateChanged)
    );
    assert_eq!(loser.calls(), ["observe"]);
    assert_eq!(winner.calls(), ["observe", "delete", "observe"]);
    assert_eq!(fixture.current().workspace().state(), pending.workspace().state());
    assert_eq!(
        fixture.current().workspace().revision(),
        pending.workspace().revision() + 1
    );
}

#[test]
fn failed_observation_retains_intent_and_redacts_provider_payload() {
    let fixture = Fixture::new(false);
    let mut provider = Provider::new(false);
    provider.fail_delete = true;
    provider.fail_observe = true;
    let error = fixture.delete(&provider).expect_err("unverified");
    assert_eq!(error, Error::ProviderUnavailable);
    assert!(!format!("{error:?} {error}").contains("private"));
    assert!(matches!(
        fixture
            .current()
            .workspace()
            .state()
            .runtime
            .as_ref()
            .expect("runtime")
            .phase,
        RemoteRuntimePhase::DeleteRequested { .. }
    ));
    provider.fail_observe = false;
    assert!(
        confirm_remote_environment_deletion(&fixture.store, &provider, &fixture.current())
            .expect("check")
            .absence_verified
    );
    assert_eq!(provider.calls(), ["delete", "observe", "observe"]);
}

#[test]
fn stale_foreign_missing_intent_and_conflicting_management_refuse_before_io() {
    let fixture = Fixture::new(false);
    let provider = Provider::new(false);
    let before = fixture.current();
    assert_eq!(
        confirm_remote_environment_deletion(&fixture.store, &provider, &before),
        Err(Error::MissingDeleteIntent)
    );
    let mut changed = before.workspace().state().clone();
    changed.spec.working_directory = "src".into();
    fixture
        .store
        .replace_remote_workspace(before.workspace(), &changed)
        .expect("changed");
    assert_eq!(
        delete_remote_environment(&fixture.store, &provider, &before),
        Err(Error::StateChanged)
    );
    let mut foreign = Provider::new(false);
    foreign.kind = CloudProvider::Azure;
    assert_eq!(fixture.delete(&foreign), Err(Error::ProviderMismatch));
    assert!(provider.calls().is_empty());
    assert!(foreign.calls().is_empty());
    for reason in [
        RemoteCleanupReason::Cancelled,
        RemoteCleanupReason::WorkspaceRemoved,
        RemoteCleanupReason::ApplicationExit,
    ] {
        let f = Fixture::new(false);
        let saved = f.current();
        let mut state = saved.workspace().state().clone();
        // Legacy cleanup intent alone is enough to refuse explicit Delete, whatever its phase.
        state.runtime.as_mut().expect("runtime").cleanup = Some(RemoteCleanupIntent {
            reason,
            requested_at_millis: 1,
        });
        f.store
            .replace_remote_workspace(saved.workspace(), &state)
            .expect("legacy intent");
        assert_eq!(f.delete(&provider), Err(Error::ManagementConflict));
    }
    assert!(provider.calls().is_empty());
}

#[test]
fn concurrent_confirmation_cannot_be_overwritten_or_cause_another_delete() {
    let fixture = Fixture::new(false);
    let store = fixture.store.clone();
    let mut provider = Provider::new(false);
    provider.on_delete = Some(Box::new(move |_| {
        let pending = store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("pending");
        let result =
            confirm_remote_environment_deletion(&store, &Provider::new(false), &pending).expect("concurrent check");
        assert!(result.absence_verified);
    }));
    assert_eq!(fixture.delete(&provider), Err(Error::StateChanged));
    assert_eq!(provider.calls(), ["delete", "observe"]);
    assert!(matches!(
        fixture
            .current()
            .workspace()
            .state()
            .runtime
            .as_ref()
            .expect("runtime")
            .phase,
        RemoteRuntimePhase::Deleted { .. }
    ));
}

#[test]
fn generic_writes_cannot_forge_complete_rewind_or_erase_delete_intent() {
    let fixture = Fixture::new(true);
    let saved = fixture.current();
    let mut forged = saved.workspace().state().clone();
    let runtime = forged.runtime.as_mut().expect("runtime");
    runtime.phase = RemoteRuntimePhase::DeleteRequested { requested_at_millis: 3 };
    runtime.cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::WorkspaceRemoved,
        requested_at_millis: 3,
    });
    assert!(
        fixture
            .store
            .replace_remote_workspace(saved.workspace(), &forged)
            .is_err()
    );
    let pending = fixture.delete(&Provider::new(true)).expect("pending").allocation;
    let original = pending.workspace().state();
    for alteration in 0..5 {
        let mut next = original.clone();
        match alteration {
            0 => next.runtime = None,
            1 => next.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Reconciling,
            2 => next.runtime.as_mut().expect("runtime").cleanup = None,
            3 => next.spec.working_directory = "different".into(),
            _ => {
                let runtime = next.runtime.as_mut().expect("runtime");
                let time = runtime.phase.delete_requested_at_millis().expect("time");
                runtime.phase = RemoteRuntimePhase::Deleted {
                    requested_at_millis: time,
                    observed_at_millis: time,
                };
            }
        }
        assert!(
            fixture
                .store
                .replace_remote_workspace(pending.workspace(), &next)
                .is_err()
        );
        assert_eq!(fixture.current(), pending);
    }
    assert!(
        fixture
            .store
            .record_remote_stop_phase(&pending, RemoteRuntimePhase::Stopping { requested_at_millis: 1 })
            .is_err()
    );
    assert!(
        fixture
            .store
            .record_remote_start_phase(&pending, RemoteRuntimePhase::Reconciling)
            .is_err()
    );
}

#[test]
fn malformed_tombstones_and_unsafe_time_are_rejected() {
    let fixture = Fixture::new(true);
    let before = fixture.current();
    assert!(
        fixture
            .store
            .record_remote_delete_phase(&before, RemoteRuntimePhase::DeleteRequested { requested_at_millis: 1 })
            .is_err()
    );
    assert_eq!(fixture.current(), before);
    let deleted = fixture.delete(&Provider::new(false)).expect("deleted").allocation;
    let encoded = serde_json::to_value(deleted.workspace().state()).expect("encode");
    assert_eq!(
        serde_json::from_value::<RemoteWorkspaceState>(encoded.clone()).expect("roundtrip"),
        *deleted.workspace().state()
    );
    for change in 0..4 {
        let mut value = encoded.clone();
        match change {
            0 => value["runtime"]["cleanup"] = serde_json::Value::Null,
            1 => value["runtime"]["worker"] = serde_json::Value::Null,
            2 => value["runtime"]["cleanup"]["reason"] = serde_json::json!("application_exit"),
            _ => value["runtime"]["phase"]["deleted"]["observed_at_millis"] = serde_json::json!(-1),
        }
        assert!(serde_json::from_value::<RemoteWorkspaceState>(value).is_err());
    }
}
