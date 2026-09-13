//! Selected-environment Stop controls; painting grants no provider authority.

use super::{
    CloudProvider, Confirmation, InventoryAction, Operation, RemoteEnvironmentSummary, StopState, check_supported,
    same_target, start_supported, supported,
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
        let start = ui.add_enabled(
            idle && !state.is_pending() && start_supported(selected),
            egui::Button::new("Start environment…"),
        );
        #[cfg(test)]
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new("start-request-test"), start.rect);
            data.insert_temp(egui::Id::new("start-request-enabled-test"), start.enabled());
        });
        if start.clicked() {
            *action = InventoryAction::RequestStart;
        }
    });
    if !supported(selected) {
        ui.label(
            RichText::new(
                "Stop requires a retained persistent supported worker. Existing RunPod or Azure Stop intent can only be checked, never resent; a saved-Stopped Azure worker can be started; timed workers are not supported.",
            )
            .color(theme::FG_DIM()),
        );
    }
    if start_supported(selected) {
        ui.label("Starts the retained compute of this saved-Stopped Azure worker under the same identity. Compute billing resumes at the profile's declared hourly cost; the retained data disk keeps /workspace.");
        ui.label("In-memory work did not survive the stop and no task resumes. After the start, reconnect session panels; an existing Start intent is retried without re-posting a running worker.");
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
        show_notice(ui, notice, selected);
    }
}

fn confirm(ui: &mut egui::Ui, confirmation: &Confirmation, enabled: bool, action: &mut InventoryAction) {
    if confirmation.operation == Operation::Start {
        confirm_start(ui, confirmation, enabled, action);
        return;
    }
    let selected = &confirmation.expected;
    ui.strong("Stop this environment?");
    identity_rows(ui, selected);
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

fn identity_rows(ui: &mut egui::Ui, selected: &RemoteEnvironmentSummary) {
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
}

fn confirm_start(ui: &mut egui::Ui, confirmation: &Confirmation, enabled: bool, action: &mut InventoryAction) {
    let selected = &confirmation.expected;
    ui.strong("Start this environment?");
    identity_rows(ui, selected);
    let cost = confirmation
        .config
        .azure_profile(&selected.profile)
        .ok()
        .map(|profile| profile.declared_hourly_cost_micros);
    let billing = match cost {
        #[allow(clippy::cast_precision_loss)]
        Some(micros) => format!(
            "Compute billing resumes at this profile's declared hourly cost of {:.2} currency units per hour (declared, not a provider quote or spending cap). The retained data disk keeps billing as before.",
            micros as f64 / 1_000_000.0
        ),
        None => "Compute billing resumes at this profile's declared hourly cost (declared, not a provider quote or spending cap). The retained data disk keeps billing as before.".to_string(),
    };
    ui.colored_label(theme::PALETTE_YELLOW(), billing);
    ui.label("Starts the same worker VM with its retained data disk, saved address and host key; a worker that already runs is not re-posted. In-memory work did not survive the stop and nothing resumes a task. Requires the exact named Azure profile, its immutable saved binding and the saved public pin; no private SSH key is needed.");
    ui.label("This sends one Start request through the Azure CLI login for that subscription. After uncertainty, refresh and press Start again; the retry reuses the saved intent. Reconnect session panels once the saved phase is Reconciling. Closing this overview does not cancel Start; exiting Horizon may interrupt local coordination.");
    ui.horizontal_wrapped(|ui| {
        let cancel = ui.button("Cancel");
        let confirm = ui.add_enabled(enabled, egui::Button::new("Start environment"));
        #[cfg(test)]
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new("start-cancel-test"), cancel.rect);
            data.insert_temp(egui::Id::new("start-confirm-test"), confirm.rect);
        });
        if cancel.clicked() {
            *action = InventoryAction::CancelStop;
        }
        if confirm.clicked() {
            *action = InventoryAction::ConfirmStart;
        }
    });
}

fn show_notice(ui: &mut egui::Ui, notice: &super::StopNotice, selected: &RemoteEnvironmentSummary) {
    ui.label(if notice.checked {
        "Last saved Stop check:"
    } else if notice.started {
        "Last explicit Start result:"
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
    } else if notice.started {
        if !notice.succeeded {
            ui.label("Refresh the saved page. If Start intent remains, press Start again: the retry reuses the saved intent and never re-posts a running worker. Compute may already be billing.");
            if notice.unverified {
                ui.label("An unverified Azure start can mean the Azure CLI is not signed in to the profile's subscription. Sign in, then press Start again.");
            }
        }
    } else if !notice.succeeded {
        ui.label(
            if matches!(selected.provider, CloudProvider::RunPod | CloudProvider::Azure) {
                "Refresh the saved page. If Stop intent exists, use Check saved Stop; do not send another Stop request."
            } else {
                "Saved intent and identity may remain. Refresh the saved page before explicitly retrying."
            },
        );
    }
}
