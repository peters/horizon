//! Reserve the non-secret request identity before crossing the provider boundary.

use super::super::{current_unix_millis, database::ensure_current_schema, validate_claim_target};
use super::{CloudWorkflowStore, Error, RemoteRuntimePhase, StoredRemoteAllocation, WorkspaceReplacement, binding};
use crate::cloud_run::interactive_worker::InteractiveWorkerRequest;
use rusqlite::{TransactionBehavior, params};

const CLAIM_LOOKUP: &str = "SELECT EXISTS(SELECT 1 FROM cloud_worker_creation_claims
    INDEXED BY cloud_worker_creation_claims_workflow WHERE workflow_id = ?1)";

impl StoredRemoteAllocation {
    /// Reconstruct a request from its reserved key or a legacy, exact observed worker.
    /// This does not authorize creation, attachment, or any provider operation.
    /// # Errors
    /// Rejects invalid snapshots or an allocation without a recorded client identity.
    pub fn worker_request(&self) -> Result<InteractiveWorkerRequest, Error> {
        let state = self.workspace.state();
        state.validate()?;
        let runtime = state.runtime.as_ref().ok_or(Error::UnboundRuntime)?;
        let key = runtime
            .ssh_public_key
            .as_ref()
            .or_else(|| runtime.worker.as_ref().map(|worker| &worker.ssh_public_key))
            .ok_or(Error::RuntimeRequestRequired)?;
        Ok(InteractiveWorkerRequest {
            workflow_id: runtime.workflow_id,
            job_id: runtime.job_id,
            target: state.spec.target.clone(),
            ssh_public_key: key.clone(),
        })
    }
}

impl CloudWorkflowStore {
    /// Commit a canonical public client key to the exact allocation before creation.
    /// The caller must already have durably retained the corresponding private key.
    /// That private material never enters snapshots or this API. Run off the render thread.
    ///
    /// Reservation neither consumes nor renews a creation grant. A competing caller
    /// reloads the winner; the same key is an idempotent read even after setup expires.
    /// Missing identity after a claim, observation, or cancellation fails closed.
    /// # Errors
    /// Rejects stale/corrupt bindings, invalid or changed keys, late reservation,
    /// expired setup, and storage failures without performing provider I/O.
    pub fn reserve_remote_worker_request(
        &self,
        expected: &StoredRemoteAllocation,
        ssh_public_key: &str,
    ) -> Result<StoredRemoteAllocation, Error> {
        let mut next = expected.workspace.state().clone();
        let runtime = next.runtime.as_mut().ok_or(Error::UnboundRuntime)?;
        if runtime.ssh_public_key.as_ref().is_some_and(|key| key != ssh_public_key) {
            return Err(Error::ReplacementIdentityMismatch);
        }
        runtime.ssh_public_key = Some(ssh_public_key.into());
        let replacement = WorkspaceReplacement::new(&expected.workspace, &next)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_current_schema(&transaction)?;
        let row = binding::load_workspace(&transaction, &next.spec.workspace_local_id)?.ok_or(Error::UnboundRuntime)?;
        let current = binding::recover(&transaction, &row)?;
        if current != *expected {
            return Err(Error::SnapshotConflict);
        }
        let runtime = current
            .workspace
            .state()
            .runtime
            .as_ref()
            .ok_or(Error::UnboundRuntime)?;
        if runtime.ssh_public_key.as_deref() == Some(ssh_public_key) {
            return Ok(current);
        }
        if runtime.phase != RemoteRuntimePhase::Provisioning
            || runtime.worker.is_some()
            || runtime.cleanup.is_some()
            || validate_claim_target(current.workflow.workflow(), runtime.job_id, &next.spec.target).is_err()
        {
            return Err(Error::RuntimeRequestUnavailable);
        }
        if current.workflow.workflow().retain_until_millis < current_unix_millis()? {
            return Err(Error::RuntimeRequestUnavailable);
        }
        let claimed = transaction.query_row(CLAIM_LOOKUP, params![runtime.workflow_id.to_string()], |row| {
            row.get::<_, bool>(0)
        })?;
        if claimed {
            return Err(Error::RuntimeRequestUnavailable);
        }
        let workspace = replacement.persist(&transaction)?;
        transaction.commit()?;
        Ok(StoredRemoteAllocation {
            workspace,
            workflow: current.workflow,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_run::store::{CloudStoreError, encode_workflow};
    use crate::remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason, RemoteWorkspaceState};
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::sync::{Arc, Barrier};

    const OWNER: &str = "00000000-0000-4000-8000-000000000001";

    fn public_key(byte: u8) -> String {
        let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
        blob.extend([byte; 32]);
        format!("ssh-ed25519 {}", STANDARD.encode(blob))
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        store: CloudWorkflowStore,
        allocation: StoredRemoteAllocation,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().expect("temporary store");
            let store =
                CloudWorkflowStore::open_path(directory.path().join("control/workflows.sqlite3")).expect("store");
            let state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
                "version": 1,
                "spec": {
                    "workspace_local_id": "workspace", "working_directory": ".", "generation": 0, "panels": [],
                    "target": { "provider": "local_docker", "profile": "development",
                        "image": format!("example/worker@sha256:{}", "a".repeat(64)),
                        "disk_gib": 20, "lifetime": "persistent" },
                    "repository": { "repository": "example/project", "commit": "b".repeat(40) }
                }
            }))
            .expect("state");
            let dormant = store.create_remote_workspace(OWNER, &state).expect("record");
            let allocation = store.allocate_remote_runtime(&dormant, i64::MAX).expect("allocation");
            Self {
                _directory: directory,
                store,
                allocation,
            }
        }

        fn reload(&self) -> StoredRemoteAllocation {
            self.store
                .load_remote_allocation(OWNER, "workspace")
                .expect("load")
                .expect("allocation")
        }

        fn claim(&self) -> bool {
            let workflow = self.allocation.workflow().workflow();
            self.store
                .claim_worker_creation(
                    workflow.id,
                    workflow.nodes[0].id,
                    &self.allocation.workspace().state().spec.target,
                    "synthetic-worker",
                )
                .expect("claim")
        }

        fn expire(&self) {
            let mut workflow = self.reload().workflow().workflow().clone();
            workflow.created_at_millis = 1000;
            workflow.updated_at_millis = 1000;
            workflow.retain_until_millis = 2000;
            rusqlite::Connection::open(self.store.path())
                .expect("fixture connection")
                .execute(
                    "UPDATE cloud_workflows SET created_at_millis=1000, updated_at_millis=1000,
                 retain_until_millis=2000, snapshot=?1 WHERE workflow_id=?2",
                    params![encode_workflow(&workflow).expect("snapshot"), workflow.id.to_string()],
                )
                .expect("expired fixture");
        }
    }

    #[test]
    fn request_is_durable_before_creation_and_same_key_does_not_rewrite_it() {
        let fixture = Fixture::new();
        let before = fixture.allocation.workspace();
        assert!(
            !serde_json::to_string(before.state())
                .expect("legacy shape")
                .contains("ssh_public_key")
        );
        assert!(matches!(
            fixture.allocation.worker_request(),
            Err(Error::RuntimeRequestRequired)
        ));
        let saved = fixture
            .store
            .reserve_remote_worker_request(&fixture.allocation, &public_key(1))
            .expect("reserve");
        assert_eq!(saved.workspace().revision(), before.revision() + 1);
        assert_eq!(saved.workflow(), fixture.allocation.workflow());
        let request = saved.worker_request().expect("request");
        assert_eq!(request.ssh_public_key, public_key(1));
        assert_eq!(request.workflow_id, saved.workflow().workflow().id);
        assert!(saved.workspace().state().spec.panels.is_empty());
        let reopened = CloudWorkflowStore::open_path(fixture.store.path()).expect("reopen");
        let recovered = reopened
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("saved");
        assert_eq!(recovered, saved);
        assert_eq!(
            reopened
                .reserve_remote_worker_request(&saved, &public_key(1))
                .expect("same key"),
            saved
        );
        assert!(fixture.claim(), "reservation must not consume the one-shot grant");
        assert!(!fixture.claim());
        assert_eq!(fixture.reload().worker_request().expect("claimed recovery"), request);
    }

    #[test]
    fn competing_keys_have_one_winner_and_stale_writers_cannot_change_it() {
        let fixture = Fixture::new();
        let barrier = Arc::new(Barrier::new(3));
        let writers: Vec<_> = [1, 2]
            .into_iter()
            .map(|byte| {
                let store = fixture.store.clone();
                let expected = fixture.allocation.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store.reserve_remote_worker_request(&expected, &public_key(byte))
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
                .filter(|result| matches!(result, Err(Error::SnapshotConflict)))
                .count(),
            1
        );
        let winner = results.into_iter().find_map(Result::ok).expect("winner");
        assert_eq!(fixture.reload(), winner);
        assert!(matches!(
            fixture
                .store
                .reserve_remote_worker_request(&fixture.allocation, &public_key(1)),
            Err(Error::SnapshotConflict)
        ));
        assert!(matches!(
            fixture.store.reserve_remote_worker_request(&winner, &public_key(3)),
            Err(Error::ReplacementIdentityMismatch)
        ));
    }

    #[test]
    fn generic_writes_cannot_install_replace_or_remove_request_identity() {
        let fixture = Fixture::new();
        let mut next = fixture.allocation.workspace().state().clone();
        next.runtime.as_mut().expect("runtime").ssh_public_key = Some(public_key(1));
        assert!(matches!(
            fixture
                .store
                .replace_remote_workspace(fixture.allocation.workspace(), &next),
            Err(Error::RuntimeRequestRequired)
        ));
        let saved = fixture
            .store
            .reserve_remote_worker_request(&fixture.allocation, &public_key(1))
            .expect("reserve");
        for key in [None, Some(public_key(2))] {
            next.runtime.as_mut().expect("runtime").ssh_public_key = key;
            assert!(matches!(
                fixture.store.replace_remote_workspace(saved.workspace(), &next),
                Err(Error::ReplacementIdentityMismatch)
            ));
        }
        assert_eq!(fixture.reload(), saved);
    }

    #[test]
    fn late_reservation_cannot_guess_identity_after_creation_or_setup_observation() {
        for case in 0..4 {
            let fixture = Fixture::new();
            let mut next = fixture.allocation.workspace().state().clone();
            match case {
                0 => assert!(fixture.claim()),
                1 => next.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Reconciling,
                2 => {
                    next.runtime.as_mut().expect("runtime").cleanup = Some(RemoteCleanupIntent {
                        reason: RemoteCleanupReason::Cancelled,
                        requested_at_millis: 1000,
                    });
                }
                _ => {
                    let mut workflow = fixture.allocation.workflow().workflow().clone();
                    workflow.updated_at_millis += 1;
                    workflow.nodes[0].state = crate::cloud_run::CloudJobState::Running;
                    fixture
                        .store
                        .replace(fixture.allocation.workflow(), &workflow)
                        .expect("observed workflow");
                }
            }
            if matches!(case, 1 | 2) {
                fixture
                    .store
                    .replace_remote_workspace(fixture.allocation.workspace(), &next)
                    .expect("observation");
            }
            let observed = fixture.reload();
            assert!(matches!(
                fixture.store.reserve_remote_worker_request(&observed, &public_key(1)),
                Err(Error::RuntimeRequestUnavailable)
            ));
            assert_eq!(fixture.reload(), observed);
        }
    }

    #[test]
    fn expiry_preserves_reserved_recovery_but_never_admits_new_identity() {
        for reserved in [false, true] {
            let fixture = Fixture::new();
            if reserved {
                fixture
                    .store
                    .reserve_remote_worker_request(&fixture.allocation, &public_key(1))
                    .expect("reserve");
            }
            fixture.expire();
            let observed = fixture.reload();
            let result = fixture.store.reserve_remote_worker_request(&observed, &public_key(1));
            if reserved {
                assert_eq!(result.expect("idempotent recovery"), observed);
                assert_eq!(
                    observed.worker_request().expect("request").ssh_public_key,
                    public_key(1)
                );
            } else {
                assert!(matches!(result, Err(Error::RuntimeRequestUnavailable)));
            }
            assert_eq!(fixture.reload(), observed);
        }
    }

    #[test]
    fn malformed_keys_and_exhausted_revisions_leave_the_record_unchanged() {
        let fixture = Fixture::new();
        for key in [
            String::new(),
            "private-test-payload".into(),
            "ssh-ed25519 invalid".into(),
            format!("{} private-test-comment", public_key(1)),
            format!(" {}", public_key(1)),
        ] {
            let error = fixture
                .store
                .reserve_remote_worker_request(&fixture.allocation, &key)
                .expect_err("invalid key");
            assert!(!error.to_string().contains("private-test"));
            assert_eq!(fixture.reload(), fixture.allocation);
        }
        for reserved in [false, true] {
            let fixture = Fixture::new();
            if reserved {
                fixture
                    .store
                    .reserve_remote_worker_request(&fixture.allocation, &public_key(1))
                    .expect("reserve before exhaustion");
            }
            rusqlite::Connection::open(fixture.store.path())
                .expect("fixture connection")
                .execute(
                    "UPDATE remote_workspaces SET revision=?1 WHERE workspace_local_id='workspace'",
                    [i64::MAX],
                )
                .expect("exhaust revision");
            let exhausted = fixture.reload();
            let result = fixture.store.reserve_remote_worker_request(&exhausted, &public_key(1));
            if reserved {
                assert_eq!(result.expect("read-only reservation recovery"), exhausted);
            } else {
                assert!(matches!(result, Err(Error::RevisionExhausted)));
            }
            assert_eq!(fixture.reload(), exhausted);
        }
    }

    #[test]
    fn missing_or_corrupt_binding_never_adopts_a_request() {
        for sql in [
            "DELETE FROM remote_runtime_allocations",
            "UPDATE remote_runtime_allocations SET session_id='00000000-0000-4000-8000-000000000002'",
        ] {
            let fixture = Fixture::new();
            rusqlite::Connection::open(fixture.store.path())
                .expect("fixture connection")
                .execute(sql, [])
                .expect("fault");
            assert!(matches!(
                fixture
                    .store
                    .reserve_remote_worker_request(&fixture.allocation, &public_key(1)),
                Err(Error::UnboundRuntime | Error::Storage(CloudStoreError::InvalidRemoteAllocation))
            ));
            assert_eq!(
                fixture.store.load_remote_workspace(OWNER, "workspace").expect("record"),
                Some(fixture.allocation.workspace().clone())
            );
        }
    }

    #[test]
    fn observed_worker_must_match_reserved_key_and_legacy_recovery_never_invents_one() {
        for reserved in [false, true] {
            let fixture = Fixture::new();
            let saved = if reserved {
                fixture
                    .store
                    .reserve_remote_worker_request(&fixture.allocation, &public_key(1))
                    .expect("reserve")
            } else {
                fixture.allocation.clone()
            };
            let mut next = saved.workspace().state().clone();
            let runtime = next.runtime.as_mut().expect("runtime");
            runtime.worker = Some(
                serde_json::from_value(serde_json::json!({
                    "identity": { "provider": "local_docker", "workflow_id": runtime.workflow_id,
                        "job_id": runtime.job_id, "resource_id": "synthetic-worker" },
                    "target": next.spec.target, "ssh_public_key": public_key(1),
                    "lease": { "lifetime": "persistent" }
                }))
                .expect("observed worker"),
            );
            fixture
                .store
                .replace_remote_workspace(saved.workspace(), &next)
                .expect("observation");
            let observed = fixture.reload();
            assert_eq!(
                observed.worker_request().expect("recovered request").ssh_public_key,
                public_key(1)
            );
            if reserved {
                next.runtime
                    .as_mut()
                    .expect("runtime")
                    .worker
                    .as_mut()
                    .expect("worker")
                    .ssh_public_key = public_key(2);
                assert!(next.validate().is_err());
            } else {
                assert!(matches!(
                    fixture.store.reserve_remote_worker_request(&observed, &public_key(1)),
                    Err(Error::RuntimeRequestUnavailable)
                ));
                assert!(
                    observed
                        .workspace()
                        .state()
                        .runtime
                        .as_ref()
                        .expect("runtime")
                        .ssh_public_key
                        .is_none()
                );
            }
            assert_eq!(fixture.reload(), observed);
        }
    }

    #[test]
    fn claim_lookup_uses_the_indexed_single_workflow_identity() {
        let fixture = Fixture::new();
        let connection = rusqlite::Connection::open(fixture.store.path()).expect("connection");
        let mut statement = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {CLAIM_LOOKUP}"))
            .expect("query plan");
        let details: Vec<String> = statement
            .query_map([fixture.allocation.workflow().workflow().id.to_string()], |row| {
                row.get(3)
            })
            .expect("plan")
            .collect::<Result<_, _>>()
            .expect("details");
        assert!(details.iter().any(|detail| detail.contains("SEARCH cloud_worker_creation_claims USING COVERING INDEX cloud_worker_creation_claims_workflow (workflow_id=?)")));
        assert!(
            !details
                .iter()
                .any(|detail| detail.contains("SCAN cloud_worker_creation_claims"))
        );
    }
}
