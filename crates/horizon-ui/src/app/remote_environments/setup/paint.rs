//! Existing-theme controls and cached exact-value disclosure, never operational readiness.
use super::{Action, InventoryAction, PreparedRemoteWorkspaceSetup, SetupState};
use crate::theme;

pub(super) struct Review {
    pub(super) prepared: PreparedRemoteWorkspaceSetup,
    fields: Vec<(&'static str, String)>,
}
impl Review {
    pub(super) fn new(prepared: PreparedRemoteWorkspaceSetup, config: &super::RemoteProviderConfig) -> Self {
        let spec = prepared.spec();
        let mut fields = vec![
            ("Proposed workspace ID (not saved)", spec.workspace_local_id.clone()),
            (
                "Provider / profile",
                format!("{:?} / {}", spec.target.provider, spec.target.profile),
            ),
            ("Image", spec.target.image.clone()),
            ("Container disk", format!("{} GiB", spec.target.disk_gib)),
            ("Repository", spec.repository.repository.clone()),
            ("Exact commit", spec.repository.commit.as_str().into()),
            ("Work branch", spec.repository.branch.clone().unwrap_or_default()),
            ("Repository directory", spec.working_directory.clone()),
        ];
        match spec.target.provider {
            horizon_core::cloud_run::CloudProvider::LocalDocker => {
                if let Ok(profile) = config.local_docker_profile(&spec.target.profile) {
                    fields.push(("Docker endpoint", profile.docker_host.clone()));
                }
            }
            horizon_core::cloud_run::CloudProvider::RunPod => {
                if let Ok(profile) = config.runpod_profile(&spec.target.profile) {
                    fields.push((
                        "GPU choices / count",
                        format!("{:?} / {}", profile.gpu_type_ids, profile.gpu_count),
                    ));
                    fields.push((
                        "Registry pull registration",
                        if profile.container_registry_auth_id.is_some() {
                            "configured"
                        } else {
                            "absent"
                        }
                        .into(),
                    ));
                }
            }
            horizon_core::cloud_run::CloudProvider::Azure => {}
        }
        for panel in &spec.panels {
            if let Some(command) = &panel.command {
                fields.push(("Shell program (not executed)", command.program.clone()));
                fields.push(("Literal arguments", format!("{:?}", command.args)));
                fields.push((
                    "Panel directory",
                    panel
                        .working_directory
                        .clone()
                        .unwrap_or_else(|| "workspace directory".into()),
                ));
            }
        }
        if let Some(volume) = prepared.network_volume() {
            fields.push(("Authorized HPS volume", volume.volume_id.clone()));
            fields.push(("Volume data center", volume.data_center_id.clone()));
            fields.push(("Minimum volume size", format!("{} GB", volume.minimum_size_gb)));
        }
        if let Some(micros) = spec.target.max_hourly_cost_micros {
            fields.push(("Compute ceiling", format!("{} US cents/hour", micros / 10_000)));
        }
        Self { prepared, fields }
    }
}

impl SetupState {
    pub(in super::super) fn history(&self, ui: &mut egui::Ui, action: &mut InventoryAction) {
        if self.unknown {
            ui.label("An earlier creation has an uncertain outcome and may still be billing. Check its original coordinates; no retry or cleanup was scheduled.");
        }
        if !self.attempts.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("setup-attempts")
                .max_height(120.0)
                .show(ui, |ui| {
                    for (index, locator) in self.attempts.iter().enumerate() {
                        ui.push_id(index, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.monospace(&locator.workspace_local_id);
                                ui.monospace(&locator.owning_session_id);
                                if ui
                                    .add_enabled(
                                        self.available && !self.is_active(),
                                        egui::Button::new("Check this setup"),
                                    )
                                    .clicked()
                                {
                                    *action = InventoryAction::WorkspaceSetup(Action::CheckAttempt(index));
                                }
                            })
                        });
                    }
                });
            ui.label("Check uses the original home and owning session. Saved requests also remain in inventory.");
        }
        if let Some(notice) = &self.notice {
            ui.colored_label(theme::PALETTE_YELLOW(), notice);
        }
    }
    pub(in super::super) fn show(&mut self, ui: &mut egui::Ui, action: &mut InventoryAction) {
        ui.strong("New remote workspace");
        if let Some(pending) = &self.pending {
            let elapsed = pending.started.elapsed().as_secs();
            ui.label(format!("Waiting for the explicit setup operation ({elapsed} seconds)…"));
            ui.label("Closing detaches this view; it does not cancel creation, stop a worker or cap billing.");
            if elapsed >= 180 {
                ui.label("Still waiting. You may close this view and later inspect the original saved workspace.");
            } else if elapsed >= 90 {
                ui.label("Setup is taking longer than expected. Do not submit another creation; the original request may still be running.");
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
            return;
        }
        ui.label("Nothing has been created for this request yet. Review local values before authorizing setup.");
        if let Some(review) = &self.review {
            ui.strong("Review request");
            ui.label("Provider availability, image access, Git access and storage contents have not been checked.");
            for (label, value) in &review.fields {
                ui.horizontal_wrapped(|ui| {
                    ui.label(*label);
                    ui.monospace(value);
                });
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Owning session");
                ui.monospace(&review.prepared.locator().owning_session_id);
            });
            if review.prepared.network_volume().is_some() {
                ui.label("HPS identity is not ownership, exclusivity or durability proof. Storage charges are additional; the compute ceiling is not a total spending cap.");
            }
            ui.label(format!(
                "Setup admission expires at Unix ms {}. A persistent worker keeps running after closing Horizon.",
                review.prepared.retain_until_millis()
            ));
            ui.label("Setup sends no repository token, prepares no checkout, starts no task and attaches no view. Use separate Prepare and Start actions afterward.");
            ui.checkbox(&mut self.consent, "I trust this entrypoint-only image and displayed storage contents, authorize their use and continuing billing, and permit no tasks before the first host pin.");
            if ui
                .add_enabled(self.consent, egui::Button::new("Create task-free worker"))
                .clicked()
            {
                *action = InventoryAction::WorkspaceSetup(Action::Confirm);
            }
            if ui.button("Edit request").clicked() {
                *action = InventoryAction::WorkspaceSetup(Action::Edit);
            }
        } else if let Some(form) = &mut self.form {
            form.show(ui);
            if ui.button("Review request").clicked() {
                *action = InventoryAction::WorkspaceSetup(Action::Review);
            }
        }
        if ui.button("Cancel").clicked() {
            *action = InventoryAction::WorkspaceSetup(Action::Cancel);
        }
    }
    pub(in super::super) fn new_button(&self, ui: &mut egui::Ui, action: &mut InventoryAction) {
        if ui
            .add_enabled(
                self.available && !self.is_active(),
                egui::Button::new("New remote workspace"),
            )
            .clicked()
        {
            *action = InventoryAction::WorkspaceSetup(Action::New);
        }
        if ui
            .add_enabled(
                self.available && !self.is_active() && self.selected.is_some(),
                egui::Button::new("Check selected setup"),
            )
            .clicked()
        {
            *action = InventoryAction::WorkspaceSetup(Action::CheckSelected);
        }
        if !self.available {
            ui.label("Setup needs an open persistent Linux session and no pending remote action.");
        }
    }
}
