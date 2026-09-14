use super::{Action, AddState};
use crate::app::remote_environments::InventoryAction;

pub(super) fn show(ui: &mut egui::Ui, state: &mut AddState, enabled: bool, action: &mut InventoryAction) {
    if state.form.is_none() {
        if ui
            .add_enabled(enabled, egui::Button::new("Add independent Shell panel…"))
            .clicked()
        {
            *action = InventoryAction::AddShell(Action::Open);
        }
        return;
    }
    ui.group(|ui| {
        ui.strong("Add independent Shell panel");
        ui.label("Save a separate task identity on this workspace's retained worker. Saving does not run a command or open a terminal view.");
        if let Some(prepared) = &state.confirmation {
            let environment = prepared.environment();
            ui.label(format!("Environment: {} / {}", environment.owning_session_id, environment.workspace_local_id));
            ui.label(format!("New panel: {}", prepared.panel().panel_local_id));
            if let Some(command) = &prepared.panel().command {
                ui.label("Program and literal arguments:");
                ui.monospace(format!("{:?}", command.program));
                for argument in &command.args {
                    ui.monospace(format!("{argument:?}"));
                }
            }
            ui.label(format!("Directory within repository: {}", prepared.panel().working_directory.as_deref().unwrap_or("workspace default")));
            ui.label("Start saved Shell task and Reopen view remain separate actions. Existing tasks and saved panels are unchanged.");
            buttons(ui, enabled, "Save independent Shell panel", Action::Confirm, action);
        } else if let Some(form) = &mut state.form {
            ui.add_enabled_ui(enabled, |ui| {
                ui.label("Program");
                ui.text_edit_singleline(&mut form.program);
                ui.label("Arguments — one literal argument per line; no shell expansion");
                ui.add(egui::TextEdit::multiline(&mut form.arguments).desired_rows(3).desired_width(f32::INFINITY));
                ui.label("Repository-relative directory — blank uses workspace default");
                ui.text_edit_singleline(&mut form.directory);
            });
            buttons(ui, enabled, "Review Shell panel", Action::Prepare, action);
        }
    });
}

fn buttons(ui: &mut egui::Ui, enabled: bool, label: &str, submit: Action, action: &mut InventoryAction) {
    ui.horizontal_wrapped(|ui| {
        if ui.button("Cancel Shell addition").clicked() {
            *action = InventoryAction::AddShell(Action::Cancel);
        }
        let response = ui.add_enabled(enabled, egui::Button::new(label));
        #[cfg(test)]
        ui.ctx()
            .data_mut(|data| data.insert_temp(egui::Id::new("add-shell-submit"), response.rect));
        if response.clicked() {
            *action = InventoryAction::AddShell(submit);
        }
    });
}
