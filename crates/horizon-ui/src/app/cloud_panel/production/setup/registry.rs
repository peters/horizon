use egui::Ui;
use horizon_core::cloud_runtime::{
    registry::{Action, draft::Draft},
    setup,
};

pub(super) fn render(ui: &mut Ui, accounts: &mut setup::Draft) -> Option<Action> {
    let mut action = None;
    let compute_saved = accounts.runpod_key.is_empty();
    ui.collapsing("Private container images", |ui| {
        ui.label("Bind each image repository to separate publishing and read-only worker credentials.");
        ui.small("Save stores credentials privately on this computer. Validate pull access sends only the saved pull credential to the compute provider. No image is published and no worker is allocated.");
        if !compute_saved {
            ui.small("Clear the unsaved compute key to manage existing provider access, or save it to switch accounts.");
        }
        for (index, draft) in accounts.registries.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                ui.separator();
                ui.label("Image repository (registry.example.com/team/worker)");
                ui.add_enabled(draft.original.is_none(), egui::TextEdit::singleline(&mut draft.repository));
                ui.label("Publishing username (optional for existing images)");
                ui.text_edit_singleline(&mut draft.publish_username);
                ui.label("Publishing credential");
                secret(ui, &mut draft.publish_secret, draft.original.as_ref().is_some_and(|binding| binding.publish.is_some()));
                expiry(ui, "Publishing expiry", &mut draft.publish_expiry);
                ui.label("Worker pull username");
                ui.text_edit_singleline(&mut draft.pull_username);
                ui.label("Read-only pull credential");
                secret(ui, &mut draft.pull_secret, draft.original.is_some());
                expiry(ui, "Pull expiry", &mut draft.pull_expiry);
                ui.checkbox(&mut draft.read_only_confirmed, "This dedicated pull grant is read-only and limited to the intended repository");
                ui.small("For ghcr.io, only read:packages is accepted. Other registries require you to confirm the issuer's grant. Unknown expiry is shown as unknown.");
                if let Some(saved) = &draft.original {
                    ui.small(format!("Pull generation: {}", saved.generation));
                    ui.small("Enter a replacement pull credential and save to rotate. Previous bindings remain available for explicit revocation.");
                    ui.label("Immutable image to validate (repository@sha256:…)");
                    ui.text_edit_singleline(&mut draft.validation_image);
                    ui.add_enabled_ui(compute_saved && draft.is_saved(), |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if ui.add_enabled(!draft.validation_image.is_empty(), egui::Button::new("Validate pull access")).clicked() {
                                action = Some(Action::Verify { image: draft.validation_image.clone() });
                            }
                            if ui.button("Status").clicked() { action = Some(Action::Status { repository: saved.repository.clone(), generation: saved.generation.clone() }); }
                            if ui.button("Reconcile").clicked() { action = Some(Action::Reconcile { repository: saved.repository.clone(), generation: saved.generation.clone() }); }
                            if ui.button("Revoke pull binding").clicked() { action = Some(Action::Revoke { repository: saved.repository.clone(), generation: saved.generation.clone() }); }
                        });
                        for generation in &saved.retired {
                            ui.horizontal_wrapped(|ui| {
                                ui.small(format!("Previous: {generation}"));
                                if ui.button("Reconcile previous").clicked() { action = Some(Action::Reconcile { repository: saved.repository.clone(), generation: generation.clone() }); }
                                if ui.button("Revoke previous").clicked() { action = Some(Action::Revoke { repository: saved.repository.clone(), generation: generation.clone() }); }
                            });
                        }
                    });
                    if !draft.is_saved() { ui.small("Save changes before managing provider access."); }
                    ui.small("Revocation removes provider pull access. Revoke the token at its issuer separately; running workers are unchanged.");
                }
            });
        }
        if ui.button("Add image repository").clicked() { accounts.registries.push(Draft::default()); }
    });
    action
}

fn secret(ui: &mut Ui, value: &mut String, saved: bool) {
    ui.add(egui::TextEdit::singleline(value).password(true).hint_text(if saved {
        "Leave blank to keep saved credential"
    } else {
        "Paste dedicated credential"
    }));
}

fn expiry(ui: &mut Ui, label: &str, value: &mut String) {
    ui.label(label);
    ui.add(egui::TextEdit::singleline(value).hint_text("Unknown, or 2027-01-01T00:00:00Z"));
}
