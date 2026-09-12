//! Explicit reopening of inert client views, never execution or connection authority.

mod target;

use super::Board;
use crate::{
    Panel, PanelId, PanelKind, PanelOptions, RemoteWorkspaceReference, SshConnectionStatus,
    cloud_run::{CloudWorkflowStore, StoredRemoteWorkspace},
    remote_workspace::{RemoteEnvironmentSummary, RemotePanelBinding},
};
use std::time::Duration;
use target::ViewTarget;

/// Bounded saved panel identities and static eligibility hints, without commands,
/// task context or credentials. Loading neither observes a provider nor certifies
/// task existence, live readiness or execution authority.
pub struct RemoteViewCatalog {
    expected: RemoteEnvironmentSummary,
    panels: Vec<String>,
    shell_start_eligible: Vec<bool>,
}

impl RemoteViewCatalog {
    /// Read only the actual client's owned record, off the render thread.
    /// `client_session_id` must come from the actual resolved persistent session,
    /// never from a copied view or inventory owner claim.
    /// # Errors
    /// Rejects foreign, stale, missing or unreadable saved environments.
    pub fn load(
        store: &CloudWorkflowStore,
        client_session_id: &str,
        expected: &RemoteEnvironmentSummary,
    ) -> Result<Self, RemoteViewReopenError> {
        let record = load_current(store, client_session_id, expected)?;
        let (panels, shell_start_eligible) = record
            .state()
            .spec
            .panels
            .iter()
            .map(|panel| (panel.panel_local_id.clone(), saved_shell_start_eligible(panel)))
            .unzip();
        Ok(Self {
            expected: expected.clone(),
            panels,
            shell_start_eligible,
        })
    }

    #[must_use]
    pub fn panel_ids(&self) -> &[String] {
        &self.panels
    }

    /// Static saved binding shape only: Shell with a command and no task handoff
    /// or agent session. Unknown identities are ineligible. This cached hint does
    /// not validate command contents, Git readiness, ownership at execution time
    /// or worker availability; starting still requires fresh explicit admission.
    #[must_use]
    pub fn saved_shell_start_eligible(&self, panel_local_id: &str) -> bool {
        self.panels
            .iter()
            .zip(&self.shell_start_eligible)
            .any(|(id, eligible)| id == panel_local_id && *eligible)
    }

    /// Whether this saved identity has a local view of the same remote environment.
    /// Presentation only: a matching view does not grant connection authority or readiness.
    /// The panel's execution reference is independent of its visual workspace placement.
    #[must_use]
    pub fn view_is_present(&self, board: &Board, panel_local_id: &str) -> bool {
        self.panels.iter().any(|id| id == panel_local_id)
            && board.panels.iter().any(|panel| {
                panel.local_id == panel_local_id
                    && panel.remote_workspace().is_some_and(|reference| {
                        reference.owner_session_id() == self.expected.owning_session_id
                            && reference.workspace_local_id() == self.expected.workspace_local_id
                    })
            })
    }
}

fn saved_shell_start_eligible(panel: &RemotePanelBinding) -> bool {
    panel.kind == PanelKind::Shell
        && panel.command.is_some()
        && panel.task_handoff.is_none()
        && panel.agent_session_id.is_none()
}

/// An unreserved local insertion proposal. Board changes may invalidate it.
pub struct RemoteViewReopenRequest {
    expected: RemoteEnvironmentSummary,
    reference: RemoteWorkspaceReference,
    local_id: String,
    panel_id: PanelId,
    target: ViewTarget,
}

/// One fully prepared disconnected snapshot. Dropping it never touches remote work.
pub struct PreparedRemoteViewReopen {
    request: RemoteViewReopenRequest,
    panel: Panel,
}

impl RemoteViewReopenRequest {
    /// Prepare only an inert local snapshot, off the render thread.
    /// No SSH key, provider, command, task handoff, agent identity or remote cwd
    /// enters the client panel. The snapshot's fixed exit process is already gone
    /// before this method returns. Store freshness is point-in-time, not revocation.
    /// # Errors
    /// Rejects record drift, missing panel intent or failed snapshot preparation.
    pub fn prepare(self, store: &CloudWorkflowStore) -> Result<PreparedRemoteViewReopen, RemoteViewReopenError> {
        self.check_record(store)?;
        let mut panel = Panel::spawn(
            self.panel_id,
            self.target.id(),
            PanelOptions {
                name: Some(format!("Remote · {}", self.local_id)),
                kind: PanelKind::Ssh,
                local_id: Some(self.local_id.clone()),
                remote_workspace: Some(self.reference.clone()),
                ..PanelOptions::default()
            },
        )
        .map_err(|_| RemoteViewReopenError::PreparationFailed)?;
        let completed = panel.wait_for_shutdown(Duration::from_secs(2));
        panel.process_output();
        if !completed
            || panel.ssh_status() != Some(SshConnectionStatus::Disconnected)
            || !panel.terminal().is_some_and(|terminal| {
                terminal.child_exited() && terminal.child_exit_status().is_some_and(|status| status.success())
            })
        {
            return Err(RemoteViewReopenError::PreparationFailed);
        }
        self.check_record(store)?;
        Ok(PreparedRemoteViewReopen { request: self, panel })
    }

    fn check_record(&self, store: &CloudWorkflowStore) -> Result<(), RemoteViewReopenError> {
        let record = load_current(store, self.reference.owner_session_id(), &self.expected)?;
        if !record
            .state()
            .spec
            .panels
            .iter()
            .any(|panel| panel.panel_local_id == self.local_id)
        {
            return Err(RemoteViewReopenError::MissingPanel);
        }
        Ok(())
    }

    fn check_board(&self, board: &Board, client_session_id: &str) -> Result<(), RemoteViewReopenError> {
        if client_session_id != self.reference.owner_session_id() {
            return Err(RemoteViewReopenError::ClientSessionMismatch);
        }
        if board.next_panel_id != self.panel_id.0 || board.next_panel_id == u64::MAX {
            return Err(RemoteViewReopenError::TargetChanged);
        }
        if board.panels.iter().any(|panel| panel.local_id == self.local_id) {
            return Err(RemoteViewReopenError::ViewAlreadyPresent);
        }
        self.target.check(board, &self.reference, &self.local_id)
    }
}

impl Board {
    /// Propose reopening exactly one missing saved panel in its owning session.
    /// Performs only metadata checks; neither reserves ids nor changes the board.
    /// # Errors
    /// Rejects foreign clients, unknown panels, identity conflicts or ambiguous workspaces.
    pub fn request_remote_view_reopen(
        &self,
        client_session_id: &str,
        catalog: &RemoteViewCatalog,
        panel_local_id: &str,
    ) -> Result<RemoteViewReopenRequest, RemoteViewReopenError> {
        if client_session_id != catalog.expected.owning_session_id {
            return Err(RemoteViewReopenError::ClientSessionMismatch);
        }
        if !catalog.panels.iter().any(|id| id == panel_local_id) {
            return Err(RemoteViewReopenError::MissingPanel);
        }
        let reference =
            RemoteWorkspaceReference::new(client_session_id.into(), catalog.expected.workspace_local_id.clone())
                .map_err(|_| RemoteViewReopenError::TargetChanged)?;
        let request = RemoteViewReopenRequest {
            expected: catalog.expected.clone(),
            target: ViewTarget::select(self, &reference)?,
            reference,
            local_id: panel_local_id.into(),
            panel_id: PanelId(self.next_panel_id),
        };
        request.check_board(self, client_session_id)?;
        Ok(request)
    }

    /// Consume a prepared snapshot into the exact still-valid local board target.
    /// Performs no I/O, process startup, provider action, transport or saved-state write.
    /// Caller must invalidate results on session/board replacement (even same-owner
    /// reused ids), selection/config changes and dismissal before calling this method.
    /// A reopened view is not a connection; later reconnect must obtain fresh admission.
    /// # Errors
    /// Rejects stale owner/target/identity proposals before changing counters or layout.
    pub fn adopt_reopened_remote_view(
        &mut self,
        client_session_id: &str,
        prepared: PreparedRemoteViewReopen,
    ) -> Result<PanelId, RemoteViewReopenError> {
        let PreparedRemoteViewReopen { request, panel } = prepared;
        request.check_board(self, client_session_id)?;
        let workspace = request
            .target
            .install(self, &request.reference, &request.expected.repository);
        // Preflight verified the reference; the insertion callback cannot fail or spawn.
        self.create_panel_with(
            PanelOptions {
                remote_workspace: Some(request.reference),
                ..PanelOptions::default()
            },
            workspace,
            |_, _, _| Ok(panel),
        )
        .map_err(|_| RemoteViewReopenError::TargetChanged)
    }
}

fn load_current(
    store: &CloudWorkflowStore,
    client_session_id: &str,
    expected: &RemoteEnvironmentSummary,
) -> Result<StoredRemoteWorkspace, RemoteViewReopenError> {
    if client_session_id != expected.owning_session_id {
        return Err(RemoteViewReopenError::ClientSessionMismatch);
    }
    let record = store
        .load_remote_workspace(client_session_id, &expected.workspace_local_id)
        .map_err(|_| RemoteViewReopenError::StorageUnavailable)?
        .ok_or(RemoteViewReopenError::StateChanged)?;
    if record.environment_summary() != *expected {
        return Err(RemoteViewReopenError::StateChanged);
    }
    Ok(record)
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemoteViewReopenError {
    #[error("the active client session does not own this saved environment")]
    ClientSessionMismatch,
    #[error("the saved environment changed; refresh before reopening its views")]
    StateChanged,
    #[error("the saved environment could not be safely read")]
    StorageUnavailable,
    #[error("the selected panel is not in this saved environment")]
    MissingPanel,
    #[error("a local view already uses this panel identity; it was not replaced")]
    ViewAlreadyPresent,
    #[error("more than one local workspace matches this environment")]
    AmbiguousWorkspace,
    #[error("the local board target or persistence identities changed; retry reopening")]
    TargetChanged,
    #[error("the disconnected local view could not be prepared")]
    PreparationFailed,
}

#[cfg(test)]
mod tests;
