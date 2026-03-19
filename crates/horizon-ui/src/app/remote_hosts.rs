use std::sync::mpsc::{self, Receiver, TryRecvError};

use horizon_core::{PanelKind, PanelOptions, RemoteHostCatalog, RemoteHostsAction, WorkspaceId};

use super::HorizonApp;

const REMOTE_HOSTS_PANEL_SIZE: [f32; 2] = [880.0, 560.0];

impl HorizonApp {
    pub(super) fn open_remote_hosts_panel(&mut self) {
        let workspace_id = self
            .board
            .active_workspace
            .unwrap_or_else(|| self.board.ensure_workspace());
        if let Some(panel_id) = self.board.workspace(workspace_id).and_then(|workspace| {
            workspace.panels.iter().copied().find(|panel_id| {
                self.board
                    .panel(*panel_id)
                    .is_some_and(|panel| panel.kind == PanelKind::RemoteHosts)
            })
        }) {
            self.board.focus(panel_id);
            return;
        }

        let options = PanelOptions {
            kind: PanelKind::RemoteHosts,
            size: Some(REMOTE_HOSTS_PANEL_SIZE),
            ..PanelOptions::default()
        };

        match self.create_panel_with_options(options, workspace_id) {
            Ok(panel_id) => {
                self.board.focus(panel_id);
                self.mark_runtime_dirty();
            }
            Err(error) => {
                tracing::error!("failed to create remote hosts panel: {error}");
            }
        }
    }

    fn spawn_remote_host_catalog_refresh() -> Receiver<horizon_core::Result<RemoteHostCatalog>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(horizon_core::discover_remote_hosts(None));
        });
        rx
    }

    pub(super) fn poll_remote_hosts_panels(&mut self) {
        self.poll_remote_host_catalog_refreshes();
        self.remote_host_catalog_refreshes.retain(|panel_id, _| {
            self.board
                .panel(*panel_id)
                .is_some_and(|panel| panel.kind == PanelKind::RemoteHosts)
        });

        let mut panels_to_refresh = Vec::new();
        let mut pending_actions = Vec::new();

        for panel in &mut self.board.panels {
            let panel_id = panel.id;
            let workspace_id = panel.workspace_id;
            let Some(remote_hosts) = panel.remote_hosts_mut() else {
                continue;
            };

            remote_hosts.maybe_request_auto_refresh();
            if remote_hosts.should_start_refresh() && !self.remote_host_catalog_refreshes.contains_key(&panel_id) {
                remote_hosts.mark_refresh_started();
                panels_to_refresh.push(panel_id);
            }

            if let Some(action) = remote_hosts.take_pending_action() {
                pending_actions.push((workspace_id, action));
            }
        }

        for panel_id in panels_to_refresh {
            self.remote_host_catalog_refreshes
                .insert(panel_id, Self::spawn_remote_host_catalog_refresh());
        }

        for (workspace_id, action) in pending_actions {
            self.apply_remote_hosts_action(workspace_id, action);
        }
    }

    fn poll_remote_host_catalog_refreshes(&mut self) {
        let panel_ids: Vec<_> = self.remote_host_catalog_refreshes.keys().copied().collect();

        for panel_id in panel_ids {
            let Some(receiver) = self.remote_host_catalog_refreshes.remove(&panel_id) else {
                continue;
            };

            match receiver.try_recv() {
                Ok(result) => {
                    if let Some(panel) = self.board.panel_mut(panel_id)
                        && let Some(remote_hosts) = panel.remote_hosts_mut()
                    {
                        remote_hosts.apply_refresh_result(result);
                    }
                }
                Err(TryRecvError::Empty) => {
                    self.remote_host_catalog_refreshes.insert(panel_id, receiver);
                }
                Err(TryRecvError::Disconnected) => {
                    if let Some(panel) = self.board.panel_mut(panel_id)
                        && let Some(remote_hosts) = panel.remote_hosts_mut()
                    {
                        remote_hosts.apply_refresh_result(Err(horizon_core::Error::State(
                            "remote host refresh worker disconnected".to_string(),
                        )));
                    }
                }
            }
        }
    }

    fn apply_remote_hosts_action(&mut self, workspace_id: WorkspaceId, action: RemoteHostsAction) {
        match action {
            RemoteHostsAction::OpenSsh { label, connection } => {
                let options = PanelOptions {
                    name: Some(label),
                    kind: PanelKind::Ssh,
                    ssh_connection: Some(connection),
                    ..PanelOptions::default()
                };

                if let Err(error) = self.create_panel_with_options(options, workspace_id) {
                    tracing::error!("failed to create ssh panel from remote hosts panel: {error}");
                } else {
                    self.mark_runtime_dirty();
                }
            }
        }
    }
}
