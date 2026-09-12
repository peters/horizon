use super::{InventoryAction, ReopenState};

pub(super) fn show(ui: &mut egui::Ui, state: &ReopenState, enabled: bool, action: &mut InventoryAction) {
    ui.separator();
    ui.strong("Saved panels");
    if state.start_outcome_unknown {
        ui.label("An earlier start request lost its selection or session context. Its outcome is unknown. Return to that environment and check its retained task; no retry was scheduled.");
    }
    ui.label("Check a retained task or reopen its local view. Reopened views stay disconnected.");
    if ui
        .add_enabled(enabled && !state.is_pending(), egui::Button::new("Show saved panels"))
        .clicked()
    {
        *action = InventoryAction::ListReopenPanels;
    }
    if let Some(pending) = &state.pending {
        ui.label(if pending.discard && pending.inspection.is_some() {
            "Waiting for the discarded task check to finish…"
        } else if pending.discard {
            "Waiting for the discarded saved-view request to finish…"
        } else if let Some(start) = &pending.start {
            start.label()
        } else if pending.inspection.is_some() {
            "Checking the selected retained task…"
        } else {
            "Preparing saved local views…"
        });
    }
    if let Some(cached) = &state.catalog {
        if cached.rows.is_empty() {
            ui.label("No saved panel identities in this environment.");
        }
        for (index, row) in cached.rows.iter().enumerate() {
            ui.push_id((&row.id, "reopen"), |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(&row.id);
                    if row.present {
                        ui.label("View already open");
                    } else if ui
                        .add_enabled(enabled && !state.is_pending(), egui::Button::new("Reopen view"))
                        .clicked()
                    {
                        *action = InventoryAction::ReopenView(index);
                    }
                    let check =
                        ui.add_enabled(enabled && !state.is_pending(), egui::Button::new("Check retained task"));
                    #[cfg(test)]
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(egui::Id::new(("inspect-task-test", index)), check.rect);
                    });
                    if check.clicked() {
                        *action = InventoryAction::InspectTask(index);
                    }
                    let start = ui.add_enabled(
                        enabled && !state.is_pending() && row.start_supported,
                        egui::Button::new("Start saved Shell task…"),
                    );
                    #[cfg(test)]
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(egui::Id::new(("start-task-request", index)), start.rect);
                    });
                    if start.clicked() {
                        *action = InventoryAction::PrepareTaskStart(index);
                    }
                });
                row.inspection.show(ui);
                state.start.show(ui, &row.id, enabled && !state.is_pending(), action);
            });
        }
        ui.label("Task checks are point-in-time, not monitoring or proof of repository or attachment readiness.");
        ui.label("Checking does not start, reconnect, stop or delete remote work.");
    }
    if let Some(notice) = &state.notice {
        ui.label(notice);
    }
}
