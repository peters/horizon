//! Turning an overlay choice into a panel in the right workspace.
use horizon_core::{PanelId, PanelKind, PanelOptions, SshConnection, WorkspaceId, WorkspaceLayout};

use crate::app::HorizonApp;
use crate::remote_hosts_overlay::{RemoteConnectMode, WorkspaceChoice};

impl HorizonApp {
    /// Open `connection` as an SSH terminal or a tunnelled VNC viewer in the
    /// chosen workspace, creating the configured default workspace on first use.
    pub(in crate::app) fn open_remote_host(
        &mut self,
        ctx: &egui::Context,
        label: String,
        connection: SshConnection,
        mode: RemoteConnectMode,
        destination: &WorkspaceChoice,
    ) -> Option<PanelId> {
        let workspace_id = self.resolve_remote_destination(ctx, destination);
        let options = self.remote_panel_options(label, connection, mode);
        match self.create_panel_with_options(options, workspace_id) {
            Ok(panel_id) => {
                self.reveal_new_panel(ctx, workspace_id, panel_id);
                self.mark_runtime_dirty();
                Some(panel_id)
            }
            Err(error) => {
                tracing::error!(%error, ?mode, "failed to open remote host from the overlay");
                None
            }
        }
    }

    fn resolve_remote_destination(&mut self, ctx: &egui::Context, destination: &WorkspaceChoice) -> WorkspaceId {
        if let WorkspaceChoice::Existing(id) = destination
            && self.board.workspace(*id).is_some()
        {
            return *id;
        }
        let name = self.template_config.remote_hosts.default_workspace_name().to_string();
        if let Some(workspace) = self.board.workspaces.iter().find(|workspace| workspace.name == name) {
            return workspace.id;
        }
        let workspace_id = self.create_workspace_visible(ctx, &name);
        self.board.arrange_workspace(workspace_id, WorkspaceLayout::Grid);
        self.mark_runtime_dirty();
        workspace_id
    }

    fn remote_panel_options(&self, label: String, connection: SshConnection, mode: RemoteConnectMode) -> PanelOptions {
        match mode {
            RemoteConnectMode::Ssh => PanelOptions {
                name: Some(label),
                kind: PanelKind::Ssh,
                ssh_connection: Some(connection),
                ..PanelOptions::default()
            },
            RemoteConnectMode::Vnc => PanelOptions {
                name: Some(label),
                kind: PanelKind::Device,
                command: Some(self.template_config.remote_hosts.vnc_target()),
                ssh_connection: Some(connection),
                ..PanelOptions::default()
            },
        }
    }
}
