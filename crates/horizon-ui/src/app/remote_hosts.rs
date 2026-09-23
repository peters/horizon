mod launch;
mod preferences;
#[cfg(test)]
mod tests;

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use horizon_core::{RemoteHostCatalog, summarize_remote_host_connections};

use crate::remote_hosts_overlay::{
    RemoteHostsOverlay, RemoteHostsOverlayAction, RemoteHostsOverlayInputs, WorkspaceOption,
};

use super::HorizonApp;

const DEFAULT_REMOTE_HOSTS_REFRESH_INTERVAL: Duration = Duration::from_mins(1);

impl HorizonApp {
    pub(super) fn toggle_remote_hosts_overlay(&mut self, ctx: &egui::Context) {
        if self.remote_hosts_overlay.is_some() {
            self.dismiss_remote_hosts_overlay(ctx);
        } else {
            self.remote_hosts_overlay = Some(RemoteHostsOverlay::new());
            self.maybe_start_remote_hosts_refresh();
        }
    }

    pub(super) fn render_remote_hosts_overlay(&mut self, ctx: &egui::Context) {
        if self.remote_hosts_overlay.is_none() {
            return;
        }
        let workspaces = self.destination_workspaces();
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
        let connection_summaries = summarize_remote_host_connections(&self.board, &self.remote_hosts_catalog);
        let action = overlay.show(
            ctx,
            &RemoteHostsOverlayInputs {
                catalog: &self.remote_hosts_catalog,
                connection_summaries: &connection_summaries,
                refresh_in_flight: self.remote_hosts_refresh_in_flight,
                next_refresh_secs,
                workspaces: &workspaces,
                default_workspace: self.template_config.remote_hosts.default_workspace_name(),
            },
        );
        match action {
            RemoteHostsOverlayAction::None => {}
            RemoteHostsOverlayAction::Cancelled => {
                self.dismiss_remote_hosts_overlay(ctx);
            }
            RemoteHostsOverlayAction::Open {
                label,
                connection,
                mode,
                destination,
            } => {
                self.dismiss_remote_hosts_overlay(ctx);
                self.open_remote_host(ctx, label, connection, mode, &destination);
            }
            RemoteHostsOverlayAction::SetDefaultWorkspace(name) => {
                let notice = if self.set_remote_hosts_default_workspace(&name) {
                    format!("Default workspace: {name}")
                } else if self.settings_has_unsaved_edits() {
                    "Save or discard the Settings edits first".to_string()
                } else {
                    "Could not update the config file; see the log".to_string()
                };
                if let Some(overlay) = self.remote_hosts_overlay.as_mut() {
                    overlay.set_notice(notice);
                }
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

    /// Workspaces a remote session can land in: ordinary local ones. A legacy
    /// remote workspace turns any new panel into a read-only restore-failure
    /// view, so it is neither listed nor matched by name.
    pub(in crate::app) fn destination_workspaces(&self) -> Vec<WorkspaceOption> {
        self.board
            .workspaces
            .iter()
            .filter(|workspace| workspace.remote_workspace.is_none())
            .map(|workspace| WorkspaceOption {
                id: workspace.id,
                name: workspace.name.clone(),
            })
            .collect()
    }

    fn dismiss_remote_hosts_overlay(&mut self, ctx: &egui::Context) {
        if self.remote_hosts_overlay.take().is_some() {
            ctx.memory_mut(egui::Memory::stop_text_input);
        }
    }
}
