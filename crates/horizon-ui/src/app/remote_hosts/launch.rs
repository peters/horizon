//! Turning an overlay choice into a panel in the right workspace.
use horizon_core::{PanelId, PanelKind, PanelOptions, RemoteHostsConfig, SshConnection, WorkspaceId, WorkspaceLayout};

use crate::app::HorizonApp;
use crate::remote_hosts_overlay::{RemoteConnectMode, WorkspaceChoice};

/// One host the overlay chose, with everything typed into the filter applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct RemoteLaunch {
    pub label: String,
    pub connection: SshConnection,
    pub mode: RemoteConnectMode,
    /// A `:port` from the filter; `None` falls back to the config's per-host
    /// map and then to `remote_hosts.vnc_port`.
    pub vnc_port: Option<u16>,
}

impl RemoteLaunch {
    /// The Device panel target for this host's VNC server, as seen from the host.
    pub(in crate::app) fn vnc_target(&self, remote_hosts: &RemoteHostsConfig) -> String {
        remote_hosts.vnc_target(&self.label, &self.connection.host, self.vnc_port)
    }
}

impl HorizonApp {
    /// Open the host as an SSH terminal or a tunnelled VNC viewer in the
    /// chosen workspace, creating the configured default workspace on first use.
    pub(in crate::app) fn open_remote_host(
        &mut self,
        ctx: &egui::Context,
        launch: RemoteLaunch,
        destination: &WorkspaceChoice,
    ) -> Option<PanelId> {
        let workspace_id = self.resolve_remote_destination(ctx, destination);
        let mode = launch.mode;
        let options = self.remote_panel_options(launch);
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
        // Only ordinary local workspaces can hold a new panel; a legacy remote
        // workspace would turn it into a restore-failure view.
        if let WorkspaceChoice::Existing(id) = destination
            && self
                .board
                .workspace(*id)
                .is_some_and(|workspace| workspace.remote_workspace.is_none())
        {
            return *id;
        }
        let name = self.template_config.remote_hosts.default_workspace_name().to_string();
        if let Some(workspace) = self
            .board
            .workspaces
            .iter()
            .find(|workspace| workspace.name == name && workspace.remote_workspace.is_none())
        {
            return workspace.id;
        }
        let workspace_id = self.create_workspace_visible(ctx, &name);
        self.board.arrange_workspace(workspace_id, WorkspaceLayout::Grid);
        self.mark_runtime_dirty();
        workspace_id
    }

    fn remote_panel_options(&self, launch: RemoteLaunch) -> PanelOptions {
        let command = match launch.mode {
            RemoteConnectMode::Ssh => None,
            RemoteConnectMode::Vnc => Some(launch.vnc_target(&self.template_config.remote_hosts)),
        };
        PanelOptions {
            name: Some(launch.label),
            kind: match launch.mode {
                RemoteConnectMode::Ssh => PanelKind::Ssh,
                RemoteConnectMode::Vnc => PanelKind::Device,
            },
            command,
            ssh_connection: Some(launch.connection),
            ..PanelOptions::default()
        }
    }
}
