//! Selected-environment Stop controls; painting grants no provider authority.

use super::{
    CloudProvider, Confirmation, InventoryAction, RemoteEnvironmentSummary, StopState, check_supported, same_target,
    supported,
};
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
        let check = ui.add_enabled(
            idle && !state.is_pending() && check_supported(selected),
            egui::Button::new("Check saved Stop"),
        );
        #[cfg(test)]
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new("stop-check-test"), check.rect);
            data.insert_temp(egui::Id::new("stop-check-enabled-test"), check.enabled());
        });
        if check.clicked() {
            *action = InventoryAction::CheckStop;
        }
    });
    if !supported(selected) {
        ui.label(
            RichText::new(
                "Stop requires a retained persistent supported worker. Existing RunPod or Azure Stop intent can only be checked, never resent; timed workers are not supported.",
            )
            .color(theme::FG_DIM()),
        );
    }
    if check_supported(selected) {
        if selected.provider == CloudProvider::Azure {
            ui.label("Checks the existing Azure Stop intent without sending Stop again. Verified completion may update its saved record; no private SSH key is needed.");
            ui.label("Requires the exact named Azure profile, its immutable saved binding, the retained worker and public pin, and an Azure CLI login for that subscription. This is not task, filesystem, billing or live SSH proof.");
        } else {
            ui.label("Checks the existing RunPod Stop intent without sending Stop again. Verified completion may update its saved record; no private SSH key is needed.");
            ui.label("A retained worker/public pin and matching profile/storage are required. This is not task, filesystem, billing or live SSH proof.");
        }
    }
    if let Some(confirmation) = &state.confirmation {
        confirm(ui, confirmation, idle && !state.is_pending(), action);
    }
    if let Some(notice) = &state.notice
        && same_target(&notice.expected, selected)
    {
        ui.label(if notice.checked {
            "Last saved Stop check:"
        } else {
            "Last explicit Stop result:"
        });
        ui.colored_label(
            if notice.succeeded {
                theme::FG()
            } else {
                theme::PALETTE_YELLOW()
            },
            &notice.message,
        );
        if notice.checked {
            ui.label("Checks are manual point-in-time observations. Opening or closing this view never repeats them.");
            if notice.unverified && selected.provider == CloudProvider::Azure {
                ui.label("An unverified Azure check can mean the Azure CLI is not signed in to the profile's subscription. Sign in, then check again; nothing was sent to the worker.");
            }
        } else if !notice.succeeded {
            ui.label(if matches!(selected.provider, CloudProvider::RunPod | CloudProvider::Azure) {
                "Refresh the saved page. If Stop intent exists, use Check saved Stop; do not send another Stop request."
            } else {
                "Saved intent and identity may remain. Refresh the saved page before explicitly retrying."
            });
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
    if selected.provider == CloudProvider::RunPod {
        ui.label("Requires the exact saved HPS attachment and public pin; no private SSH key is needed. Stop verifies retained provider metadata, not filesystem durability or a backup. No volume deletion is requested; storage may still be billed.");
        ui.label("This sends one Stop request. After uncertainty, refresh and use Check saved Stop, never resend. Closing this overview does not cancel Stop; exiting Horizon may interrupt local coordination.");
    } else if selected.provider == CloudProvider::Azure {
        ui.label("Deallocates the worker VM: compute billing stops, the retained data disk keeps /workspace and continues to be billed. Requires the exact named Azure profile, its immutable saved binding and the saved public pin; no private SSH key is needed. Stop verifies deallocated compute and the retained disk, not filesystem durability or a backup.");
        ui.label("This sends one Stop request through the Azure CLI login for that subscription. After uncertainty, refresh and use Check saved Stop, never resend. Closing this overview does not cancel Stop; exiting Horizon may interrupt local coordination.");
    } else {
        ui.label("Retains this local container and its files. No checkpoint or backup is created. This does not delete the environment.");
    }
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
