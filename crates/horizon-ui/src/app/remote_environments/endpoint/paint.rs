//! Cached identity presentation and explicit consent; painting never dispatches I/O.

use super::{Action, EndpointState, InventoryAction, RemoteEnvironmentSummary, supported};

pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut EndpointState,
    selected: &RemoteEnvironmentSummary,
    idle: bool,
    action: &mut InventoryAction,
) {
    ui.separator();
    let request = ui.add_enabled(
        idle && supported(selected),
        egui::Button::new("Refresh saved connection…"),
    );
    #[cfg(test)]
    ui.ctx()
        .data_mut(|data| data.insert_temp(egui::Id::new("endpoint-request"), request.enabled()));
    if request.clicked() {
        *action = InventoryAction::Endpoint(Action::Request);
    }
    ui.label("Linux RunPod with retained HPS storage only. The worker must already be running; saved phase is not proof of current readiness.");
    if let Some(confirmation) = &mut state.confirmation {
        ui.strong("Authenticate and refresh this saved connection?");
        let expected = &confirmation.scope.expected;
        for (label, value) in [
            ("Workspace", expected.workspace_local_id.as_str()),
            ("Owning session", expected.owning_session_id.as_str()),
            ("Named profile", expected.profile.as_str()),
            (
                "Exact Pod",
                expected
                    .worker_identity
                    .as_ref()
                    .map_or("Unavailable", |worker| worker.resource_id.as_str()),
            ),
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
        ui.label("Uses only the original retained SSH host key and client identity to run /usr/bin/true at provider-observed coordinates. Only saved host/port may change after authentication and re-observation; no key discovery or rotation.");
        ui.label("No compute Start, Stop or Delete; no panel reconnect or task replay. Existing Start intent is preserved. A separate explicit Start retry may resolve it. No storage durability or backup is certified.");
        ui.label("Closing this overview does not cancel submitted work. Exiting Horizon may interrupt local coordination; refresh saved inventory after uncertainty. No retry is automatic.");
        ui.checkbox(
            &mut confirmation.acknowledged,
            "I authorize original-identity authentication and saving connection coordinates only.",
        );
        ui.horizontal_wrapped(|ui| {
            if ui.button("Cancel connection refresh").clicked() {
                *action = InventoryAction::Endpoint(Action::Cancel);
            }
            let confirm = ui.add_enabled(
                idle && confirmation.acknowledged,
                egui::Button::new("Authenticate and save connection"),
            );
            #[cfg(test)]
            ui.ctx()
                .data_mut(|data| data.insert_temp(egui::Id::new("endpoint-confirm"), (confirm.id, confirm.enabled())));
            if confirm.clicked_by(egui::PointerButton::Primary) {
                *action = InventoryAction::Endpoint(Action::Confirm);
            }
        });
    }
    if let Some(notice) = &state.notice {
        ui.label(notice.message);
    }
}
