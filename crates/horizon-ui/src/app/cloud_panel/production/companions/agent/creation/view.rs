//! What the owner sees of an agent's request to create a companion cloud, on the
//! source cloud's card, and what the agent's polls answer while it waits.
use super::{Choice, Pending, State};
use serde_json::{Value, json};

impl State {
    /// The owner's view of each request for `cloud`, with Create, Decline and a checkout choice.
    pub(in crate::app::cloud_panel::production::companions) fn render(&mut self, ui: &mut egui::Ui, cloud: &str) {
        let mut chosen = Vec::new();
        for pending in self.pending.iter_mut().filter(|pending| pending.source == cloud) {
            let alias = pending.alias.clone();
            let choice = ui
                .push_id(("companion-creation", &alias), |ui| pending.render(ui))
                .inner;
            if let Some(choice) = choice {
                chosen.push((pending.source.clone(), alias, choice));
            }
        }
        self.actions.extend(chosen);
    }
}

impl Pending {
    pub(super) fn describe(&self) -> Value {
        let (phase, message) = if self.waiting() {
            (
                "confirmation_required",
                "Waiting for the owner to confirm creating this companion cloud on the source cloud's card",
            )
        } else {
            ("submitted", "The owner confirmed; the companion cloud is being created")
        };
        json!({
            "operation_id": self.id,
            "action": "ensure_ready",
            "cloud": self.source,
            "alias": self.alias,
            "target_cloud_id": null,
            "phase": phase,
            "done": false,
            // The owner's choice moves it forward, so polling is enough.
            "resend": false,
            "message": message,
        })
    }

    pub(super) fn render(&mut self, ui: &mut egui::Ui) -> Option<Choice> {
        ui.separator();
        ui.label(if self.existing {
            format!(
                "An agent asked to start companion {}, whose cloud was never started",
                self.alias
            )
        } else {
            format!("An agent asked to create companion {}", self.alias)
        });
        ui.small(format!(
            "{} · {}",
            self.declaration.repository, self.declaration.profile
        ));
        ui.small("Starting it runs a paid cloud from the checkout's committed profile.");
        let mut choice = None;
        if self.waiting() {
            if self.search.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.small("Looking for a checkout in this workspace…");
                });
            }
            let locked = self.cloud_id.is_some();
            for path in &self.checkouts {
                ui.add_enabled_ui(!locked, |ui| {
                    ui.radio_value(&mut self.chosen, Some(path.clone()), path.display().to_string());
                });
            }
            if let Some(path) = self.chosen.as_ref().filter(|path| !self.checkouts.contains(path)) {
                ui.small(format!("Checkout {}", path.display()));
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !locked,
                        crate::app::cloud_panel::runtime::action_button("Choose checkout…"),
                    )
                    .clicked()
                {
                    choice = Some(Choice::Browse);
                }
                let create = match (self.existing, locked) {
                    (true, _) => "Start cloud",
                    (false, true) => "Retry",
                    (false, false) => "Create cloud",
                };
                if ui
                    .add_enabled(
                        self.chosen.is_some(),
                        crate::app::cloud_panel::runtime::action_button(create),
                    )
                    .clicked()
                {
                    choice = Some(Choice::Create);
                }
                if ui
                    .add(crate::app::cloud_panel::runtime::action_button("Decline"))
                    .clicked()
                {
                    choice = Some(Choice::Decline);
                }
            });
        } else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.small("Creating the companion cloud…");
            });
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        choice
    }
}
