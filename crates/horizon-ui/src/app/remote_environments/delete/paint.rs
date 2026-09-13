//! Destructive scope is explicit; painting and acknowledgement never call a provider.

use super::{
    Action, Confirmation, DeleteState, InventoryAction, Operation, RemoteEnvironmentSummary,
    result::{Status, supported},
};
use crate::theme;
use egui::RichText;
use horizon_core::{cloud_run::CloudProvider, remote_workspace::RemoteRuntimePhase};

pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut DeleteState,
    selected: &RemoteEnvironmentSummary,
    idle: bool,
    action: &mut InventoryAction,
) {
    ui.separator();
    ui.strong("Explicit Delete");
    ui.horizontal_wrapped(|ui| {
        for (operation, label, requested) in [
            (Operation::Delete, "Delete environment…", Action::Request),
            (Operation::Check, "Check saved Delete", Action::Check),
            (Operation::Retry, "Retry Delete…", Action::Retry),
        ] {
            let response = ui.add_enabled(
                idle && !state.is_pending() && supported(selected, operation),
                egui::Button::new(label),
            );
            #[cfg(test)]
            ui.ctx()
                .data_mut(|data| data.insert_temp(egui::Id::new(label), (response.rect, response.enabled())));
            if response.clicked() {
                *action = InventoryAction::Delete(requested);
            }
        }
    });
    if matches!(selected.saved_phase, Some(RemoteRuntimePhase::Deleted { .. })) {
        ui.label("Worker absence was verified previously (saved). This is historical, not a new provider check. Its identity and deletion record are retained.");
    } else {
        ui.label("Delete requires a retained persistent worker and exact saved profile/ownership. No SSH pin, private key or reachable guest is required. Check sends no Delete and does not contact the guest; verified completion may update the saved record.");
    }
    if selected.provider == CloudProvider::RunPod {
        ui.label("RunPod limitation: a fresh Check/Retry cannot confirm bare absence after a lost response or restart. Completion needs owned presence, acknowledged Delete and absence in the same operation. An unverified result is not permission to keep retrying.");
    }
    let pending = state.is_pending();
    if let Some(confirmation) = &mut state.confirmation {
        confirm(ui, confirmation, idle && !pending, action);
    }
    if let Some(notice) = &state.notice {
        ui.colored_label(
            if matches!(notice.status, Status::Verified | Status::Historical) {
                theme::FG()
            } else {
                theme::PALETTE_YELLOW()
            },
            &notice.message,
        );
        ui.label("Refresh saved inventory after uncertainty. Check never resends Delete; any Retry requires new destructive consent. No backup or billing cessation is certified.");
    }
}

fn confirm(ui: &mut egui::Ui, confirmation: &mut Confirmation, enabled: bool, action: &mut InventoryAction) {
    let selected = &confirmation.request.scope.expected;
    let retry = confirmation.request.operation == Operation::Retry;
    ui.strong(if retry {
        "Authorize another Delete request?"
    } else {
        "Delete this environment?"
    });
    for (label, value) in [
        ("Workspace", selected.workspace_local_id.as_str()),
        ("Owning session", selected.owning_session_id.as_str()),
        ("Named provider profile", selected.profile.as_str()),
        (
            if selected.provider == CloudProvider::Azure {
                "Exact owned resource group"
            } else {
                "Exact Pod ID"
            },
            selected
                .worker_identity
                .as_ref()
                .map_or("Unavailable", |identity| identity.resource_id.as_str()),
        ),
    ] {
        ui.horizontal_wrapped(|ui| {
            ui.label(label);
            ui.add(
                egui::Label::new(RichText::new(value).monospace())
                    .wrap()
                    .selectable(true),
            );
        });
    }
    ui.colored_label(theme::PALETTE_RED(), "Running tasks end. Unsaved memory and unpushed worker-local data can be permanently lost. No checkpoint or backup is created.");
    if selected.provider == CloudProvider::Azure {
        ui.label("Deletes the entire owned Azure resource group shown above: VM, managed workspace data disk and networking. Data on that group’s workspace disk is deleted, not retained as with Stop. No unrelated resource group is targeted.");
    } else {
        ui.label("Deletes only the exact RunPod Pod. Its independent HPS/network volume is not deleted and remains separately billable. Retained storage is not a backup or proof that files were saved durably.");
    }
    if retry {
        ui.colored_label(theme::PALETTE_YELLOW(), "The earlier Delete may still be in progress. This is separate consent for at most one further Delete, after checking whether the resource is still present.");
    }
    ui.label("No SSH pin or private key is needed. Closing this overview does not cancel an already submitted Delete. Exiting Horizon may interrupt local coordination while provider work continues. Ordinary closure never initiates deletion or stops compute.");
    let acknowledgement = ui.checkbox(
        &mut confirmation.acknowledged,
        "I understand the exact deletion scope and potential data loss.",
    );
    if acknowledgement.changed() {
        ui.ctx().request_repaint();
    }
    #[cfg(test)]
    ui.ctx()
        .data_mut(|data| data.insert_temp(egui::Id::new("delete-acknowledgement"), acknowledgement.rect));
    ui.horizontal_wrapped(|ui| {
        let cancel = ui.button("Cancel Delete");
        let confirm = ui.add_enabled(
            enabled && confirmation.acknowledged,
            egui::Button::new(if retry {
                "Send one Delete retry"
            } else {
                "Delete environment"
            })
            .stroke(egui::Stroke::new(1.0, theme::PALETTE_RED())),
        );
        #[cfg(test)]
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new("delete-cancel"), cancel.rect);
            data.insert_temp(egui::Id::new("delete-confirm"), (confirm.rect, confirm.enabled()));
            data.insert_temp(egui::Id::new("delete-confirm-focus"), confirm.id);
        });
        if cancel.clicked() {
            *action = InventoryAction::Delete(Action::Cancel);
        }
        // Destructive consent is a pointer activation, never an Enter/Space shortcut.
        if confirm.clicked_by(egui::PointerButton::Primary) {
            *action = InventoryAction::Delete(Action::Confirm);
        }
    });
}
