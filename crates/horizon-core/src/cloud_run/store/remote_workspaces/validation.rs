use super::RemoteWorkspaceStoreError as Error;
use crate::remote_workspace::{RemoteRuntimePhase, RemoteWorkspaceState, valid_local_id};

pub(super) fn validate_session_id(session_id: &str) -> Result<(), Error> {
    let id = uuid::Uuid::parse_str(session_id).map_err(|_| Error::InvalidSessionId)?;
    if id.to_string() != session_id || id.is_nil() {
        return Err(Error::InvalidSessionId);
    }
    Ok(())
}

pub(super) fn validate_key(session_id: &str, workspace_local_id: &str) -> Result<(), Error> {
    validate_session_id(session_id)?;
    if !valid_local_id(workspace_local_id) {
        return Err(Error::InvalidWorkspaceId);
    }
    Ok(())
}

pub(super) fn validate_replacement(previous: &RemoteWorkspaceState, next: &RemoteWorkspaceState) -> Result<(), Error> {
    next.validate()?;
    if previous.spec.workspace_local_id != next.spec.workspace_local_id {
        return Err(Error::ReplacementIdentityMismatch);
    }
    if next.spec.generation != previous.spec.generation
        && (previous.runtime.is_some()
            || next.runtime.is_none()
            || previous.spec.generation.checked_add(1) != Some(next.spec.generation))
    {
        return Err(Error::NonMonotonicReplacement);
    }
    if let Some(checkpoint) = &previous.checkpoint {
        let Some(next_checkpoint) = &next.checkpoint else {
            return Err(Error::NonMonotonicReplacement);
        };
        if next_checkpoint.generation < checkpoint.generation
            || next_checkpoint.runtime_generation < checkpoint.runtime_generation
            || next_checkpoint.captured_at_millis < checkpoint.captured_at_millis
            || (next_checkpoint.generation == checkpoint.generation && next_checkpoint != checkpoint)
        {
            return Err(Error::NonMonotonicReplacement);
        }
    }
    if let (Some(runtime), Some(next_runtime)) = (&previous.runtime, &next.runtime)
        && ((runtime.cleanup.is_some() && next_runtime.cleanup != runtime.cleanup)
            || (runtime.phase != RemoteRuntimePhase::Provisioning
                && next_runtime.phase == RemoteRuntimePhase::Provisioning))
    {
        return Err(Error::NonMonotonicReplacement);
    }
    if let (Some(runtime), Some(next_runtime)) = (&previous.runtime, &next.runtime)
        && (runtime.generation != next_runtime.generation
            || runtime.workflow_id != next_runtime.workflow_id
            || runtime.job_id != next_runtime.job_id
            || previous.spec.target != next.spec.target
            || previous.spec.repository != next.spec.repository
            || runtime
                .worker
                .as_ref()
                .is_some_and(|worker| next_runtime.worker.as_ref() != Some(worker))
            || runtime
                .ssh
                .as_ref()
                .is_some_and(|ssh| next_runtime.ssh.as_ref() != Some(ssh)))
    {
        return Err(Error::ReplacementIdentityMismatch);
    }
    if previous.runtime.is_none()
        && next.runtime.is_some()
        && previous.spec.generation.checked_add(1) != Some(next.spec.generation)
    {
        return Err(Error::NonMonotonicReplacement);
    }
    Ok(())
}
