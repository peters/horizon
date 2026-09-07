//! Consuming handoff into an existing disconnected remote view, never a task launch.

use super::Board;
use crate::{
    PanelContent, PanelId, SshConnectionStatus,
    cloud_run::CloudWorkflowStore,
    remote_panel_attachment::{RemotePanelAttachError, RemotePanelConnectionAttempt},
    runtime_state::RemoteWorkspaceReference,
};

impl Board {
    /// Install one explicit connection attempt into its exact disconnected client view.
    /// `client_session_id` must come from the actual resolved active client session,
    /// never from the view's copied owner claim. Cross-session Open needs separate
    /// admission and is not authorized by this same-owner handoff.
    ///
    /// Preserves view identity, layout, launch metadata and focus. Performs one owned
    /// store check, but no provider I/O, task replay, Stop, Delete or saved-state write.
    /// Success means local transport was installed, not authenticated readiness.
    /// # Errors
    /// Rejects mismatched sessions/targets, live existing transport or stale allocation.
    /// A rejected attempt closes only its own local transport, not the target view.
    pub fn adopt_remote_panel_connection(
        &mut self,
        store: &CloudWorkflowStore,
        client_session_id: &str,
        panel_id: PanelId,
        attempt: RemotePanelConnectionAttempt,
    ) -> Result<(), RemotePanelHandoffError> {
        let workspace = attempt.allocation().workspace();
        if client_session_id != workspace.session_id() {
            return Err(RemotePanelHandoffError::ClientSessionMismatch);
        }
        let panel = self.panel(panel_id).ok_or(RemotePanelHandoffError::TargetChanged)?;
        let reference = panel.remote_workspace().ok_or(RemotePanelHandoffError::TargetChanged)?;
        if reference.owner_session_id() != client_session_id
            || reference.workspace_local_id() != workspace.state().spec.workspace_local_id
            || panel.local_id != attempt.panel_id()
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
        let terminal = attempt.into_terminal(store)?;
        let panel = self.panel_mut(panel_id).ok_or(RemotePanelHandoffError::TargetChanged)?;
        panel.content = PanelContent::Terminal(terminal);
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
    #[error(transparent)]
    Attachment(#[from] RemotePanelAttachError),
}
