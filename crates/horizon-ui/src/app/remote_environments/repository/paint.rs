use super::{InventoryAction, RemoteGitCredentialMode, RepositoryState};

pub(super) fn show(ui: &mut egui::Ui, state: &mut RepositoryState, enabled: bool, action: &mut InventoryAction) {
    ui.separator();
    ui.strong("Remote repository preparation");
    if state.unknown {
        ui.colored_label(crate::theme::PALETTE_YELLOW(),
            "A repository operation is unconfirmed. A credential may already be installed. Inspect before another explicit request; never rotate or retry automatically.");
    }
    if let Some(notice) = &state.notice {
        ui.label(notice);
    }
    if state.is_pending() {
        ui.label("Repository operation pending. Closing this view does not cancel remote work.");
        return;
    }
    ui.add_enabled_ui(enabled, |ui| {
        if let Some((_, prepared)) = &state.confirmation {
            ui.strong("Prepare this exact saved repository?");
            ui.label(format!("Repository: {}", prepared.repository()));
            ui.label(format!("Commit: {}", prepared.commit()));
            ui.label(format!("Work branch: {}", prepared.work_branch()));
            let destination = prepared.environment();
            ui.label(format!("Destination: {:?} / {}", destination.provider, destination.profile));
            if let Some(worker) = &destination.worker_identity { ui.monospace(&worker.resource_id); }
            let installing = prepared.credential_mode() == RemoteGitCredentialMode::InstallFirst;
            if installing {
                ui.label("Repository-scoped PAT: transient input, sent only to this pinned worker. No replacement or rotation.");
                password_field(ui, &mut state.token);
                ui.checkbox(&mut state.consent, "I authorize first-token delivery to this worker.");
            } else { ui.label("Use the worker's existing credential; no PAT will be sent."); }
            ui.label("Install/present is not GitHub permission proof. This does not start a task.");
            ui.horizontal(|ui| {
                if ui.button("Cancel preparation").clicked() { *action = InventoryAction::CancelRepository; }
                let valid = !installing || state.consent && super::RepositoryPat::new(&state.token).is_ok();
                if ui.add_enabled(valid, egui::Button::new("Confirm repository preparation")).clicked() {
                    *action = InventoryAction::ConfirmRepository;
                }
            });
        } else {
            ui.checkbox(&mut state.install, "Include explicit first-token installation");
            ui.horizontal(|ui| {
                if ui.button("Review repository preparation").clicked() { *action = InventoryAction::PrepareRepository; }
                if ui.button("Check preparation receipt").clicked() { *action = InventoryAction::InspectRepository; }
            });
        }
    });
}

pub(super) fn password_field(ui: &mut egui::Ui, token: &mut String) -> egui::Id {
    let mut output = egui::TextEdit::singleline(token)
        .password(true)
        .char_limit(16_384)
        .id_salt("remote-repository-pat")
        .show(ui);
    // Password masking disables clipboard copy, but egui still records plaintext undo.
    output.state.clear_undoer();
    output.state.store(ui.ctx(), output.response.id);
    output.response.id
}
