//! Durable remote workspace data, independent of providers, persistence I/O, and UI.
//! Snapshot validation is not permission to attach: fresh provider observation and lease checks remain required.

#[cfg(test)]
mod tests;
mod validation;

pub(crate) use validation::valid_local_id;

use crate::{
    PanelKind,
    cloud_run::{
        ArtifactDigest, CloudJobId, CloudWorkflowId, GitCommitSha, GitSource, WorkerTarget,
        interactive_worker::{InteractiveWorker, InteractiveWorkerSshEndpoint},
    },
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const REMOTE_WORKSPACE_STATE_VERSION: u32 = 1;

/// One durable workspace owns at most one runtime; panels never own workers.
///
/// Deserialization validates the complete aggregate. Call [`Self::validate`]
/// after editing public fields and before persistence or lifecycle actions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "WorkspaceSnapshot")]
pub struct RemoteWorkspaceState {
    pub version: u32,
    pub spec: RemoteWorkspaceSpec,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RemoteRuntimeGeneration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<RepositoryCheckpoint>,
}

impl RemoteWorkspaceState {
    /// Create a dormant workspace with a validated specification.
    /// # Errors
    /// Rejects invalid identities, targets, repositories, directories, or panels.
    pub fn new(spec: RemoteWorkspaceSpec) -> Result<Self, RemoteWorkspaceError> {
        let state = Self {
            version: REMOTE_WORKSPACE_STATE_VERSION,
            spec,
            runtime: None,
            checkpoint: None,
        };
        state.validate()?;
        Ok(state)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceSnapshot {
    version: u32,
    spec: RemoteWorkspaceSpec,
    #[serde(default)]
    runtime: Option<RemoteRuntimeGeneration>,
    #[serde(default)]
    checkpoint: Option<RepositoryCheckpoint>,
}

impl TryFrom<WorkspaceSnapshot> for RemoteWorkspaceState {
    type Error = RemoteWorkspaceError;

    fn try_from(value: WorkspaceSnapshot) -> Result<Self, Self::Error> {
        let state = Self {
            version: value.version,
            spec: value.spec,
            runtime: value.runtime,
            checkpoint: value.checkpoint,
        };
        state.validate()?;
        Ok(state)
    }
}

/// Desired workspace state survives deletion of its disposable worker.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteWorkspaceSpec {
    pub workspace_local_id: String,
    pub target: WorkerTarget,
    /// Uses the existing owner/repository identity, never a credential-bearing URL.
    pub repository: GitSource,
    /// Normalized repository-relative POSIX path; `.` means the repository root.
    pub working_directory: String,
    /// Last allocated runtime generation. Zero means no runtime has been allocated.
    pub generation: u64,
    pub panels: Vec<RemotePanelBinding>,
}

/// A panel's durable launch intent and optional agent-native resume identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemotePanelBinding {
    pub panel_local_id: String,
    pub kind: PanelKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<RemotePanelCommand>,
    /// Overrides the workspace directory, still relative to the repository root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    /// User task context, never a credential transport.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_handoff: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
}

impl RemotePanelBinding {
    /// Stable tmux identity derived from the panel, with no caller-supplied alias.
    /// # Errors
    /// Rejects an invalid panel identity before it can enter a tmux command.
    pub fn tmux_session_name(&self) -> Result<String, RemoteWorkspaceError> {
        if !validation::valid_local_id(&self.panel_local_id) {
            return Err(RemoteWorkspaceError::InvalidPanel("local_id"));
        }
        Ok(format!("horizon-panel-{}", self.panel_local_id))
    }
}

/// Argument boundaries are preserved; later execution must not join these into a shell command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemotePanelCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// One allocation attempt, persisted before provider creation starts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteRuntimeGeneration {
    pub workspace_local_id: String,
    pub generation: u64,
    pub workflow_id: CloudWorkflowId,
    pub job_id: CloudJobId,
    pub phase: RemoteRuntimePhase,
    /// Missing until an exact provider identity has been discovered and verified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<InteractiveWorker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<InteractiveWorkerSshEndpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<RemoteCleanupIntent>,
}

/// Dormant is represented by an absent runtime, never by a retained worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteRuntimePhase {
    Provisioning,
    Reconciling,
    Materializing,
    Ready,
    Checkpointing,
    Cancelling,
    Deleting,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteCleanupIntent {
    pub reason: RemoteCleanupReason,
    pub requested_at_millis: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteCleanupReason {
    LastPanelClosed,
    WorkspaceRemoved,
    ApplicationExit,
    Cancelled,
    Failed,
    LeaseExpired,
}

/// A verified repository watermark may outlive the runtime that produced it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryCheckpoint {
    pub workspace_local_id: String,
    pub base_commit: GitCommitSha,
    pub manifest_digest: ArtifactDigest,
    pub runtime_generation: u64,
    /// Monotonically increasing checkpoint watermark, independent of runtime generation.
    pub generation: u64,
    pub captured_at_millis: i64,
    /// Content-addressed recovery bundle, resolved by the checkpoint store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_artifact: Option<ArtifactDigest>,
}

/// Errors identify the invariant without echoing task text or command arguments.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum RemoteWorkspaceError {
    #[error("unsupported remote workspace state version {0}")]
    UnsupportedVersion(u32),
    #[error("invalid remote workspace specification: {0}")]
    InvalidSpec(&'static str),
    #[error("duplicate remote panel identity")]
    DuplicatePanel,
    #[error("invalid remote panel: {0}")]
    InvalidPanel(&'static str),
    #[error("invalid remote runtime: {0}")]
    InvalidRuntime(&'static str),
    #[error("invalid repository checkpoint: {0}")]
    InvalidCheckpoint(&'static str),
}
