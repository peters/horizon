//! Cached selected-task presentation; reuse the saved-panel worker and invalidation scope.

use super::{ClientContext, Completion, Context, ReopenState};
use horizon_core::{
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_worker_status::{
        ConfiguredRemotePanelStatusRequest, RemotePanelObservation, RemotePanelStatus, inspect_configured_remote_panel,
    },
};

#[derive(Default)]
pub(super) struct TaskObservation {
    last_success: Option<CachedTask>,
    failure: Option<String>,
}

struct CachedTask {
    status: String,
    checked_at: String,
}

impl TaskObservation {
    fn accept(&mut self, result: Result<RemotePanelObservation, String>) {
        match result {
            Ok(observation) => {
                let status = match observation.status {
                    RemotePanelStatus::Running { pid } => format!("Running at check (PID {pid})"),
                    RemotePanelStatus::Exited {
                        pid,
                        exit_status: Some(code),
                    } => {
                        format!("Exited at check (PID {pid}, exit status {code})")
                    }
                    RemotePanelStatus::Exited { pid, exit_status: None } => {
                        format!("Exited at check (PID {pid}, exit status unknown)")
                    }
                    RemotePanelStatus::Unavailable => "Retained task unavailable at check. Nothing was created.".into(),
                };
                let checked_at = observation
                    .observed_at_rfc3339()
                    .unwrap_or_else(|| format!("{} Unix ms", observation.observed_at_millis));
                self.last_success = Some(CachedTask { status, checked_at });
                self.failure = None;
            }
            Err(message) => self.failure = Some(message),
        }
    }

    pub(super) fn show(&self, ui: &mut egui::Ui) {
        if let Some(previous) = &self.last_success {
            ui.label(&previous.status);
            ui.horizontal_wrapped(|ui| {
                ui.label("Last successful task check (UTC):");
                ui.monospace(&previous.checked_at);
            });
        }
        if let Some(message) = &self.failure {
            ui.colored_label(crate::theme::PALETTE_YELLOW(), message);
            if self.last_success.is_some() {
                ui.colored_label(
                    crate::theme::PALETTE_YELLOW(),
                    "Latest task check failed; the previous observation may be stale.",
                );
            }
        } else if self.last_success.is_none() {
            ui.label("Retained task has not been checked.");
        }
    }
}

impl ReopenState {
    pub(super) fn inspect_task(&mut self, client: &ClientContext<'_>, index: usize, ctx: &Context) {
        if self.is_pending() {
            return;
        }
        let Some(cached) = &mut self.catalog else { return };
        if !cached.scope.matches(client) {
            self.invalidate();
            self.set_notice("The session or saved selection changed. Show saved panels again.", ctx);
            return;
        }
        let Some(row) = cached.rows.get_mut(index) else { return };
        row.inspection.failure = None;
        let panel = row.id.clone();
        let requested_panel = panel.clone();
        let scope = cached.scope.clone();
        let requested_scope = scope.clone();
        let identities = RemoteSshIdentityStore::new(client.home);
        self.spawn(client.home, scope, ctx, move |store| {
            inspect_configured_remote_panel(
                store,
                &identities,
                &requested_scope.config,
                ConfiguredRemotePanelStatusRequest {
                    expected: &requested_scope.expected,
                    client_session_id: &requested_scope.owner,
                    panel_id: &requested_panel,
                },
            )
            .map(Completion::Inspection)
            .map_err(|error| error.to_string())
        });
        if let Some(pending) = &mut self.pending {
            pending.inspection = Some(panel);
        } else {
            let error = self.notice.take().unwrap_or_else(|| super::worker_failure().into());
            self.accept_inspection(&panel, Err(error));
        }
    }

    pub(super) fn accept_inspection(&mut self, panel: &str, result: Result<Completion, String>) {
        if let Some(row) = self
            .catalog
            .as_mut()
            .and_then(|cached| cached.rows.iter_mut().find(|row| row.id == panel))
        {
            row.inspection.accept(result.and_then(|completion| match completion {
                Completion::Inspection(observation) if observation.panel_id == panel => Ok(observation),
                _ => Err("The task result did not match its request. Check the task again.".into()),
            }));
        }
    }
}
