use super::{
    REMOTE_WORKSPACE_STATE_VERSION, RemotePanelBinding, RemoteRuntimeGeneration, RemoteRuntimePhase,
    RemoteWorkspaceError as Error, RemoteWorkspaceSpec, RemoteWorkspaceState, RepositoryCheckpoint,
};
use crate::{PanelKind, cloud_run::interactive_worker::valid_worker_target};
use std::collections::HashSet;

const MAX_PANELS: usize = 256;
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_PATH_BYTES: usize = 4096;

impl RemoteWorkspaceState {
    /// Validate identities, single-runtime ownership, panel isolation, and checkpoint provenance.
    /// # Errors
    /// Rejects invalid or incompatible persisted domain data without performing I/O.
    pub fn validate(&self) -> Result<(), Error> {
        if self.version != REMOTE_WORKSPACE_STATE_VERSION {
            return Err(Error::UnsupportedVersion(self.version));
        }
        self.spec.validate()?;
        if let Some(runtime) = &self.runtime {
            runtime.validate_for(&self.spec)?;
        }
        if let Some(checkpoint) = &self.checkpoint {
            checkpoint.validate_for(&self.spec)?;
        }
        Ok(())
    }
}

impl RemoteWorkspaceSpec {
    /// Validate the desired workspace without creating or inspecting resources.
    /// # Errors
    /// Rejects unsupported launch kinds, malformed identities, and invalid source or target data.
    pub fn validate(&self) -> Result<(), Error> {
        if !valid_local_id(&self.workspace_local_id) {
            return Err(Error::InvalidSpec("local_id"));
        }
        if self.target.image.len() > MAX_PATH_BYTES || !valid_worker_target(&self.target, self.target.provider) {
            return Err(Error::InvalidSpec("worker target"));
        }
        if self.repository.repository.len() > MAX_PATH_BYTES
            || self
                .repository
                .branch
                .as_ref()
                .is_some_and(|branch| branch.len() > MAX_PATH_BYTES)
            || self.repository.validate().is_err()
        {
            return Err(Error::InvalidSpec("repository"));
        }
        if !valid_relative_directory(&self.working_directory) {
            return Err(Error::InvalidSpec("working directory"));
        }
        if self.panels.len() > MAX_PANELS {
            return Err(Error::InvalidSpec("panel count"));
        }
        let mut ids = HashSet::new();
        for panel in &self.panels {
            panel.validate()?;
            if !ids.insert(&panel.panel_local_id) {
                return Err(Error::DuplicatePanel);
            }
        }
        Ok(())
    }
}

impl RemotePanelBinding {
    /// Validate a panel launch and its optional supported native session binding.
    /// # Errors
    /// Rejects malformed identities, non-terminal panel kinds, and invalid launch data.
    pub fn validate(&self) -> Result<(), Error> {
        if !valid_local_id(&self.panel_local_id) {
            return Err(Error::InvalidPanel("local_id"));
        }
        if !matches!(self.kind, PanelKind::Shell | PanelKind::Command) && !self.kind.is_agent() {
            return Err(Error::InvalidPanel("kind"));
        }
        if self
            .working_directory
            .as_deref()
            .is_some_and(|path| !valid_relative_directory(path))
        {
            return Err(Error::InvalidPanel("working directory"));
        }
        if let Some(command) = &self.command {
            if !valid_token(&command.program, MAX_PATH_BYTES)
                || command.args.len() > 256
                || command
                    .args
                    .iter()
                    .any(|arg| arg.len() > MAX_TEXT_BYTES || arg.contains('\0'))
                || command.args.iter().map(String::len).sum::<usize>() > MAX_TEXT_BYTES
            {
                return Err(Error::InvalidPanel("command"));
            }
        } else if self.kind == PanelKind::Command {
            return Err(Error::InvalidPanel("missing command"));
        }
        if self
            .task_handoff
            .as_ref()
            .is_some_and(|task| task.len() > MAX_TEXT_BYTES || task.contains('\0'))
        {
            return Err(Error::InvalidPanel("task handoff"));
        }
        if self
            .agent_session_id
            .as_deref()
            .is_some_and(|id| !self.kind.supports_session_binding() || !valid_token(id, MAX_PATH_BYTES))
        {
            return Err(Error::InvalidPanel("agent session"));
        }
        Ok(())
    }
}

impl RemoteRuntimeGeneration {
    /// Validate this allocation's binding to one workspace specification.
    /// Expired lease timestamps remain valid for exact-resource cleanup.
    /// # Errors
    /// Rejects generation, provider, workflow, target, connection, and cleanup drift.
    pub fn validate_for(&self, spec: &RemoteWorkspaceSpec) -> Result<(), Error> {
        if self.workspace_local_id != spec.workspace_local_id {
            return Err(Error::InvalidRuntime("workspace identity"));
        }
        if self.generation == 0 || self.generation != spec.generation {
            return Err(Error::InvalidRuntime("generation"));
        }
        if let Some(worker) = &self.worker {
            if !worker.has_valid_shape()
                || worker.target != spec.target
                || worker.identity.workflow_id != self.workflow_id
                || worker.identity.job_id != self.job_id
            {
                return Err(Error::InvalidRuntime("worker identity"));
            }
        } else if matches!(
            self.phase,
            RemoteRuntimePhase::Materializing
                | RemoteRuntimePhase::Ready
                | RemoteRuntimePhase::Checkpointing
                | RemoteRuntimePhase::Deleting
        ) {
            return Err(Error::InvalidRuntime("missing worker"));
        }
        if self
            .ssh
            .as_ref()
            .is_some_and(|ssh| self.worker.is_none() || !ssh.is_complete())
        {
            return Err(Error::InvalidRuntime("SSH endpoint"));
        }
        if matches!(
            self.phase,
            RemoteRuntimePhase::Materializing | RemoteRuntimePhase::Ready | RemoteRuntimePhase::Checkpointing
        ) && self.ssh.is_none()
        {
            return Err(Error::InvalidRuntime("missing SSH endpoint"));
        }
        if self
            .cleanup
            .as_ref()
            .is_some_and(|cleanup| cleanup.requested_at_millis < 0)
            || (matches!(
                self.phase,
                RemoteRuntimePhase::Cancelling | RemoteRuntimePhase::Deleting
            ) && self.cleanup.is_none())
        {
            return Err(Error::InvalidRuntime("cleanup intent"));
        }
        Ok(())
    }
}

impl RepositoryCheckpoint {
    /// Check a watermark's relationship to the desired repository and allocated generations.
    /// # Errors
    /// Rejects a different base, future/unallocated runtime, zero watermark, or invalid timestamp.
    pub fn validate_for(&self, spec: &RemoteWorkspaceSpec) -> Result<(), Error> {
        if self.workspace_local_id != spec.workspace_local_id {
            return Err(Error::InvalidCheckpoint("workspace identity"));
        }
        if self.base_commit != spec.repository.commit {
            return Err(Error::InvalidCheckpoint("base commit"));
        }
        if self.runtime_generation == 0 || self.runtime_generation > spec.generation || self.generation == 0 {
            return Err(Error::InvalidCheckpoint("generation"));
        }
        if self.captured_at_millis < 0 {
            return Err(Error::InvalidCheckpoint("timestamp"));
        }
        Ok(())
    }
}

pub(super) fn valid_local_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_token(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && !value.starts_with('-')
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}

fn valid_relative_directory(value: &str) -> bool {
    value == "."
        || (!value.is_empty()
            && value.len() <= MAX_PATH_BYTES
            && !value.contains(['\\', ':'])
            && !value.chars().any(char::is_control)
            && value.split('/').all(|part| !matches!(part, "" | "." | "..")))
}
