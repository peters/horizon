use super::super::{
    current_unix_millis,
    remote_workspaces::tests::{OTHER_OWNER, OWNER, provisioning, store, workspace as timed_workspace},
};
use super::*;
use crate::cloud_run::{
    WorkerLifetime,
    interactive_worker::{InteractiveWorker, InteractiveWorkerIdentity, InteractiveWorkerLifetime},
};
use crate::remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rusqlite::Connection;
use std::sync::{Arc, Barrier};

fn workspace(id: &str) -> RemoteWorkspaceState {
    let mut state = timed_workspace(id);
    state.spec.target.lifetime = WorkerLifetime::Persistent;
    state
}

fn observed_worker(state: &RemoteWorkspaceState) -> InteractiveWorker {
    let runtime = state.runtime.as_ref().expect("runtime");
    let mut key = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
    key.extend([7; 32]);
    assert_eq!(state.spec.target.lifetime, WorkerLifetime::Persistent);
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

fn allocate(store: &CloudWorkflowStore, expected: &StoredRemoteWorkspace) -> StoredRemoteAllocation {
    let now = current_unix_millis().expect("test timestamp");
    store
        .allocate_remote_runtime(expected, now, now + 86_400_000)
        .expect("allocate")
}

fn dormant(store: &CloudWorkflowStore, id: &str) -> StoredRemoteWorkspace {
    store
        .create_remote_workspace(OWNER, &workspace(id))
        .expect("dormant workspace")
}

fn counts(store: &CloudWorkflowStore) -> (i64, i64, i64) {
    Connection::open(store.path())
        .expect("raw store")
        .query_row(
            "SELECT (SELECT COUNT(*) FROM cloud_workflows),
                (SELECT COUNT(*) FROM remote_runtime_allocations),
                (SELECT COUNT(*) FROM cloud_worker_creation_claims)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("counts")
}

mod allocation;
mod guards;
mod migration;
mod persistence;
mod recovery;
