//! Existing-theme controls and cached exact-value disclosure, never operational readiness.
use super::{Action, InventoryAction, PreparedRemoteWorkspaceSetup, PreviewState};
use crate::theme;

pub(super) struct Review {
    prepared: PreparedRemoteWorkspaceSetup,
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

impl PreviewState {
    pub(in super::super) fn show(&mut self, ui: &mut egui::Ui, action: &mut InventoryAction) {
        ui.strong("New remote workspace — request preview only");
        ui.label(
            "Nothing has been created. This form does not save a workspace, read credentials or contact a provider.",
        );
        if self.pending.is_some() {
            ui.label("Reviewing local request values…");
        } else if let Some(review) = &self.review {
            ui.strong("Review request");
            ui.label("Local validation passed. Provider availability, image access, Git access and storage contents have not been checked.");
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
            ui.label("A persistent worker would continue running after closing Horizon. No creation or billing is authorized by this review.");
            if ui.button("Edit request").clicked() {
                *action = InventoryAction::RequestPreview(Action::Edit);
            }
        } else if let Some(form) = &mut self.form {
            form.show(ui);
            if ui.button("Review request").clicked() {
                *action = InventoryAction::RequestPreview(Action::Review);
            }
        }
        if let Some(notice) = &self.notice {
            ui.colored_label(theme::PALETTE_YELLOW(), notice);
        }
        if ui.button("Cancel").clicked() {
            *action = InventoryAction::RequestPreview(Action::Cancel);
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
            *action = InventoryAction::RequestPreview(Action::New);
        }
        if !self.available {
            ui.label("Request preview needs an open persistent Linux session and no pending remote action.");
        }
    }
}
