//! Explicit one-shot startup; existing inspection never calls this mutation path.

pub(super) mod configured;

use super::RemotePanelStatus;
use crate::{
    cloud_run::{CloudWorkflowStore, StoredRemoteAllocation, interactive_worker::InteractiveWorkerProvider},
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_workspace_recovery::RemoteWorkspaceRecoveryError,
};

/// Start only the saved explicit Shell intent on its completed ordinary Git checkout.
/// The saved cwd is relative to that checkout; the saved nonempty source branch is
/// the work branch. No caller-supplied command, environment or repository is accepted.
/// Requires actual-session/provider/cost authorization by the caller and runs off-thread.
/// Existing matching or exited tasks are observed, never restarted. A missing or
/// invalid reply is unknown: inspect before any further explicitly authorized action.
/// No worker allocation, credential installation, Stop/Delete or automatic retry occurs.
/// Fifteen-second stdin admission is capped by the lease, including spawn elapsed
/// time. Spawn/reap can block; bytes already sent are not atomically revoked by Stop.
/// # Errors
/// Rejects unsupported intent, stale ownership, missing pins and expired lifetimes.
pub fn start_remote_git_shell<P: InteractiveWorkerProvider + ?Sized>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    allocation: &StoredRemoteAllocation,
    panel_id: &str,
) -> Result<RemotePanelStatus, RemoteGitTaskStartError> {
    #[cfg(target_os = "linux")]
    {
        start_with(store, identities, provider, allocation, panel_id, execute)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, identities, provider, allocation, panel_id);
        Err(RemoteGitTaskStartError::UnsupportedPlatform)
    }
}

/// Static diagnostics omit task content, SSH output and private paths.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemoteGitTaskStartError {
    #[error("Git task start requires a fully supported saved Shell and repository intent")]
    InvalidIntent,
    #[error("Git task start requires an existing worker and retained SSH pin")]
    MissingRetainedWorker,
    #[error("Git task start requires an unexpired worker")]
    ExpiredWorker,
    #[error("task start outcome is unknown; inspect without automatic retry")]
    OutcomeUnknown,
    #[error("Git task start is unsupported on this client platform")]
    UnsupportedPlatform,
    #[error(transparent)]
    Recovery(#[from] RemoteWorkspaceRecoveryError),
    #[error(transparent)]
    Admission(#[from] super::RemotePanelStatusError),
}

#[cfg(target_os = "linux")]
use {
    crate::{
        cloud_run::interactive_worker::InteractiveWorkerSshEndpoint,
        remote_worker_ssh::{known_hosts, prepared_command, query},
        remote_workspace_recovery::{RecoveredRemoteWorkspace, inspect_remote_allocation},
    },
    std::{
        process::Command,
        time::{Duration, Instant},
    },
    time::OffsetDateTime,
};

#[cfg(target_os = "linux")]
fn request(allocation: &StoredRemoteAllocation, panel: &str) -> Result<Vec<u8>, RemoteGitTaskStartError> {
    use crate::{PanelKind, repository_git::GitPreparation};
    use RemoteGitTaskStartError::InvalidIntent;
    let state = allocation.workspace().state();
    let spec = &state.spec;
    let runtime = allocation
        .worker_request()
        .map_err(RemoteWorkspaceRecoveryError::from)?
        .job_id;
    let task = spec
        .panels
        .iter()
        .find(|task| task.panel_local_id == panel)
        .ok_or(InvalidIntent)?;
    if task.kind != PanelKind::Shell || task.task_handoff.is_some() || task.agent_session_id.is_some() {
        return Err(InvalidIntent);
    }
    let command = task.command.as_ref().ok_or(InvalidIntent)?;
    let directory = task.working_directory.as_deref().unwrap_or(&spec.working_directory);
    let path = std::path::Path::new(directory);
    let argv: Vec<&str> = std::iter::once(command.program.as_str())
        .chain(command.args.iter().map(String::as_str))
        .collect();
    if path.is_absolute()
        || path.components().any(|part| part == std::path::Component::ParentDir)
        || directory.contains('\0')
        || argv[0].is_empty()
        || argv[0].starts_with('-')
        || argv.len() > 257
        || argv.iter().any(|arg| arg.contains('\0'))
        || argv.iter().map(|arg| arg.len()).sum::<usize>() > 65536
    {
        return Err(InvalidIntent);
    }
    let repository = GitPreparation {
        version: 1,
        workspace_local_id: spec.workspace_local_id.clone(),
        runtime_id: runtime.to_string().parse().map_err(|_| InvalidIntent)?,
        source: spec.repository.clone(),
        work_branch: spec.repository.branch.clone().ok_or(InvalidIntent)?,
    };
    repository.binding().map_err(|_| InvalidIntent)?;
    let bytes = serde_json::to_vec(&serde_json::json!({
        "version":1, "operation":"start-git", "runtime":runtime, "panel":panel,
        "directory":directory, "argv":argv, "repository":repository,
    }))
    .map_err(|_| InvalidIntent)?;
    if bytes.len() > 512 * 1024 {
        return Err(InvalidIntent);
    }
    Ok(bytes)
}

#[cfg(target_os = "linux")]
fn start_with<P: InteractiveWorkerProvider + ?Sized>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    allocation: &StoredRemoteAllocation,
    panel: &str,
    execute: impl FnOnce(
        &CloudWorkflowStore,
        &RecoveredRemoteWorkspace,
        &str,
        &[u8],
    ) -> Result<RemotePanelStatus, RemoteGitTaskStartError>,
) -> Result<RemotePanelStatus, RemoteGitTaskStartError> {
    let input = request(allocation, panel)?;
    let current = store
        .load_remote_allocation(
            allocation.workspace().session_id(),
            &allocation.workspace().state().spec.workspace_local_id,
        )
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    if current.as_ref() != Some(allocation) {
        return Err(RemoteWorkspaceRecoveryError::StateChanged.into());
    }
    allocation
        .recovery_request()
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    let runtime = allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(RemoteGitTaskStartError::MissingRetainedWorker)?;
    if runtime.worker.is_none()
        || !runtime
            .ssh
            .as_ref()
            .is_some_and(InteractiveWorkerSshEndpoint::is_complete)
    {
        return Err(RemoteGitTaskStartError::MissingRetainedWorker);
    }
    let recovered = inspect_remote_allocation(identities, provider, allocation)?;
    validate(store, &recovered, panel)?;
    let result = execute(store, &recovered, panel, &input)?;
    validate(store, &recovered, panel).map_err(|_| RemoteGitTaskStartError::OutcomeUnknown)?;
    Ok(result)
}

#[cfg(target_os = "linux")]
fn lease_deadline(recovered: &RecoveredRemoteWorkspace) -> Result<Option<OffsetDateTime>, RemoteGitTaskStartError> {
    recovered
        .observation()
        .and_then(|status| status.worker.lifetime.as_time_limited())
        .map(|lease| {
            OffsetDateTime::parse(&lease.terminate_after, &time::format_description::well_known::Rfc3339)
                .map_err(|_| RemoteGitTaskStartError::ExpiredWorker)
        })
        .transpose()
}

#[cfg(target_os = "linux")]
fn validate<'a>(
    store: &CloudWorkflowStore,
    recovered: &'a RecoveredRemoteWorkspace,
    panel: &str,
) -> Result<&'a crate::cloud_run::interactive_worker::InteractiveWorkerSshEndpoint, RemoteGitTaskStartError> {
    let endpoint = crate::remote_worker_inspection::validate_current(store, recovered, Some(panel))?;
    if lease_deadline(recovered)?.is_some_and(|end| end <= OffsetDateTime::now_utc()) {
        return Err(RemoteGitTaskStartError::ExpiredWorker);
    }
    Ok(endpoint)
}

#[cfg(target_os = "linux")]
fn execute(
    store: &CloudWorkflowStore,
    recovered: &RecoveredRemoteWorkspace,
    panel: &str,
    input: &[u8],
) -> Result<RemotePanelStatus, RemoteGitTaskStartError> {
    let endpoint = validate(store, recovered, panel)?;
    let identity = recovered.identity();
    let trust = known_hosts(identity, endpoint)?;
    let command = prepared_command(identity.private_key_path(), trust.path(), endpoint)?;
    validate(store, recovered, panel)?;
    exchange(
        command,
        input,
        panel,
        lease_deadline(recovered)?,
        OffsetDateTime::now_utc,
    )
}

#[cfg(target_os = "linux")]
fn exchange(
    command: Command,
    mut input: &[u8],
    panel: &str,
    deadline: Option<OffsetDateTime>,
    now: impl Fn() -> OffsetDateTime,
) -> Result<RemotePanelStatus, RemoteGitTaskStartError> {
    let started = Instant::now();
    let timeout = timeout(deadline, now())?;
    let expired = || started.elapsed() >= timeout || deadline.is_some_and(|end| now() >= end);
    let expected = input.len() as u64;
    let result = query::exchange(
        command,
        &mut input,
        timeout,
        super::protocol::RESPONSE_LIMIT,
        expired,
        Some(expected),
    )
    .map_err(|_| RemoteGitTaskStartError::OutcomeUnknown)?;
    let (query::InputProgress::Complete(written) | query::InputProgress::Incomplete(written)) = result.input;
    if written != expected || !result.status.success() {
        return Err(RemoteGitTaskStartError::OutcomeUnknown);
    }
    super::protocol::response(&result.output, panel).map_err(|_| RemoteGitTaskStartError::OutcomeUnknown)
}

#[cfg(target_os = "linux")]
fn timeout(deadline: Option<OffsetDateTime>, now: OffsetDateTime) -> Result<Duration, RemoteGitTaskStartError> {
    let maximum = Duration::from_secs(15);
    match deadline {
        None => Ok(maximum),
        Some(end) if end <= now => Err(RemoteGitTaskStartError::ExpiredWorker),
        Some(end) => Duration::try_from(end - now)
            .map(|remaining| remaining.min(maximum))
            .map_err(|_| RemoteGitTaskStartError::ExpiredWorker),
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
