//! The bulk stop of the sidebar: the user chooses which idle clouds of a group stop
//! their workers, and sees what that saves each hour before confirming.
use std::collections::BTreeSet;

use horizon_core::cloud_list::{self, Group};

use super::{HorizonApp, lifecycle::Action};
use crate::app::sidebar::IdleCloud;

#[derive(Default)]
pub(super) struct State {
    /// The group whose idle clouds the dialog offers; `None` while it is closed.
    group: Option<Group>,
    clouds: Vec<IdleCloud>,
    /// The clouds the user took out of the stop. All are chosen at first.
    unchecked: BTreeSet<u32>,
}

impl State {
    fn chosen(&self) -> impl Iterator<Item = &IdleCloud> {
        self.clouds
            .iter()
            .filter(|idle| !self.unchecked.contains(&idle.cloud.id))
    }
}

impl HorizonApp {
    pub(in crate::app) fn idle_stop_open(&self) -> bool {
        self.cloud_prototype.production.bulk_stop.group.is_some()
    }

    /// Opens the bulk stop for the idle clouds of `group` that the sidebar read last.
    pub(in crate::app) fn request_idle_stop(&mut self, group: Group) {
        let clouds = self.sidebar_idle_clouds(group);
        if clouds.is_empty() {
            return;
        }
        self.cloud_prototype.production.bulk_stop = State {
            group: Some(group),
            clouds,
            unchecked: BTreeSet::new(),
        };
    }

    pub(in crate::app::cloud_panel) fn render_idle_stop_confirmation(&mut self, ctx: &egui::Context) {
        let state = &mut self.cloud_prototype.production.bulk_stop;
        let Some(group) = state.group else { return };
        let mut confirmed = false;
        let mut cancelled = false;
        let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let modal = egui::Id::new("cloud-idle-stop-confirmation");
        let response = egui::Modal::new(modal)
            .area(egui::Modal::default_area(modal).order(egui::Order::Tooltip))
            .show(ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 64.0).clamp(180.0, 460.0));
                ui.heading("Stop idle workers?");
                ui.add_space(6.0);
                ui.label(format!(
                    "These clouds in {} are ready and no agent works on them.",
                    group.label()
                ));
                ui.add_space(8.0);
                for idle in &state.clouds {
                    let id = idle.cloud.id;
                    let mut checked = !state.unchecked.contains(&id);
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut checked, &idle.cloud.name).changed() {
                            if checked {
                                state.unchecked.remove(&id);
                            } else {
                                state.unchecked.insert(id);
                            }
                        }
                        let rate = idle
                            .cloud
                            .hourly_rate
                            .map_or_else(|| "no reported rate".to_owned(), cloud_list::rate_text);
                        ui.label(
                            egui::RichText::new(format!("{} · {rate}", idle.workspace))
                                .color(crate::theme::FG_DIM())
                                .size(12.0),
                        );
                    });
                }
                ui.add_space(10.0);
                let count = state.chosen().count();
                if count > 0 {
                    ui.label(egui::RichText::new(cloud_list::saving_text(state.chosen().map(|idle| &idle.cloud))).strong());
                }
                ui.label("Running processes end. Workspace storage is kept and stays billable; Resume starts a worker again.");
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let label = if count == 1 {
                        "Stop 1 worker".to_owned()
                    } else {
                        format!("Stop {count} workers")
                    };
                    confirmed = ui
                        .add_enabled(count > 0, egui::Button::new(label).fill(crate::theme::BTN_CLOSE()))
                        .clicked();
                    cancelled = ui.button("Keep running").clicked();
                });
            });
        let dismissed = response.should_close();
        if dismissed && escape {
            self.consume_navigation_key(
                ctx,
                horizon_core::ShortcutBinding::new(
                    horizon_core::ShortcutModifiers::NONE,
                    horizon_core::ShortcutKey::Escape,
                ),
            );
        }
        if !(cancelled || dismissed || confirmed) {
            return;
        }
        let state = std::mem::take(&mut self.cloud_prototype.production.bulk_stop);
        if confirmed {
            for id in state.chosen().map(|idle| idle.cloud.id) {
                // The list is up to a second old: a cloud that got busy since keeps running.
                if self.cloud_can_stop_now(id) {
                    self.change_production_worker(id, Action::Stop, ctx);
                }
            }
        }
    }

    /// Cloud `id` is still idle and its card offers Stop.
    fn cloud_can_stop_now(&self, id: u32) -> bool {
        self.cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.issue == id)
            .is_some_and(|group| self.cloud_facts(group, std::time::SystemTime::now()).idle())
    }
}

#[cfg(all(test, unix))]
mod tests;
