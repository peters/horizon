use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use horizon_core::{PanelKind, PanelOptions, RemoteHostCatalog, WorkspaceId, WorkspaceLayout};

use crate::remote_hosts_overlay::{RemoteHostsOverlay, RemoteHostsOverlayAction};

use super::HorizonApp;

const DEFAULT_REMOTE_HOSTS_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

impl HorizonApp {
    pub(super) fn toggle_remote_hosts_overlay(&mut self) {
        if self.remote_hosts_overlay.is_some() {
            self.remote_hosts_overlay = None;
        } else {
            self.remote_hosts_overlay = Some(RemoteHostsOverlay::new());
            self.maybe_start_remote_hosts_refresh();
        }
    }

    pub(super) fn render_remote_hosts_overlay(&mut self, ctx: &egui::Context) {
        let Some(overlay) = self.remote_hosts_overlay.as_mut() else {
            return;
        };

        let next_refresh_secs = if self.remote_hosts_refresh_in_flight {
            None
        } else {
            self.remote_hosts_last_refresh.map(|t| {
                DEFAULT_REMOTE_HOSTS_REFRESH_INTERVAL
                    .saturating_sub(t.elapsed())
                    .as_secs()
            })
        };
        let action = overlay.show(
            ctx,
            &self.remote_hosts_catalog,
            self.remote_hosts_refresh_in_flight,
            next_refresh_secs,
        );
        match action {
            RemoteHostsOverlayAction::None => {}
            RemoteHostsOverlayAction::Cancelled => {
                self.remote_hosts_overlay = None;
            }
            RemoteHostsOverlayAction::OpenSsh { label, connection } => {
                self.remote_hosts_overlay = None;
                let workspace_id = self.remote_sessions_workspace();
                self.open_ssh_panel(workspace_id, label, connection);
            }
        }
    }

    pub(super) fn poll_remote_hosts_refresh(&mut self) {
        self.poll_inflight_refresh();

        // Auto-refresh while the overlay is open.
        if self.remote_hosts_overlay.is_some() {
            self.maybe_start_remote_hosts_refresh();
        }
    }

    fn poll_inflight_refresh(&mut self) {
        let Some(rx) = self.remote_hosts_refresh_rx.take() else {
            return;
        };

        match rx.try_recv() {
            Ok(result) => {
                self.remote_hosts_refresh_in_flight = false;
                self.remote_hosts_last_refresh = Some(Instant::now());
                match result {
                    Ok(catalog) => {
                        self.remote_hosts_catalog = catalog;
                    }
                    Err(error) => {
                        tracing::warn!("remote host discovery failed: {error}");
                    }
                }
            }
            Err(TryRecvError::Empty) => {
                self.remote_hosts_refresh_rx = Some(rx);
            }
            Err(TryRecvError::Disconnected) => {
                self.remote_hosts_refresh_in_flight = false;
                self.remote_hosts_last_refresh = Some(Instant::now());
                tracing::warn!("remote host refresh worker disconnected");
            }
        }
    }

    fn maybe_start_remote_hosts_refresh(&mut self) {
        if self.remote_hosts_refresh_in_flight {
            return;
        }

        let should_refresh = self
            .remote_hosts_last_refresh
            .is_none_or(|t| t.elapsed() >= DEFAULT_REMOTE_HOSTS_REFRESH_INTERVAL);

        if should_refresh {
            self.remote_hosts_refresh_rx = Some(Self::spawn_remote_host_catalog_refresh());
            self.remote_hosts_refresh_in_flight = true;
        }
    }

    fn spawn_remote_host_catalog_refresh() -> Receiver<horizon_core::Result<RemoteHostCatalog>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(horizon_core::discover_remote_hosts(None));
        });
        rx
    }

    fn remote_sessions_workspace(&mut self) -> WorkspaceId {
        const WORKSPACE_NAME: &str = "Remote Sessions";
        if let Some(ws) = self.board.workspaces.iter().find(|ws| ws.name == WORKSPACE_NAME) {
            return ws.id;
        }
        let ws_id = self.board.create_workspace(WORKSPACE_NAME);
        self.board.arrange_workspace(ws_id, WorkspaceLayout::Grid);
        self.mark_runtime_dirty();
        ws_id
    }

    fn open_ssh_panel(&mut self, workspace_id: WorkspaceId, label: String, connection: horizon_core::SshConnection) {
        let options = PanelOptions {
            name: Some(label),
            kind: PanelKind::Ssh,
            ssh_connection: Some(connection),
            ..PanelOptions::default()
        };

        if let Err(error) = self.create_panel_with_options(options, workspace_id) {
            tracing::error!("failed to create ssh panel from remote hosts: {error}");
        } else {
            self.mark_runtime_dirty();
        }
    }
}
