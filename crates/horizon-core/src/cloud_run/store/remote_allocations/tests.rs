mod hardening;

use super::super::{encode_workflow, tests::retained_workflow};
use super::*;
use crate::cloud_run::interactive_worker::{
    InteractiveWorker, InteractiveWorkerIdentity, InteractiveWorkerLifetime, InteractiveWorkerSshEndpoint,
};
use crate::cloud_run::{ArtifactDigest, CloudJobOutcome, CloudProvider, GitCommitSha, GitSource, WorkerLifetime};
use crate::remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason, RemoteWorkspaceSpec, RepositoryCheckpoint};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rusqlite::Connection;
use std::sync::{Arc, Barrier};

const OWNER: &str = "11111111-1111-4111-8111-111111111111";
const OTHER_OWNER: &str = "22222222-2222-4222-8222-222222222222";

struct Fixture {
    _directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    original: StoredRemoteWorkspace,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("private directory");
    let store = CloudWorkflowStore::open_path(directory.path().join("control/workflows.sqlite3")).expect("store");
    let workflow = retained_workflow(CloudProvider::LocalDocker, 1000);
    let mut target = workflow.nodes[0].worker.clone().expect("target");
    target.lifetime = WorkerLifetime::Persistent;
    let state = RemoteWorkspaceState::new(RemoteWorkspaceSpec {
        workspace_local_id: "workspace".into(),
        target,
        repository: GitSource {
            repository: "owner/project".into(),
            commit: GitCommitSha::parse("b".repeat(40)).expect("commit"),
            branch: None,
        },
        working_directory: ".".into(),
        generation: 0,
        panels: Vec::new(),
    })
    .expect("workspace specification");
    let original = store.create_remote_workspace(OWNER, &state).expect("dormant");
    Fixture {
        _directory: directory,
        store,
        original,
    }
}

impl Fixture {
    fn allocate(&self) -> StoredRemoteAllocation {
        self.store
            .allocate_remote_runtime(&self.original, i64::MAX)
            .expect("allocate")
    }

    fn recover(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("recovery")
            .expect("allocation")
    }

    fn counts(&self) -> [i64; 3] {
        Connection::open(self.store.path())
            .expect("raw store")
            .query_row(
                "SELECT (SELECT COUNT(*) FROM cloud_workflows),
             (SELECT COUNT(*) FROM remote_runtime_allocations),
             (SELECT COUNT(*) FROM cloud_worker_creation_claims)",
                [],
                |row| Ok([row.get(0)?, row.get(1)?, row.get(2)?]),
            )
            .expect("record counts")
    }
}

fn claim(store: &CloudWorkflowStore, saved: &StoredRemoteAllocation) -> Result<bool, CloudStoreError> {
    let workflow = saved.workflow().workflow();
    store.claim_worker_creation(
        workflow.id,
        workflow.nodes[0].id,
        &saved.workspace().state().spec.target,
        "synthetic-worker",
    )
}

fn observed_worker(state: &RemoteWorkspaceState) -> InteractiveWorker {
    let runtime = state.runtime.as_ref().expect("runtime");
    let mut key = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
    key.extend([7; 32]);
    InteractiveWorker {
        identity: InteractiveWorkerIdentity {
            provider: state.spec.target.provider,
            workflow_id: runtime.workflow_id,
            job_id: runtime.job_id,
            resource_id: "synthetic-worker".into(),
        },
        target: state.spec.target.clone(),
        ssh_public_key: format!("ssh-ed25519 {}", STANDARD.encode(key)),
        lifetime: InteractiveWorkerLifetime::Persistent,
    }
}

#[test]
fn allocation_reopens_one_exact_identity_without_consuming_creation_or_reallocating() {
    let fixture = fixture();
    let store = &fixture.store;
    assert_eq!(store.load_remote_allocation(OWNER, "workspace").expect("dormant"), None);
    let started = current_unix_millis().expect("clock");
    let saved = fixture.allocate();
    let state = saved.workspace().state();
    let runtime = state.runtime.as_ref().expect("runtime");
    let workflow = saved.workflow().workflow();
    assert!((started..=current_unix_millis().expect("clock")).contains(&workflow.created_at_millis));
    assert_eq!(workflow.updated_at_millis, workflow.created_at_millis);
    assert_eq!(workflow.retain_until_millis, i64::MAX);
    assert_eq!((saved.workspace().revision(), state.spec.generation), (2, 1));
    assert!(state.spec.panels.is_empty());
    assert!(runtime.cleanup.is_none());
    assert_eq!(runtime.workflow_id, workflow.id);
    assert_eq!(workflow.nodes.len(), 1);
    assert_eq!(runtime.job_id, workflow.nodes[0].id);
    assert_eq!(workflow.nodes[0].kind, WorkflowNodeKind::RemoteWorkspace);
    assert_eq!(workflow.nodes[0].worker.as_ref(), Some(&state.spec.target));
    assert_eq!(workflow.nodes[0].source.as_ref(), Some(&state.spec.repository));
    Connection::open(store.path())
        .expect("schema-three allocation fixture")
        .execute_batch(
            "DROP TABLE remote_first_pin_intents; DROP TABLE remote_runtime_creation_fences; PRAGMA user_version=3",
        )
        .expect("downgrade fixture before recovery");
    let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen");
    assert_eq!(
        reopened.load_remote_allocation(OWNER, "workspace").expect("recover"),
        Some(saved.clone())
    );
    assert!(matches!(
        reopened.allocate_remote_runtime(&fixture.original, i64::MAX),
        Err(Error::RevisionConflict { .. })
    ));
    assert!(matches!(
        reopened.allocate_remote_runtime(saved.workspace(), i64::MAX),
        Err(Error::RuntimeAlreadyActive)
    ));
    assert!(matches!(
        reopened.load_remote_allocation(OTHER_OWNER, "workspace"),
        Err(Error::OwnershipMismatch)
    ));
    assert_eq!(fixture.counts(), [1, 1, 0]);
    assert!(claim(store, &saved).expect("first grant"));
    let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen claimed store");
    assert!(!claim(&reopened, &saved).expect("no second grant"));
    assert_eq!(fixture.counts(), [1, 1, 1]);
}

#[test]
fn concurrent_allocators_commit_exactly_one_complete_relationship() {
    let fixture = fixture();
    let store = &fixture.store;
    let barrier = Arc::new(Barrier::new(3));
    let writers: Vec<_> = (0..2)
        .map(|_| {
            let store = store.clone();
            let original = fixture.original.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                store.allocate_remote_runtime(&original, i64::MAX)
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = writers
        .into_iter()
        .map(|writer| writer.join().expect("writer"))
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(Error::RevisionConflict { expected: 1, actual: 2 })))
            .count(),
        1
    );
    assert_eq!(
        fixture.recover(),
        results.into_iter().find_map(Result::ok).expect("winner")
    );
    assert_eq!(fixture.counts(), [1, 1, 0]);
}

#[test]
fn workflow_capacity_failure_rolls_back_the_pending_generation() {
    let fixture = fixture();
    let store = &fixture.store;
    let mut connection = Connection::open(store.path()).expect("raw store");
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .expect("fixture transaction");
    ensure_current_schema(&transaction).expect("current fixture schema");
    for _ in 0..MAX_RECOVERED_WORKFLOWS {
        let workflow = retained_workflow(CloudProvider::LocalDocker, 1000);
        PreparedWorkflowInsert::new(&workflow)
            .expect("prepare fixture")
            .persist(&transaction)
            .expect("retained workflow");
    }
    transaction.commit().expect("full retained set");
    assert_eq!(
        store.list_retained(1000).expect("recoverable budget").len(),
        MAX_RECOVERED_WORKFLOWS
    );
    assert!(matches!(
        store.allocate_remote_runtime(&fixture.original, i64::MAX),
        Err(Error::Storage(CloudStoreError::RecoveryLimitExceeded))
    ));
    assert_eq!(
        fixture.counts(),
        [i64::try_from(MAX_RECOVERED_WORKFLOWS).expect("bounded count"), 0, 0]
    );
    assert_eq!(
        store.load_remote_workspace(OWNER, "workspace").expect("unchanged"),
        Some(fixture.original.clone())
    );
    connection
        .execute(
            "DELETE FROM cloud_workflows WHERE workflow_id IN (SELECT workflow_id FROM cloud_workflows LIMIT 1)",
            [],
        )
        .expect("free one fixture slot");
    assert_eq!(fixture.allocate().workspace().state().spec.generation, 1);
}

#[test]
fn generic_writes_cannot_clear_copy_or_reclassify_even_when_the_binding_is_lost() {
    for lost_binding in [false, true] {
        let fixture = fixture();
        let store = &fixture.store;
        let saved = fixture.allocate();
        if lost_binding {
            Connection::open(store.path())
                .expect("raw store")
                .execute("DELETE FROM remote_runtime_allocations", [])
                .expect("lost binding");
        }
        let mut cleared = saved.workspace().state().clone();
        cleared.runtime = None;
        assert!(matches!(
            store.replace_remote_workspace(saved.workspace(), &cleared),
            Err(Error::Storage(CloudStoreError::InvalidRemoteAllocation))
        ));
        let mut copy = saved.workspace().state().clone();
        copy.spec.workspace_local_id = "copy".into();
        copy.runtime.as_mut().expect("runtime").workspace_local_id = "copy".into();
        assert!(matches!(
            store.create_remote_workspace(OTHER_OWNER, &copy),
            Err(Error::RuntimeAllocationRequired)
        ));
        let mut next = saved.workflow().workflow().clone();
        next.updated_at_millis += 1;
        next.nodes[0].kind = WorkflowNodeKind::Build;
        assert!(matches!(
            store.replace(saved.workflow(), &next),
            Err(CloudStoreError::InvalidRemoteAllocation)
        ));
        if lost_binding {
            assert!(matches!(
                store.load_remote_allocation(OWNER, "workspace"),
                Err(Error::UnboundRuntime)
            ));
            assert!(matches!(
                claim(store, &saved),
                Err(CloudStoreError::InvalidRemoteAllocation)
            ));
        }
        assert_eq!(
            store.load_remote_workspace(OWNER, "workspace").expect("retained"),
            Some(saved.workspace().clone())
        );
        assert_eq!(fixture.counts(), [1, i64::from(!lost_binding), 0]);
    }
}

#[test]
fn workflow_identity_and_single_node_shape_are_enforced_but_progress_can_update() {
    let fixture = fixture();
    let store = &fixture.store;
    let saved = fixture.allocate();
    let workflow = saved.workflow().workflow();
    let mut copy = workflow.clone();
    copy.id = CloudWorkflowId::new();
    assert!(matches!(
        store.create(&copy),
        Err(CloudStoreError::RemoteAllocationRequired)
    ));
    for change in [0, 1, 2, 3] {
        let mut next = workflow.clone();
        next.updated_at_millis += 1;
        match change {
            0 => next.nodes[0].worker.as_mut().expect("target").disk_gib += 1,
            1 => next.nodes[0].source.as_mut().expect("source").repository = "other/project".into(),
            2 => next.nodes[0].kind = WorkflowNodeKind::Build,
            _ => next.nodes[0].worker.as_mut().expect("target").lifetime = WorkerLifetime::TimeLimited { seconds: 900 },
        }
        assert!(matches!(
            store.replace(saved.workflow(), &next),
            Err(CloudStoreError::InvalidRemoteAllocation)
        ));
    }
    let mut progress = workflow.clone();
    progress.updated_at_millis += 1;
    progress.nodes[0].state = CloudJobState::Provisioning;
    let updated = store.replace(saved.workflow(), &progress).expect("progress");
    assert_eq!(fixture.recover().workflow(), &updated);
    let mut observed = updated;
    for (phase, outcome) in [
        (CloudJobState::Running, None),
        (CloudJobState::Failed, Some(CloudJobOutcome::Failed)),
    ] {
        let mut next = observed.workflow().clone();
        next.updated_at_millis += 1;
        next.nodes[0].state = phase;
        next.nodes[0].outcome = outcome;
        observed = store.replace(&observed, &next).expect("non-creating progress");
        for eligible in [CloudJobState::Queued, CloudJobState::Provisioning] {
            next.updated_at_millis += 1;
            next.nodes[0].state = eligible;
            next.nodes[0].outcome = None;
            assert!(matches!(
                store.replace(&observed, &next),
                Err(CloudStoreError::InvalidRemoteAllocation)
            ));
        }
        assert_eq!(fixture.recover().workflow(), &observed);
        assert!(matches!(
            claim(store, &saved),
            Err(CloudStoreError::ClaimTargetNotReady(_))
        ));
        assert_eq!(fixture.counts(), [1, 1, 0]);
    }
    for change in [0, 1, 2, 3] {
        let mut next = saved.workflow().workflow().clone();
        match change {
            0 => next.nodes[0].source = None,
            1 => next.nodes[0].worker = None,
            2 => next.nodes[0].retry.max_attempts = 2,
            _ => {
                let mut extra = next.nodes[0].clone();
                extra.id = CloudJobId::new();
                extra.logical_key = "other".into();
                next.nodes.push(extra);
            }
        }
        assert!(next.validate().is_err());
    }
}

#[test]
fn expired_setup_keeps_worker_ssh_and_checkpoints_without_renewal_or_new_generation() {
    let fixture = fixture();
    let store = &fixture.store;
    let saved = fixture.allocate();
    let mut expired = saved.workflow().workflow().clone();
    expired.created_at_millis = 1000;
    expired.updated_at_millis = 1000;
    expired.retain_until_millis = 2000;
    Connection::open(store.path())
        .expect("raw store")
        .execute(
            "UPDATE cloud_workflows SET created_at_millis=1000, updated_at_millis=1000,
         retain_until_millis=2000, snapshot=?1 WHERE workflow_id=?2",
            params![
                encode_workflow(&expired).expect("expired snapshot"),
                expired.id.to_string()
            ],
        )
        .expect("elapsed setup fixture");
    let saved = fixture.recover();
    let mut state = saved.workspace().state().clone();
    let worker = observed_worker(&state);
    let runtime = state.runtime.as_mut().expect("runtime");
    runtime.phase = RemoteRuntimePhase::Ready;
    runtime.ssh = Some(InteractiveWorkerSshEndpoint {
        host: "127.0.0.1".into(),
        port: 2222,
        username: "horizon".into(),
        host_key: worker.ssh_public_key.clone(),
    });
    runtime.worker = Some(worker);
    let running = store
        .replace_remote_workspace(saved.workspace(), &state)
        .expect("running fixture");
    let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen");
    assert!(
        reopened
            .list_retained(current_unix_millis().expect("now"))
            .expect("retained setup")
            .is_empty()
    );
    assert_eq!(fixture.recover().workspace(), &running);
    assert!(matches!(
        claim(&reopened, &saved),
        Err(CloudStoreError::WorkflowExpired(_))
    ));
    let mut extended = saved.workflow().workflow().clone();
    extended.updated_at_millis += 1;
    extended.retain_until_millis = i64::MAX;
    assert!(matches!(
        reopened.replace(saved.workflow(), &extended),
        Err(CloudStoreError::InvalidRemoteAllocation)
    ));
    state.checkpoint = Some(RepositoryCheckpoint {
        workspace_local_id: "workspace".into(),
        base_commit: state.spec.repository.commit.clone(),
        manifest_digest: ArtifactDigest::parse_sha256("c".repeat(64)).expect("digest"),
        runtime_generation: 1,
        generation: 1,
        captured_at_millis: 3000,
        recovery_artifact: None,
    });
    let checkpointed = reopened
        .replace_remote_workspace(&running, &state)
        .expect("checkpoint after setup expiry");
    assert_eq!(checkpointed.state().runtime, running.state().runtime);
    assert_eq!(fixture.recover().workspace(), &checkpointed);
    assert!(matches!(
        reopened.allocate_remote_runtime(&checkpointed, i64::MAX),
        Err(Error::RuntimeAlreadyActive)
    ));
    assert_eq!(fixture.counts(), [1, 1, 0]);
}

#[test]
fn cleanup_reconciling_and_observed_records_remain_recoverable_without_new_grants() {
    enum Observation {
        Cleanup(RemoteCleanupReason),
        Phase(RemoteRuntimePhase),
        Worker,
    }
    for observation in [
        Observation::Cleanup(RemoteCleanupReason::LastPanelClosed),
        Observation::Cleanup(RemoteCleanupReason::WorkspaceRemoved),
        Observation::Cleanup(RemoteCleanupReason::ApplicationExit),
        Observation::Cleanup(RemoteCleanupReason::Cancelled),
        Observation::Phase(RemoteRuntimePhase::Reconciling),
        Observation::Phase(RemoteRuntimePhase::Failed),
        Observation::Worker,
    ] {
        let fixture = fixture();
        let store = &fixture.store;
        let saved = fixture.allocate();
        let mut state = saved.workspace().state().clone();
        let worker = observed_worker(&state);
        let runtime = state.runtime.as_mut().expect("runtime");
        match observation {
            Observation::Cleanup(reason) => {
                runtime.cleanup = Some(RemoteCleanupIntent {
                    reason,
                    requested_at_millis: 2000,
                });
            }
            Observation::Phase(phase) => runtime.phase = phase,
            Observation::Worker => runtime.worker = Some(worker),
        }
        let retained = store
            .replace_remote_workspace(saved.workspace(), &state)
            .expect("observation");
        assert_eq!(fixture.recover().workspace(), &retained);
        assert!(matches!(
            claim(store, &saved),
            Err(CloudStoreError::ClaimTargetNotReady(_))
        ));
        if state.runtime.as_ref().expect("runtime").phase != RemoteRuntimePhase::Provisioning {
            state.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Provisioning;
            assert!(matches!(
                store.replace_remote_workspace(&retained, &state),
                Err(Error::NonMonotonicReplacement)
            ));
        }
        assert_eq!(fixture.counts(), [1, 1, 0]);
    }
}

#[test]
fn a_setup_deadline_that_expires_waiting_for_the_write_lock_never_allocates() {
    let fixture = fixture();
    let store = fixture.store.clone();
    let (ready, locked) = std::sync::mpsc::channel();
    let blocker = std::thread::spawn(move || {
        let mut connection = store.connection().expect("blocking connection");
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("hold allocation lock");
        ready.send(current_unix_millis().expect("clock") + 250).expect("locked");
        std::thread::sleep(std::time::Duration::from_millis(500));
        transaction.commit().expect("release lock");
    });
    let deadline = locked
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("write lock held");
    let result = fixture.store.allocate_remote_runtime(&fixture.original, deadline);
    blocker.join().expect("lock holder");
    assert!(matches!(result, Err(Error::InvalidAllocationRetention)));
    assert_eq!(fixture.counts(), [0, 0, 0]);
    assert_eq!(
        fixture
            .store
            .load_remote_workspace(OWNER, "workspace")
            .expect("unchanged"),
        Some(fixture.original)
    );
}

mod first_pin {
    use super::*;
    use crate::cloud_run::interactive_worker::{InteractiveWorkerLifecycle, InteractiveWorkerStatus};

    fn pending() -> (Fixture, StoredRemoteAllocation) {
        let mut fixture = fixture();
        let mut state = fixture.original.state().clone();
        state.spec.target.provider = CloudProvider::RunPod;
        fixture.original = fixture
            .store
            .replace_remote_workspace(&fixture.original, &state)
            .expect("target");
        let allocation = fixture.allocate();
        (fixture, allocation)
    }

    pub(super) fn reserved() -> (Fixture, StoredRemoteAllocation) {
        let (fixture, allocation) = pending();
        let key = observed_worker(allocation.workspace().state()).ssh_public_key;
        let saved = fixture
            .store
            .reserve_remote_worker_request(&allocation, &key)
            .expect("request");
        (fixture, saved)
    }

    pub(super) fn intent_count(store: &CloudWorkflowStore) -> i64 {
        Connection::open(store.path())
            .expect("fixture connection")
            .query_row("SELECT COUNT(*) FROM remote_first_pin_intents", [], |row| row.get(0))
            .expect("intent count")
    }

    pub(super) fn status(saved: &StoredRemoteAllocation, ready: bool) -> InteractiveWorkerStatus {
        let worker = observed_worker(saved.workspace().state());
        let ssh = ready.then(|| InteractiveWorkerSshEndpoint {
            host: "127.0.0.1".into(),
            port: 22,
            username: "root".into(),
            host_key: worker.ssh_public_key.clone(),
        });
        InteractiveWorkerStatus {
            worker,
            ssh,
            lifecycle: if ready {
                InteractiveWorkerLifecycle::Ready
            } else {
                InteractiveWorkerLifecycle::Provisioning
            },
        }
    }

    fn expire(fixture: &Fixture) -> StoredRemoteAllocation {
        let mut workflow = fixture.recover().workflow().workflow().clone();
        workflow.created_at_millis = 1000;
        workflow.updated_at_millis = 1000;
        workflow.retain_until_millis = 2000;
        Connection::open(fixture.store.path())
            .expect("fixture connection")
            .execute(
                "UPDATE cloud_workflows SET created_at_millis=1000, updated_at_millis=1000,
             retain_until_millis=2000, snapshot=?1 WHERE workflow_id=?2",
                params![encode_workflow(&workflow).expect("snapshot"), workflow.id.to_string()],
            )
            .expect("expired fixture");
        fixture.recover()
    }

    #[test]
    fn first_pin_intent_requires_reserved_identity_and_never_consumes_or_renews_creation() {
        let (fixture, allocation) = pending();
        let store = &fixture.store;
        assert!(matches!(
            store.record_remote_first_pin_intent(&allocation),
            Err(Error::RuntimeRequestRequired)
        ));
        assert_eq!(intent_count(store), 0);
        let saved = store
            .reserve_remote_worker_request(
                &allocation,
                &observed_worker(allocation.workspace().state()).ssh_public_key,
            )
            .expect("reserve");
        assert!(matches!(
            store.record_remote_first_pin_intent(&allocation),
            Err(Error::SnapshotConflict)
        ));
        assert_eq!(store.load_remote_first_pin_request(&saved).expect("unmarked"), None);
        store.record_remote_first_pin_intent(&saved).expect("explicit intent");
        store
            .record_remote_first_pin_intent(&saved)
            .expect("idempotent pre-claim intent");
        assert_eq!(
            store.load_remote_first_pin_request(&saved).expect("positive"),
            Some(saved.worker_request().expect("request"))
        );
        assert_eq!(fixture.recover(), saved);
        assert_eq!(fixture.counts(), [1, 1, 0]);
        assert_eq!(intent_count(store), 1);
        assert!(claim(store, &saved).expect("first claim"));
        assert!(!claim(store, &saved).expect("one shot"));
        assert!(matches!(
            store.record_remote_first_pin_intent(&saved),
            Err(Error::RuntimeSetupUnavailable)
        ));
        assert_eq!(fixture.counts(), [1, 1, 1]);
        assert_eq!(intent_count(store), 1);
    }

    #[test]
    fn first_pin_intent_survives_lost_response_provisioning_and_restart_then_is_consumed() {
        let (fixture, saved) = reserved();
        fixture.store.record_remote_first_pin_intent(&saved).expect("intent");
        assert!(claim(&fixture.store, &saved).expect("claim"));
        let lost = fixture
            .store
            .record_remote_worker_recovery(&saved, None)
            .expect("lost response");
        assert!(
            lost.workspace()
                .state()
                .runtime
                .as_ref()
                .expect("runtime")
                .worker
                .is_none()
        );
        let store = CloudWorkflowStore::open_path(fixture.store.path()).expect("restart");
        assert_eq!(
            store.load_remote_first_pin_request(&lost).expect("restart intent"),
            Some(saved.worker_request().expect("request"))
        );
        let provisioned = store
            .record_remote_worker_recovery(&lost, Some(&status(&lost, false)))
            .expect("provisioning");
        assert_eq!(intent_count(&store), 1);
        assert!(
            store
                .load_remote_first_pin_request(&provisioned)
                .expect("pending pin")
                .is_some()
        );
        assert!(matches!(
            store.record_remote_first_pin_intent(&provisioned),
            Err(Error::RuntimeSetupUnavailable)
        ));
        let pinned = store
            .record_remote_worker_recovery(&provisioned, Some(&status(&provisioned, true)))
            .expect("first pin");
        let runtime = pinned.workspace().state().runtime.as_ref().expect("runtime");
        assert!(runtime.ssh.is_some());
        assert_eq!(runtime.phase, RemoteRuntimePhase::Reconciling);
        assert_eq!(intent_count(&store), 0);
        assert_eq!(
            store.load_remote_first_pin_request(&pinned).expect("retained trust"),
            None
        );
        assert!(matches!(
            store.record_remote_first_pin_intent(&pinned),
            Err(Error::RuntimeSetupUnavailable)
        ));
        assert_eq!(
            store
                .record_remote_worker_recovery(&pinned, None)
                .expect("later absence"),
            pinned
        );
        assert_eq!(fixture.counts(), [1, 1, 1]);
        assert_eq!(intent_count(&store), 0);
    }

    #[test]
    fn first_pin_snapshot_and_intent_roll_back_together_after_real_write_failure() {
        let (fixture, saved) = reserved();
        let store = &fixture.store;
        store.record_remote_first_pin_intent(&saved).expect("intent");
        let connection = Connection::open(store.path()).expect("fixture connection");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_first_pin BEFORE UPDATE ON remote_workspaces
            BEGIN SELECT RAISE(ABORT, 'synthetic snapshot failure'); END",
            )
            .expect("failure injection");
        let ready = status(&saved, true);
        assert!(matches!(
            store.record_remote_worker_recovery(&saved, Some(&ready)),
            Err(Error::Database(_))
        ));
        assert_eq!(fixture.recover(), saved);
        assert_eq!(intent_count(store), 1);
        assert_eq!(fixture.counts(), [1, 1, 0]);
        connection
            .execute_batch("DROP TRIGGER fail_first_pin")
            .expect("remove injection");
        let pinned = store
            .record_remote_worker_recovery(&saved, Some(&ready))
            .expect("retry pin");
        assert_eq!(intent_count(store), 0);
        assert!(matches!(
            store.record_remote_worker_recovery(&saved, Some(&ready)),
            Err(Error::SnapshotConflict)
        ));
        let mut changed_pin = ready;
        changed_pin.ssh.as_mut().expect("ssh").host = "127.0.0.2".into();
        assert!(matches!(
            store.record_remote_worker_recovery(&pinned, Some(&changed_pin)),
            Err(Error::InvalidWorkerObservation)
        ));
        assert_eq!(fixture.recover(), pinned);
        assert_eq!(intent_count(store), 0);
    }

    #[test]
    fn first_pin_intent_refuses_late_setup_but_survives_its_original_setup_deadline() {
        for retain_intent in [false, true] {
            let (fixture, saved) = reserved();
            if retain_intent {
                fixture
                    .store
                    .record_remote_first_pin_intent(&saved)
                    .expect("initial intent");
            }
            let expired = expire(&fixture);
            assert!(matches!(
                fixture.store.record_remote_first_pin_intent(&expired),
                Err(Error::RuntimeSetupUnavailable)
            ));
            assert_eq!(
                fixture
                    .store
                    .load_remote_first_pin_request(&expired)
                    .expect("read-only expiry"),
                retain_intent.then(|| saved.worker_request().expect("request"))
            );
            assert_eq!(fixture.recover(), expired);
            assert_eq!(intent_count(&fixture.store), i64::from(retain_intent));
            assert!(matches!(
                claim(&fixture.store, &expired),
                Err(CloudStoreError::WorkflowExpired(_))
            ));
        }
        for observed in [false, true] {
            let (fixture, saved) = reserved();
            let current = if observed {
                fixture
                    .store
                    .record_remote_worker_recovery(&saved, Some(&status(&saved, false)))
                    .expect("legacy observation")
            } else {
                assert!(claim(&fixture.store, &saved).expect("legacy claim"));
                saved
            };
            assert!(matches!(
                fixture.store.record_remote_first_pin_intent(&current),
                Err(Error::RuntimeSetupUnavailable)
            ));
            assert_eq!(
                fixture
                    .store
                    .load_remote_first_pin_request(&current)
                    .expect("no inferred authority"),
                None
            );
            assert_eq!(intent_count(&fixture.store), 0);
        }
    }

    #[test]
    fn first_pin_lookup_matches_both_snapshots_and_does_not_authorize_generic_replacement() {
        let (fixture, saved) = reserved();
        let store = &fixture.store;
        store.record_remote_first_pin_intent(&saved).expect("intent");
        let mut next = saved.workflow().workflow().clone();
        next.title = "Updated display title".into();
        next.updated_at_millis += 1;
        store.replace(saved.workflow(), &next).expect("workflow-only revision");
        assert!(matches!(
            store.load_remote_first_pin_request(&saved),
            Err(Error::SnapshotConflict)
        ));
        assert!(matches!(
            store.record_remote_first_pin_intent(&saved),
            Err(Error::SnapshotConflict)
        ));
        let current = fixture.recover();
        assert!(
            store
                .load_remote_first_pin_request(&current)
                .expect("fresh snapshots")
                .is_some()
        );
        for retire in [false, true] {
            let mut state = current.workspace().state().clone();
            if retire {
                state.runtime = None;
            } else {
                state.spec.generation += 1;
                state.runtime.as_mut().expect("runtime").generation += 1;
            }
            assert!(store.replace_remote_workspace(current.workspace(), &state).is_err());
        }
        assert!(matches!(
            store.allocate_remote_runtime(current.workspace(), i64::MAX),
            Err(Error::RuntimeAlreadyActive)
        ));
        let (_, foreign) = reserved();
        assert!(matches!(
            store.load_remote_first_pin_request(&foreign),
            Err(Error::SnapshotConflict)
        ));
        assert!(matches!(
            store.record_remote_first_pin_intent(&foreign),
            Err(Error::SnapshotConflict)
        ));
        assert_eq!(fixture.recover(), current);
        assert_eq!(intent_count(store), 1);
        assert_eq!(fixture.counts(), [1, 1, 0]);
    }

    #[test]
    fn first_pin_lookup_rejects_malformed_matching_rows_and_never_adopts_orphans() {
        for alteration in [
            "session_id='22222222-2222-4222-8222-222222222222'",
            "generation=2",
            "workflow_id='33333333-3333-4333-8333-333333333333'",
            "job_id='44444444-4444-4444-8444-444444444444'",
            "version=2",
            "request_digest=printf('%064d', 0)",
            "session_id=printf('%4096s', 'x')",
        ] {
            let (fixture, saved) = reserved();
            fixture.store.record_remote_first_pin_intent(&saved).expect("intent");
            let connection = Connection::open(fixture.store.path()).expect("fixture connection");
            connection
                .execute_batch(&format!(
                    "PRAGMA ignore_check_constraints=ON; UPDATE remote_first_pin_intents SET {alteration}"
                ))
                .expect("corrupt intent");
            assert!(
                matches!(
                    fixture.store.load_remote_first_pin_request(&saved),
                    Err(Error::Storage(CloudStoreError::InvalidRemoteAllocation))
                ),
                "{alteration}"
            );
            assert!(
                matches!(
                    fixture.store.record_remote_first_pin_intent(&saved),
                    Err(Error::Storage(CloudStoreError::InvalidRemoteAllocation))
                ),
                "{alteration}"
            );
            assert_eq!(fixture.recover(), saved);
            assert_eq!(fixture.counts(), [1, 1, 0]);
        }
        let (fixture, saved) = reserved();
        fixture.store.record_remote_first_pin_intent(&saved).expect("intent");
        Connection::open(fixture.store.path())
            .expect("fixture connection")
            .execute_batch("PRAGMA foreign_keys=OFF; UPDATE remote_first_pin_intents SET workspace_local_id='orphan'")
            .expect("orphan");
        assert_eq!(
            fixture
                .store
                .load_remote_first_pin_request(&saved)
                .expect("orphan ignored"),
            None
        );
        assert!(fixture.store.record_remote_first_pin_intent(&saved).is_err());
        assert_eq!(intent_count(&fixture.store), 1);
    }

    #[test]
    fn first_pin_migration_never_backfills_legacy_allocations_or_changes_snapshots_and_claims() {
        let (fixture, saved) = reserved();
        assert!(claim(&fixture.store, &saved).expect("legacy claim"));
        let snapshots = || {
            Connection::open(fixture.store.path())
                .expect("fixture connection")
                .query_row(
                    "SELECT (SELECT snapshot FROM remote_workspaces), (SELECT snapshot FROM cloud_workflows)",
                    [],
                    |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
                )
                .expect("raw snapshots")
        };
        let before = snapshots();
        Connection::open(fixture.store.path())
            .expect("fixture connection")
            .execute_batch("DROP TABLE remote_first_pin_intents; PRAGMA user_version=4")
            .expect("schema-four fixture");
        assert!(matches!(
            CloudWorkflowStore::open_read_only_path(fixture.store.path()),
            Err(CloudStoreError::UnsupportedSchema(4))
        ));
        assert!(matches!(
            fixture.store.load_remote_first_pin_request(&saved),
            Err(Error::Storage(CloudStoreError::UnsupportedSchema(4)))
        ));
        let connection = Connection::open(fixture.store.path()).expect("fixture connection");
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("unchanged version"),
            4
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE name='remote_first_pin_intents'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .expect("no read repair"),
            0
        );
        let reopened = CloudWorkflowStore::open_path(fixture.store.path()).expect("explicit migration");
        assert_eq!(snapshots(), before);
        assert_eq!(fixture.recover(), saved);
        assert_eq!(fixture.counts(), [1, 1, 1]);
        assert_eq!(intent_count(&reopened), 0);
        assert_eq!(
            reopened
                .load_remote_first_pin_request(&saved)
                .expect("no legacy bootstrap"),
            None
        );
        assert!(!claim(&reopened, &saved).expect("claim preserved"));
    }

    #[test]
    fn first_pin_and_creation_race_cannot_publish_a_late_intent() {
        let (fixture, saved) = reserved();
        let barrier = Arc::new(Barrier::new(2));
        let claimant = {
            let barrier = Arc::clone(&barrier);
            let store = fixture.store.clone();
            let saved = saved.clone();
            std::thread::spawn(move || {
                barrier.wait();
                claim(&store, &saved).expect("claim")
            })
        };
        barrier.wait();
        let result = fixture.store.record_remote_first_pin_intent(&saved);
        assert!(claimant.join().expect("claimant"));
        match result {
            Ok(()) => assert_eq!(intent_count(&fixture.store), 1),
            Err(Error::RuntimeSetupUnavailable) => assert_eq!(intent_count(&fixture.store), 0),
            Err(error) => panic!("unexpected intent result: {error}"),
        }
        assert!(matches!(
            fixture.store.record_remote_first_pin_intent(&saved),
            Err(Error::RuntimeSetupUnavailable)
        ));
        assert!(!claim(&fixture.store, &saved).expect("no repeated grant"));
        assert_eq!(fixture.recover(), saved);
        assert_eq!(fixture.counts(), [1, 1, 1]);
    }
}
