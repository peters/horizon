//! Opens the Dependencies panel and carries out what it asks the app to do.

use horizon_core::{PanelKind, PanelOptions, WorkspaceId};

use super::HorizonApp;

/// Room for the filter tiles, the pull request card and a selected repository side by side.
const PANEL_SIZE: [f32; 2] = [1320.0, 880.0];
const WORKER_TERMINAL_SIZE: [f32; 2] = [820.0, 560.0];

impl HorizonApp {
    /// Shows the Dependencies panel, creating it in the active workspace when there is none.
    /// One panel is enough: it observes the worker, which keeps running without it.
    pub(super) fn open_dependencies_panel(&mut self, ctx: &egui::Context) {
        if let Some(panel) = self
            .board
            .panels
            .iter()
            .find(|panel| panel.kind == PanelKind::Dependencies)
            .map(|panel| panel.id)
        {
            self.reveal_selected_panel(ctx, panel);
            return;
        }
        let workspace = self.dependencies_workspace();
        let options = PanelOptions {
            kind: PanelKind::Dependencies,
            size: Some(PANEL_SIZE),
            ..PanelOptions::default()
        };
        match self.create_panel_with_options(options, workspace) {
            Ok(panel) => self.reveal_new_panel(ctx, workspace, panel),
            Err(error) => tracing::error!(%error, "could not open the Dependencies panel"),
        }
        self.mark_runtime_dirty();
    }

    fn dependencies_workspace(&mut self) -> WorkspaceId {
        self.board
            .active_workspace
            .or_else(|| self.board.workspaces.first().map(|workspace| workspace.id))
            .unwrap_or_else(|| self.board.create_workspace("Dependencies"))
    }

    #[cfg(feature = "cloud-workspaces")]
    pub(super) fn apply_dependencies_requests(&mut self, ctx: &egui::Context) {
        use crate::dependencies_widget::Request;

        let requests: Vec<_> = self
            .panel_render_caches
            .dependencies_ui_state
            .iter_mut()
            .flat_map(|(panel, state)| state.take_requests().into_iter().map(|request| (*panel, request)))
            .collect();
        for (panel, request) in requests {
            match request {
                Request::OpenCloudSettings => self.open_cloud_accounts(ctx, false),
                Request::OpenWorkerTerminal { arguments, cwd } => {
                    let workspace = self
                        .board
                        .panel_workspace_id(panel)
                        .unwrap_or_else(|| self.dependencies_workspace());
                    let options = PanelOptions {
                        kind: PanelKind::Shell,
                        name: Some("Dependency worker · SSH".into()),
                        command: Some("ssh".into()),
                        args: arguments,
                        cwd: Some(cwd),
                        size: Some(WORKER_TERMINAL_SIZE),
                        ..PanelOptions::default()
                    };
                    self.open_dependencies_companion(ctx, options, workspace);
                }
                Request::OpenLocalAgent(launch) => {
                    let workspace = self.board.create_workspace("Worker diagnosis");
                    let options = PanelOptions {
                        kind: launch.kind,
                        name: Some("Local worker diagnosis".into()),
                        args: vec![launch.prompt],
                        cwd: Some(launch.cwd),
                        size: Some([960.0, 680.0]),
                        ..PanelOptions::default()
                    };
                    self.open_dependencies_companion(ctx, options, workspace);
                }
            }
        }
    }

    #[cfg(feature = "cloud-workspaces")]
    fn open_dependencies_companion(&mut self, ctx: &egui::Context, options: PanelOptions, workspace: WorkspaceId) {
        match self.create_panel_with_options(options, workspace) {
            Ok(panel) => self.reveal_new_panel(ctx, workspace, panel),
            Err(error) => tracing::error!(%error, "could not open a panel for the dependency worker"),
        }
        self.mark_runtime_dirty();
    }
}
