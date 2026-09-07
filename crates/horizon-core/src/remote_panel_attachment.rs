//! Explicit, non-creating attachment to one retained panel. Run off the render thread.

mod configured;

pub use configured::{
    ConfiguredRemotePanelAttachError, ConfiguredRemotePanelAttachRequest, attach_configured_remote_panel,
};

use crate::{
    Terminal,
    cloud_run::{
        CloudWorkflowStore, RemoteWorkspaceStoreError, StoredRemoteAllocation,
        interactive_worker::InteractiveWorkerProvider,
    },
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_worker_status::{RemotePanelStatus, RemotePanelStatusError},
    remote_workspace_recovery::RemoteWorkspaceRecoveryError,
};

/// Presentation inputs only: no executable, credential, environment or persisted SSH argv.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemotePanelTerminalSize {
    pub rows: u16,
    pub cols: u16,
    pub cell_width: u16,
    pub cell_height: u16,
    pub scrollback_limit: usize,
    pub window_id: u64,
    pub kitty_keyboard: bool,
}

#[derive(Clone, Copy)]
pub struct RemotePanelAttachRequest<'a> {
    pub allocation: &'a StoredRemoteAllocation,
    pub panel_id: &'a str,
    pub terminal: RemotePanelTerminalSize,
}

/// A local connection attempt, not authenticated attachment or workspace readiness.
/// Dropping it closes only local transport. It never stops a worker or replays a task.
pub struct RemotePanelConnectionAttempt {
    allocation: StoredRemoteAllocation,
    panel_id: String,
    observed_status: RemotePanelStatus,
    terminal: Terminal,
}

impl RemotePanelConnectionAttempt {
    #[must_use]
    pub fn allocation(&self) -> &StoredRemoteAllocation {
        &self.allocation
    }

    #[must_use]
    pub fn panel_id(&self) -> &str {
        &self.panel_id
    }

    /// Point-in-time status from saved-intent verification, not proof of attachment.
    #[must_use]
    pub fn observed_status(&self) -> &RemotePanelStatus {
        &self.observed_status
    }

    /// Consume only after checking the actual client session and disconnected target panel.
    /// Rechecks owned snapshots before handing off the input-capable local terminal.
    /// This is point-in-time admission, not continuous revocation or an atomic Stop fence.
    /// # Errors
    /// A stale attempt is discarded by closing only its local transport.
    pub fn into_terminal(self, store: &CloudWorkflowStore) -> Result<Terminal, RemotePanelAttachError> {
        check_current(store, &self.allocation)?;
        Ok(self.terminal)
    }
}

impl std::fmt::Debug for RemotePanelConnectionAttempt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemotePanelConnectionAttempt")
            .finish_non_exhaustive()
    }
}

/// Inspect one exact persistent allocation and verify saved shell/command intent,
/// then launch a pinned local SSH PTY that can only attach an existing worker pane.
/// The caller must explicitly admit the actual session, provider profile and cost
/// policy; a copied reference or old observation is not permission to call this.
/// No key creation, allocation, ensure, task startup, Stop, Delete or fallback occurs.
/// Exited panes remain inspectable without task replay. No repository readiness,
/// agent resume, authenticated attachment or saved Ready promotion is implied.
/// # Errors
/// Rejects stale/foreign snapshots, unsupported lifetime/platform/intent, pending
/// management, missing identity/task, changed pins and unavailable local transport.
pub fn attach_remote_panel<P: InteractiveWorkerProvider + ?Sized>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    request: RemotePanelAttachRequest<'_>,
) -> Result<RemotePanelConnectionAttempt, RemotePanelAttachError> {
    #[cfg(target_os = "linux")]
    {
        attach_with(
            store,
            identities,
            provider,
            request,
            crate::remote_worker_status::inspect_remote_panel_intent,
            spawn_pinned,
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, identities, provider, request);
        Err(RemotePanelAttachError::UnsupportedPlatform)
    }
}

#[cfg(target_os = "linux")]
fn attach_with<P: InteractiveWorkerProvider + ?Sized>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    request: RemotePanelAttachRequest<'_>,
    inspect: impl FnOnce(
        &CloudWorkflowStore,
        &crate::remote_workspace_recovery::RecoveredRemoteWorkspace,
        &str,
    ) -> Result<RemotePanelStatus, RemotePanelStatusError>,
    spawn: impl FnOnce(
        &CloudWorkflowStore,
        &crate::remote_workspace_recovery::RecoveredRemoteWorkspace,
        &str,
        RemotePanelTerminalSize,
    ) -> Result<Terminal, RemotePanelAttachError>,
) -> Result<RemotePanelConnectionAttempt, RemotePanelAttachError> {
    let expected = request.allocation;
    check_current(store, expected)?;
    expected.recovery_request()?;
    let spec = &expected.workspace().state().spec;
    if spec.target.lifetime != crate::cloud_run::WorkerLifetime::Persistent {
        return Err(RemotePanelAttachError::UnsupportedLifetime);
    }
    if !spec.panels.iter().any(|panel| panel.panel_local_id == request.panel_id) {
        return Err(RemotePanelStatusError::UnknownPanel.into());
    }
    let recovered = crate::remote_workspace_recovery::inspect_remote_allocation(identities, provider, expected)?;
    check_current(store, expected)?;
    let observed_status = inspect(store, &recovered, request.panel_id)?;
    if observed_status == RemotePanelStatus::Unavailable {
        return Err(RemotePanelAttachError::TaskUnavailable);
    }
    check_current(store, recovered.allocation())?;
    let terminal = spawn(store, &recovered, request.panel_id, request.terminal)?;
    check_current(store, recovered.allocation())?;
    Ok(RemotePanelConnectionAttempt {
        allocation: recovered.allocation().clone(),
        panel_id: request.panel_id.into(),
        observed_status,
        terminal,
    })
}

#[cfg(target_os = "linux")]
fn spawn_pinned(
    store: &CloudWorkflowStore,
    recovered: &crate::remote_workspace_recovery::RecoveredRemoteWorkspace,
    panel_id: &str,
    size: RemotePanelTerminalSize,
) -> Result<Terminal, RemotePanelAttachError> {
    let endpoint = recovered
        .observation()
        .and_then(|status| status.ssh.as_ref())
        .ok_or(RemotePanelStatusError::WorkerUnavailable)?;
    let runtime = recovered.allocation().recovery_request()?.job_id;
    let prepared =
        crate::remote_worker_ssh::PreparedAttachment::new(recovered.identity(), endpoint, runtime, panel_id)?;
    check_current(store, recovered.allocation())?;
    prepared
        .spawn(crate::terminal::TerminalSpawnOptions {
            program: String::new(),
            args: Vec::new(),
            cwd: None,
            rows: size.rows,
            cols: size.cols,
            cell_width: size.cell_width,
            cell_height: size.cell_height,
            scrollback_limit: size.scrollback_limit,
            window_id: size.window_id,
            replay_bytes: Vec::new(),
            env: std::collections::HashMap::new(),
            kitty_keyboard: size.kitty_keyboard,
        })
        .map_err(|_| RemotePanelAttachError::TransportUnavailable)
}

fn check_current(store: &CloudWorkflowStore, expected: &StoredRemoteAllocation) -> Result<(), RemotePanelAttachError> {
    let current = store.load_remote_allocation(
        expected.workspace().session_id(),
        &expected.workspace().state().spec.workspace_local_id,
    )?;
    if current.as_ref() != Some(expected) {
        return Err(RemotePanelAttachError::StateChanged);
    }
    Ok(())
}

/// Diagnostics omit provider output, task content, SSH arguments and private paths.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemotePanelAttachError {
    #[error("remote allocation changed; discard this connection attempt and refresh")]
    StateChanged,
    #[error("protected attachment currently requires persistent execution")]
    UnsupportedLifetime,
    #[error("retained panel is unavailable; no task was started or replaced")]
    TaskUnavailable,
    #[error("local remote connection could not be started")]
    TransportUnavailable,
    #[error("protected remote panel attachment is not yet supported on this platform")]
    UnsupportedPlatform,
    #[error(transparent)]
    Recovery(#[from] RemoteWorkspaceRecoveryError),
    #[error(transparent)]
    Inspection(#[from] RemotePanelStatusError),
}

impl From<RemoteWorkspaceStoreError> for RemotePanelAttachError {
    fn from(error: RemoteWorkspaceStoreError) -> Self {
        RemoteWorkspaceRecoveryError::from(error).into()
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
