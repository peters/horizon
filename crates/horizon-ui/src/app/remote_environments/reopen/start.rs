//! Explicit confirmation is separate from inspection and never schedules a retry.

use super::{ClientContext, Completion, Context, InventoryAction, ReopenState, RequestScope};
use horizon_core::{
    cloud_run::CloudProvider,
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_worker_status::{
        ConfiguredRemotePanelStatusRequest, PreparedRemoteGitStart, RemotePanelStatus,
        prepare_configured_remote_git_start, start_configured_remote_git_shell,
    },
};

pub(super) fn supported(provider: CloudProvider, saved_shell_eligible: bool) -> bool {
    cfg!(target_os = "linux")
        && saved_shell_eligible
        && matches!(provider, CloudProvider::LocalDocker | CloudProvider::RunPod)
}

#[derive(Default)]
pub(super) struct StartState {
    confirmation: Option<Confirmation>,
    result: Option<(String, String)>,
}

struct Confirmation {
    panel: String,
    scope: RequestScope,
    prepared: Box<PreparedRemoteGitStart>,
    argv: String,
}

pub(super) enum PendingStart {
    Prepare(String),
    Execute(String),
}

impl PendingStart {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Prepare(_) => "Preparing the saved Shell confirmation…",
            Self::Execute(_) => "Requesting the saved Shell task on the remote worker…",
        }
    }
}

impl StartState {
    pub(super) fn cancel(&mut self) {
        self.confirmation = None;
    }

    pub(super) fn show(&self, ui: &mut egui::Ui, panel: &str, enabled: bool, action: &mut InventoryAction) {
        if let Some((id, result)) = &self.result
            && id == panel
        {
            ui.label("Last explicit start request:");
            ui.label(result);
            ui.label("This is not ongoing monitoring. Check the retained task for a new observation.");
        }
        let Some(confirmation) = &self.confirmation else { return };
        if confirmation.panel != panel {
            return;
        }
        let prepared = &confirmation.prepared;
        ui.strong("Start this saved Shell task?");
        for (label, value) in [
            ("Repository", prepared.repository()),
            ("Commit", prepared.commit()),
            ("Work branch", prepared.work_branch()),
            ("Directory", prepared.working_directory()),
        ] {
            ui.horizontal_wrapped(|ui| {
                ui.label(label);
                ui.add(
                    egui::Label::new(egui::RichText::new(value).monospace())
                        .wrap()
                        .selectable(true),
                );
            });
        }
        ui.label("Saved command (literal argv):");
        egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(&confirmation.argv).monospace())
                    .wrap()
                    .selectable(true),
            );
        });
        ui.label(
            "Runs on the retained worker after checking its prepared Git checkout. Closing this view does not stop it.",
        );
        ui.label("An existing matching task is not restarted. No worker or checkout is created, and no terminal view is attached here.");
        ui.horizontal_wrapped(|ui| {
            if ui.button("Cancel").clicked() {
                *action = InventoryAction::CancelTaskStart;
            }
            let confirm = ui.add_enabled(enabled, egui::Button::new("Start saved Shell task"));
            #[cfg(test)]
            ui.ctx().data_mut(|data| {
                data.insert_temp(egui::Id::new("start-task-confirm"), confirm.rect);
            });
            if confirm.clicked() {
                *action = InventoryAction::ConfirmTaskStart;
            }
        });
    }
}

impl ReopenState {
    pub(super) fn prepare_start(&mut self, client: &ClientContext<'_>, index: usize, ctx: &Context) {
        if self.is_pending() {
            return;
        }
        let Some(cached) = &self.catalog else { return };
        if !cached.scope.matches(client) {
            self.invalidate();
            return;
        }
        let Some(row) = cached.rows.get(index) else { return };
        if !row.start_supported {
            return;
        }
        let panel = row.id.clone();
        let requested_panel = panel.clone();
        let scope = cached.scope.clone();
        let requested_scope = scope.clone();
        self.start = StartState::default();
        self.spawn(client.home, scope, ctx, move |store| {
            prepare_configured_remote_git_start(
                store,
                &requested_scope.config,
                ConfiguredRemotePanelStatusRequest {
                    expected: &requested_scope.expected,
                    client_session_id: &requested_scope.owner,
                    panel_id: &requested_panel,
                },
            )
            .map(Box::new)
            .map(Completion::StartPreview)
            .map_err(|error| error.to_string())
        });
        if let Some(pending) = &mut self.pending {
            pending.start = Some(PendingStart::Prepare(panel));
        }
    }

    pub(super) fn confirm_start(&mut self, client: &ClientContext<'_>, ctx: &Context) {
        if self.is_pending() {
            return;
        }
        let Some(confirmation) = self.start.confirmation.take() else {
            return;
        };
        if !confirmation.scope.matches(client) {
            self.invalidate();
            return;
        }
        let panel = confirmation.panel.clone();
        let scope = confirmation.scope.clone();
        let identities = RemoteSshIdentityStore::new(client.home);
        self.spawn(client.home, scope, ctx, move |store| {
            start_configured_remote_git_shell(
                store,
                &identities,
                &confirmation.scope.config,
                ConfiguredRemotePanelStatusRequest {
                    expected: &confirmation.scope.expected,
                    client_session_id: &confirmation.scope.owner,
                    panel_id: &confirmation.panel,
                },
                *confirmation.prepared,
            )
            .map(Completion::Started)
            .map_err(|error| error.to_string())
        });
        if let Some(pending) = &mut self.pending {
            pending.start = Some(PendingStart::Execute(panel));
        }
    }

    pub(super) fn accept_start(
        &mut self,
        pending: PendingStart,
        scope: RequestScope,
        result: Result<Completion, String>,
    ) {
        match (pending, result) {
            (PendingStart::Prepare(panel), Ok(Completion::StartPreview(prepared))) if prepared.panel_id() == panel => {
                let argv = format!("{:?}", prepared.argv());
                self.start.confirmation = Some(Confirmation {
                    panel,
                    scope,
                    prepared,
                    argv,
                });
            }
            (PendingStart::Prepare(_), result) => {
                self.notice = Some(
                    result
                        .err()
                        .unwrap_or_else(|| "The start preview did not match its request.".into()),
                );
            }
            (PendingStart::Execute(panel), result) => {
                let message = match result {
                    Ok(Completion::Started(RemotePanelStatus::Running { pid })) => {
                        format!("Running at start response (PID {pid}).")
                    }
                    Ok(Completion::Started(RemotePanelStatus::Exited { pid, exit_status })) => {
                        format!(
                            "Exited at start response (PID {pid}, exit status {}). Not restarted.",
                            exit_status.map_or_else(|| "unknown".into(), |status| status.to_string())
                        )
                    }
                    Ok(Completion::Started(RemotePanelStatus::Unavailable)) => {
                        "Unavailable at start response. Not restarted.".into()
                    }
                    Err(message) => {
                        format!("{message} Inspect the retained task before another explicit start request.")
                    }
                    _ => "Start outcome is unknown. Inspect the retained task; no retry was scheduled.".into(),
                };
                self.start.result = Some((panel, message));
            }
        }
    }
}
