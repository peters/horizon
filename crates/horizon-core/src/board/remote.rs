//! Consuming handoff into an existing disconnected remote view, never a task launch.

use super::Board;
use crate::{
    PanelContent, PanelId, SshConnectionStatus, Terminal,
    cloud_run::CloudWorkflowStore,
    remote_panel_attachment::{RemotePanelAttachError, RemotePanelConnectionAttempt},
    runtime_state::RemoteWorkspaceReference,
};
use std::time::{Duration, Instant};

const MAX_HANDOFF_AGE: Duration = Duration::from_secs(1);

/// One short-lived, non-serializable local handoff, not remote execution authority.
/// Prepare off the render thread and deliver promptly. The UI must also discard
/// responses after a request, selection, configuration or active-session change.
pub struct PreparedRemotePanelHandoff {
    owner_session_id: String,
    workspace_local_id: String,
    panel_local_id: String,
    terminal: Terminal,
    admitted_at: Instant,
}

impl PreparedRemotePanelHandoff {
    /// Recheck the owned allocation and consume its transport off the render thread.
    /// Adoption rejects handoffs older than one second and closes only that local
    /// connection. Holding or dropping a handoff never expires, stops or deletes the remote task.
    /// Admission is point-in-time, not continuous revocation or an atomic Stop fence.
    /// # Errors
    /// Rejects allocation drift or failed storage reads without touching the view.
    pub fn prepare(
        store: &CloudWorkflowStore,
        attempt: RemotePanelConnectionAttempt,
    ) -> Result<Self, RemotePanelHandoffError> {
        let workspace = attempt.allocation().workspace();
        let owner_session_id = workspace.session_id().into();
        let workspace_local_id = workspace.state().spec.workspace_local_id.clone();
        let panel_local_id = attempt.panel_id().into();
        let admitted_at = Instant::now();
        let terminal = attempt.into_terminal(store)?;
        Ok(Self {
            owner_session_id,
            workspace_local_id,
            panel_local_id,
            terminal,
            admitted_at,
        })
    }
}

impl std::fmt::Debug for PreparedRemotePanelHandoff {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedRemotePanelHandoff")
            .finish_non_exhaustive()
    }
}

impl Board {
    /// Install one explicit connection attempt into its exact disconnected client view.
    /// `client_session_id` must come from the actual resolved active client session,
    /// never from the view's copied owner claim. Cross-session Open needs separate
    /// admission and is not authorized by this same-owner handoff.
    ///
    /// Preserves view identity, layout, launch metadata and focus. Performs no storage
    /// or provider I/O, task replay, Stop, Delete or saved-state write.
    /// Rechecks client/view identity, not durable allocation: a later Stop or store
    /// change may remain undetected. The short local age limit is not revocation.
    /// Success means local transport was installed, not authenticated readiness.
    /// # Errors
    /// Rejects mismatched sessions/targets, live existing transport or expired admission.
    /// A rejected attempt closes only its own local transport, not the target view.
    pub fn adopt_remote_panel_connection(
        &mut self,
        client_session_id: &str,
        panel_id: PanelId,
        handoff: PreparedRemotePanelHandoff,
    ) -> Result<(), RemotePanelHandoffError> {
        if client_session_id != handoff.owner_session_id {
            return Err(RemotePanelHandoffError::ClientSessionMismatch);
        }
        let panel = self.panel(panel_id).ok_or(RemotePanelHandoffError::TargetChanged)?;
        let reference = panel.remote_workspace().ok_or(RemotePanelHandoffError::TargetChanged)?;
        if reference.owner_session_id() != client_session_id
            || reference.workspace_local_id() != handoff.workspace_local_id
            || panel.local_id != handoff.panel_local_id
        {
            return Err(RemotePanelHandoffError::TargetChanged);
        }
        RemoteWorkspaceReference::validate_client_panel(
            &panel.local_id,
            panel.kind,
            &panel.resume,
            panel.session_binding.as_ref(),
        )
        .map_err(|_| RemotePanelHandoffError::TargetChanged)?;
        let visual_workspace = self
            .workspace_for_panel(panel_id)
            .ok_or(RemotePanelHandoffError::TargetChanged)?;
        if !visual_workspace.panels.contains(&panel_id)
            || visual_workspace
                .remote_workspace
                .as_ref()
                .is_some_and(|visual_reference| visual_reference != reference)
        {
            return Err(RemotePanelHandoffError::TargetChanged);
        }
        let old_terminal = panel.terminal().ok_or(RemotePanelHandoffError::TargetChanged)?;
        if panel.ssh_status() != Some(SshConnectionStatus::Disconnected) || !old_terminal.child_exited() {
            return Err(RemotePanelHandoffError::AlreadyConnected);
        }
        if handoff.admitted_at.elapsed() > MAX_HANDOFF_AGE {
            return Err(RemotePanelHandoffError::Expired);
        }
        let panel = self.panel_mut(panel_id).ok_or(RemotePanelHandoffError::TargetChanged)?;
        panel.content = PanelContent::Terminal(handoff.terminal);
        panel.terminal_title.clear();
        panel.had_recent_output = false;
        // Generic SSH promotes Connecting on any output; that is not attachment proof.
        panel.ssh_status = None;
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemotePanelHandoffError {
    #[error("the active client session does not own this remote connection attempt")]
    ClientSessionMismatch,
    #[error("the disconnected remote view changed; discard the connection attempt")]
    TargetChanged,
    #[error("the remote view still has a local connection; it was not replaced")]
    AlreadyConnected,
    #[error("the local connection handoff expired; retry without restarting the remote task")]
    Expired,
    #[error(transparent)]
    Attachment(#[from] RemotePanelAttachError),
}

#[cfg(test)]
mod tests;
