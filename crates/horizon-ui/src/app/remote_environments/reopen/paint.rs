use super::{InventoryAction, ReopenState};

pub(super) fn show(ui: &mut egui::Ui, state: &ReopenState, enabled: bool, action: &mut InventoryAction) {
    ui.separator();
    ui.strong("Reopen closed views");
    ui.label("Restore local views in their owning session. They stay disconnected; remote tasks are unchanged.");
    if ui
        .add_enabled(enabled && !state.is_pending(), egui::Button::new("Show saved panels"))
        .clicked()
    {
        *action = InventoryAction::ListReopenPanels;
    }
    if let Some(pending) = &state.pending {
        ui.label(if pending.discard {
            "Waiting for the discarded saved-view request to finish…"
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
                });
            });
        }
    }
    if let Some(notice) = &state.notice {
        ui.label(notice);
    }
}
