//! Compact saved inventory metadata. No live status or lifecycle authority is inferred.

use super::{RemoteRuntimePhase, RemoteWorkspaceState, RepositoryCheckpoint};
use crate::cloud_run::{
    CloudProvider, StoredRemoteWorkspace, WorkerLifetime, interactive_worker::InteractiveWorkerIdentity,
};

/// Safe to retain in an overview without retaining task handoffs, commands or SSH keys.
/// Saved setup phase is not a current provider observation, even when it is `Ready`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteEnvironmentSummary {
    pub workspace_local_id: String,
    pub owning_session_id: String,
    pub revision: u64,
    pub repository: String,
    pub provider: CloudProvider,
    pub profile: String,
    pub lifetime: WorkerLifetime,
    pub generation: u64,
    pub saved_phase: Option<RemoteRuntimePhase>,
    pub worker_identity: Option<InteractiveWorkerIdentity>,
    pub checkpoint: Option<RepositoryCheckpoint>,
    pub panel_count: usize,
}

impl StoredRemoteWorkspace {
    /// Project a validated owned record without copying executable or secret-bearing payloads.
    #[must_use]
    pub fn environment_summary(&self) -> RemoteEnvironmentSummary {
        RemoteEnvironmentSummary::from_state(self.session_id(), self.revision(), self.state())
    }
}

impl RemoteEnvironmentSummary {
    fn from_state(session_id: &str, revision: u64, state: &RemoteWorkspaceState) -> Self {
        Self {
            workspace_local_id: state.spec.workspace_local_id.clone(),
            owning_session_id: session_id.into(),
            revision,
            repository: state.spec.repository.repository.clone(),
            provider: state.spec.target.provider,
            profile: state.spec.target.profile.clone(),
            lifetime: state.spec.target.lifetime,
            generation: state.spec.generation,
            saved_phase: state.runtime.as_ref().map(|runtime| runtime.phase),
            worker_identity: state
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.worker.as_ref().map(|worker| worker.identity.clone())),
            checkpoint: state.checkpoint.clone(),
            panel_count: state.spec.panels.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PanelKind;
    use crate::cloud_run::{CloudJobId, CloudWorkflowId, GitCommitSha, GitSource, WorkerTarget};
    use crate::remote_workspace::{RemotePanelBinding, RemoteRuntimeGeneration, RemoteWorkspaceSpec};

    fn state() -> RemoteWorkspaceState {
        RemoteWorkspaceState::new(RemoteWorkspaceSpec {
            workspace_local_id: "saved-environment".into(),
            target: WorkerTarget {
                provider: CloudProvider::LocalDocker,
                profile: "development".into(),
                image: format!("example/worker@sha256:{}", "a".repeat(64)),
                disk_gib: 20,
                lifetime: WorkerLifetime::Persistent,
                max_hourly_cost_micros: None,
            },
            repository: GitSource {
                repository: "example/project".into(),
                commit: GitCommitSha::parse("b".repeat(40)).expect("commit"),
                branch: None,
            },
            working_directory: ".".into(),
            generation: 0,
            panels: vec![RemotePanelBinding {
                panel_local_id: "saved-panel".into(),
                kind: PanelKind::Shell,
                command: None,
                working_directory: None,
                task_handoff: Some("private-task-marker".repeat(1000)),
                agent_session_id: None,
            }],
        })
        .expect("valid state")
    }

    #[test]
    fn stored_summary_preserves_owner_without_task_payload() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store =
            crate::cloud_run::CloudWorkflowStore::open_path(temp.path().join("private/store.sqlite3")).expect("store");
        let owner = uuid::Uuid::new_v4().to_string();
        let stored = store.create_remote_workspace(&owner, &state()).expect("record");
        let summary = stored.environment_summary();
        assert_eq!(summary.owning_session_id, owner);
        assert_eq!(summary.workspace_local_id, "saved-environment");
        assert_eq!(summary.repository, "example/project");
        assert_eq!(summary.revision, 1);
        assert_eq!(summary.panel_count, 1);
        assert_eq!(summary.lifetime, WorkerLifetime::Persistent);
        assert!(summary.saved_phase.is_none());
        assert!(summary.worker_identity.is_none());
        assert!(!format!("{summary:?}").contains("private-task-marker"));
    }

    #[test]
    fn unobserved_runtime_is_not_replaced_with_a_display_identity() {
        let mut state = state();
        state.spec.generation = 2;
        state.runtime = Some(RemoteRuntimeGeneration {
            workspace_local_id: state.spec.workspace_local_id.clone(),
            generation: 2,
            workflow_id: CloudWorkflowId::new(),
            job_id: CloudJobId::new(),
            phase: RemoteRuntimePhase::Reconciling,
            ssh_public_key: None,
            worker: None,
            ssh: None,
            cleanup: None,
        });
        let summary = RemoteEnvironmentSummary::from_state("owner", 7, &state);
        assert_eq!(summary.saved_phase, Some(RemoteRuntimePhase::Reconciling));
        assert_eq!(summary.generation, 2);
        assert_eq!(summary.revision, 7);
        assert!(summary.worker_identity.is_none());
    }
}
