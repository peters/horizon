use super::*;
use crate::PanelKind;
use crate::cloud_run::{
    ArtifactDigest, CloudJobId, CloudProvider, CloudWorkflowId, GitCommitSha, GitSource, WorkerLifetime, WorkerTarget,
};
use crate::remote_workspace::{
    RemoteCleanupIntent, RemoteCleanupReason, RemotePanelBinding, RemoteRuntimeGeneration, RemoteRuntimePhase,
    RemoteWorkspaceSpec, RepositoryCheckpoint,
};
use std::sync::{Arc, Barrier};

const OWNER: &str = "11111111-1111-4111-8111-111111111111";
const OTHER_OWNER: &str = "22222222-2222-4222-8222-222222222222";

fn workspace(id: &str) -> RemoteWorkspaceState {
    RemoteWorkspaceState::new(RemoteWorkspaceSpec {
        workspace_local_id: id.into(),
        target: WorkerTarget {
            provider: CloudProvider::LocalDocker,
            profile: "local-development".into(),
            image: format!("registry.example/worker@sha256:{}", "a".repeat(64)),
            disk_gib: 20,
            lifetime: WorkerLifetime::TimeLimited { seconds: 900 },
            max_hourly_cost_micros: None,
        },
        repository: GitSource {
            repository: "owner/project".into(),
            commit: GitCommitSha::parse("b".repeat(40)).expect("commit"),
            branch: None,
        },
        working_directory: "crates/core".into(),
        generation: 0,
        panels: vec![RemotePanelBinding {
            panel_local_id: "panel-one".into(),
            kind: PanelKind::Pi,
            command: None,
            working_directory: Some("crates/core/tests".into()),
            task_handoff: Some("Continue the saved task\nwithout losing its tail marker".into()),
            agent_session_id: Some("saved-session".into()),
        }],
    })
    .expect("valid workspace")
}

fn store() -> (tempfile::TempDir, CloudWorkflowStore) {
    let directory = tempfile::tempdir().expect("private test directory");
    let store = CloudWorkflowStore::open_path(directory.path().join("control-plane/workflows.sqlite3"))
        .expect("open test store");
    (directory, store)
}

fn provisioning(mut state: RemoteWorkspaceState) -> RemoteWorkspaceState {
    state.spec.generation += 1;
    state.runtime = Some(RemoteRuntimeGeneration {
        workspace_local_id: state.spec.workspace_local_id.clone(),
        generation: state.spec.generation,
        workflow_id: CloudWorkflowId::new(),
        job_id: CloudJobId::new(),
        phase: RemoteRuntimePhase::Provisioning,
        worker: None,
        ssh: None,
        cleanup: None,
    });
    state
}

mod migration;
mod ownership;
mod recovery;

fn seed_workspaces(store: &CloudWorkflowStore, sizes: impl IntoIterator<Item = Option<usize>>) {
    let mut connection = Connection::open(store.path()).expect("raw store");
    let transaction = connection.transaction().expect("fixture transaction");
    for (index, size) in sizes.into_iter().enumerate() {
        let state = workspace(&format!("workspace-{index:04}"));
        let mut snapshot = encode(OWNER, &state).expect("encode fixture");
        if let Some(size) = size {
            assert!(size >= snapshot.len());
            snapshot.resize(size, b' ');
        }
        transaction
            .execute(
                "INSERT INTO remote_workspaces VALUES (?1, ?2, 1, ?3)",
                params![state.spec.workspace_local_id, OWNER, snapshot],
            )
            .expect("insert fixture");
    }
    transaction.commit().expect("commit fixture");
}
