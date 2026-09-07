//! Selected-environment Stop controls; painting grants no provider authority.

use super::{Confirmation, InventoryAction, RemoteEnvironmentSummary, StopState, same_target, supported};
use crate::theme;
use egui::RichText;

pub(super) fn show(
    ui: &mut egui::Ui,
    state: &StopState,
    selected: &RemoteEnvironmentSummary,
    idle: bool,
    action: &mut InventoryAction,
) {
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        ui.strong("Explicit Stop");
        let request = ui.add_enabled(
            idle && !state.is_pending() && supported(selected),
            egui::Button::new("Stop environment…"),
        );
        #[cfg(test)]
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new("stop-request-test"), request.rect);
            data.insert_temp(egui::Id::new("stop-request-enabled-test"), request.enabled());
        });
        if request.clicked() {
            *action = InventoryAction::RequestStop;
        }
    });
    if !supported(selected) {
        ui.label(
            RichText::new(
                "Stop requires a retained persistent local-provider worker. Timed and cloud Stop are not supported yet.",
            )
            .color(theme::FG_DIM()),
        );
    }
    if let Some(confirmation) = &state.confirmation {
        confirm(ui, confirmation, idle && !state.is_pending(), action);
    }
    if let Some(notice) = &state.notice
        && same_target(&notice.expected, selected)
    {
        ui.label("Last explicit Stop result:");
        ui.colored_label(
            if notice.succeeded {
                theme::FG()
            } else {
                theme::PALETTE_YELLOW()
            },
            &notice.message,
        );
        if !notice.succeeded {
            ui.label("Saved intent and identity may remain. Refresh the saved page before explicitly retrying.");
        }
    }
}

fn confirm(ui: &mut egui::Ui, confirmation: &Confirmation, enabled: bool, action: &mut InventoryAction) {
    let selected = &confirmation.expected;
    ui.strong("Stop this environment?");
    for (label, value) in [
        ("Workspace", selected.workspace_local_id.as_str()),
        ("Owning session", selected.owning_session_id.as_str()),
        ("Provider profile", selected.profile.as_str()),
        (
            "Exact resource ID",
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
    ui.colored_label(
        theme::PALETTE_YELLOW(),
        "Stops every process in this worker. Unsaved process memory is lost.",
    );
    ui.label("Retains this local container and its files. No checkpoint or backup is created. This does not delete the environment.");
    ui.horizontal_wrapped(|ui| {
        let cancel = ui.button("Cancel");
        let confirm = ui.add_enabled(
            enabled,
            egui::Button::new("Stop environment").stroke(egui::Stroke::new(1.0, theme::PALETTE_RED())),
        );
        #[cfg(test)]
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new("stop-cancel-test"), cancel.rect);
            data.insert_temp(egui::Id::new("stop-confirm-test"), confirm.rect);
        });
        if cancel.clicked() {
            *action = InventoryAction::CancelStop;
        }
        if confirm.clicked() {
            *action = InventoryAction::ConfirmStop;
        }
    });
}
