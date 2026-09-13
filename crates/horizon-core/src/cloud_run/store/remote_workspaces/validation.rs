use super::RemoteWorkspaceStoreError as Error;
use crate::remote_workspace::{RemoteRuntimePhase, RemoteWorkspaceState, valid_local_id};

pub(super) fn validate_session_id(session_id: &str) -> Result<(), Error> {
    let id = uuid::Uuid::parse_str(session_id).map_err(|_| Error::InvalidSessionId)?;
    if id.to_string() != session_id || id.is_nil() {
        return Err(Error::InvalidSessionId);
    }
    Ok(())
}

pub(in crate::cloud_run::store) fn validate_key(session_id: &str, workspace_local_id: &str) -> Result<(), Error> {
    validate_session_id(session_id)?;
    if !valid_local_id(workspace_local_id) {
        return Err(Error::InvalidWorkspaceId);
    }
    Ok(())
}

pub(super) fn validate_replacement(previous: &RemoteWorkspaceState, next: &RemoteWorkspaceState) -> Result<(), Error> {
    next.validate()?;
    if previous != next
        && [previous, next].into_iter().any(|state| {
            state
                .runtime
                .as_ref()
                .is_some_and(|runtime| runtime.phase.delete_requested_at_millis().is_some())
        })
    {
        return Err(Error::RuntimeDeleteCoordinationRequired);
    }
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
    // Stop intent is never erased, retargeted or rewound; a saved Stopped record changes
    // only into explicit Start intent requested no earlier than its observation.
    if let Some(runtime) = &previous.runtime
        && let Some(requested_at_millis) = runtime.phase.stop_requested_at_millis()
        && next.runtime.as_ref().is_none_or(|next_runtime| {
            !starts_after_stop(runtime.phase, next_runtime.phase)
                && (next_runtime.phase.stop_requested_at_millis() != Some(requested_at_millis)
                    || (matches!(runtime.phase, RemoteRuntimePhase::Stopped { .. })
                        && next_runtime.phase != runtime.phase))
        })
    {
        return Err(Error::NonMonotonicReplacement);
    }
    // Start intent resolves only into the same intent or a renewed observation.
    if let Some(runtime) = &previous.runtime
        && runtime.phase.start_requested_at_millis().is_some()
        && next.runtime.as_ref().is_none_or(|next_runtime| {
            next_runtime.phase != runtime.phase && next_runtime.phase != RemoteRuntimePhase::Reconciling
        })
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
                .ssh_public_key
                .as_ref()
                .is_some_and(|key| next_runtime.ssh_public_key.as_ref() != Some(key))
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

pub(super) fn validate_delete_replacement(
    previous: &RemoteWorkspaceState,
    next: &RemoteWorkspaceState,
) -> Result<(), Error> {
    use crate::{
        cloud_run::WorkerLifetime,
        remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason},
    };
    next.validate()?;
    let mut permitted = previous.clone();
    let runtime = permitted
        .runtime
        .as_mut()
        .ok_or(Error::RuntimeDeleteCoordinationRequired)?;
    let next_runtime = next.runtime.as_ref().ok_or(Error::RuntimeDeleteCoordinationRequired)?;
    if runtime.worker.is_none() || previous.spec.target.lifetime != WorkerLifetime::Persistent {
        return Err(Error::RuntimeDeleteCoordinationRequired);
    }
    match (runtime.phase, next_runtime.phase) {
        (
            RemoteRuntimePhase::Ready
            | RemoteRuntimePhase::Reconciling
            | RemoteRuntimePhase::Failed
            | RemoteRuntimePhase::Stopped { .. },
            RemoteRuntimePhase::DeleteRequested { requested_at_millis },
        ) if runtime.cleanup.is_none()
            && requested_at_millis >= 0
            && !matches!(runtime.phase, RemoteRuntimePhase::Stopped { observed_at_millis, .. }
                    if requested_at_millis < observed_at_millis) =>
        {
            runtime.cleanup = Some(RemoteCleanupIntent {
                reason: RemoteCleanupReason::WorkspaceRemoved,
                requested_at_millis,
            });
        }
        (
            RemoteRuntimePhase::DeleteRequested { requested_at_millis },
            RemoteRuntimePhase::DeleteRequested {
                requested_at_millis: retained,
            },
        ) if requested_at_millis == retained => {}
        (
            RemoteRuntimePhase::DeleteRequested { requested_at_millis },
            RemoteRuntimePhase::Deleted {
                requested_at_millis: retained,
                observed_at_millis,
            },
        ) if requested_at_millis == retained && observed_at_millis >= requested_at_millis => {}
        _ => return Err(Error::RuntimeDeleteCoordinationRequired),
    }
    runtime.phase = next_runtime.phase;
    if &permitted != next {
        return Err(Error::ReplacementIdentityMismatch);
    }
    Ok(())
}

pub(super) fn validate_endpoint_refresh(
    previous: &RemoteWorkspaceState,
    next: &RemoteWorkspaceState,
) -> Result<(), Error> {
    use crate::{
        cloud_run::{CloudProvider, WorkerLifetime},
        remote_workspace::start::endpoint::refresh_phase_allowed,
    };
    next.validate()?;
    if !refresh_phase_allowed(previous)
        || previous.spec.target.provider != CloudProvider::RunPod
        || previous.spec.target.lifetime != WorkerLifetime::Persistent
    {
        return Err(Error::ReplacementIdentityMismatch);
    }
    let mut permitted = previous.clone();
    let runtime = permitted.runtime.as_mut().ok_or(Error::ReplacementIdentityMismatch)?;
    if runtime.worker.is_none() {
        return Err(Error::ReplacementIdentityMismatch);
    }
    let saved = runtime
        .ssh
        .as_mut()
        .filter(|ssh| ssh.is_complete())
        .ok_or(Error::ReplacementIdentityMismatch)?;
    let observed = next
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.ssh.as_ref())
        .filter(|ssh| ssh.is_complete())
        .ok_or(Error::ReplacementIdentityMismatch)?;
    saved.host.clone_from(&observed.host);
    saved.port = observed.port;
    // Everything but transport coordinates is immutable, including phase and both keys.
    if permitted != *next {
        return Err(Error::ReplacementIdentityMismatch);
    }
    Ok(())
}

/// A saved Stopped record may take explicit Start intent requested at or after the
/// retention observation; no other phase follows a saved Stop.
fn starts_after_stop(previous: RemoteRuntimePhase, next: RemoteRuntimePhase) -> bool {
    matches!(
        (previous, next),
        (
            RemoteRuntimePhase::Stopped { observed_at_millis, .. },
            RemoteRuntimePhase::Starting { requested_at_millis },
        ) if requested_at_millis >= observed_at_millis
    )
}
