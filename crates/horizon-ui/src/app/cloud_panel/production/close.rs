use std::collections::BTreeSet;

use super::{HorizonApp, Runtime, Stage, cards::wording, lifecycle::Action};

#[derive(Default)]
pub(super) struct State {
    confirming: Option<u32>,
    deleting: BTreeSet<u32>,
}

impl HorizonApp {
    pub(in crate::app) fn cloud_close_confirmation_open(&self) -> bool {
        self.cloud_prototype.production.close.confirming.is_some()
    }

    pub(in crate::app::cloud_panel) fn request_cloud_close(&mut self, id: u32) {
        self.cloud_prototype.production.close.confirming = Some(id);
    }

    pub(in crate::app::cloud_panel) fn render_cloud_close_confirmation(&mut self, ctx: &egui::Context) {
        let Some(id) = self.cloud_prototype.production.close.confirming else {
            return;
        };
        let Some(group) = self.cloud_prototype.groups.0.iter().find(|group| group.issue == id) else {
            self.cloud_prototype.production.close.confirming = None;
            return;
        };
        let Some(launch) = &group.remote else { return };
        let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
        let choice = close_action(runtime, launch.deployment_started);
        let mut confirmed = false;
        let mut cancelled = false;
        let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let modal = egui::Id::new("cloud-close-confirmation");
        let response = egui::Modal::new(modal)
            .area(egui::Modal::default_area(modal).order(egui::Order::Tooltip))
            .show(ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 64.0).clamp(180.0, 460.0));
                ui.heading(format!("Close {}?", group.title));
                ui.add_space(10.0);
                if matches!(choice, Ok(Action::Remove)) {
                    ui.label("Close this cloud and its panels? No managed worker or workspace storage remains.");
                } else {
                    ui.label(wording::delete_confirmation(runtime));
                }
                if let Err(reason) = choice {
                    ui.add_space(8.0);
                    ui.label(reason);
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    confirmed = ui
                        .add_enabled(
                            choice.is_ok(),
                            egui::Button::new("Close cloud").fill(crate::theme::BTN_CLOSE()),
                        )
                        .clicked();
                    cancelled = ui.button("Keep cloud").clicked();
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
        if cancelled || dismissed || confirmed {
            self.cloud_prototype.production.close.confirming = None;
        }
        if confirmed && let Ok(action) = choice {
            match action {
                Action::Remove => self.remove_deleted_cloud(id, ctx),
                Action::Delete => {
                    self.change_production_worker(id, action, ctx);
                    if self
                        .cloud_prototype
                        .production
                        .runtimes
                        .get(&id)
                        .is_some_and(|runtime| {
                            runtime.receiver.is_some()
                                && runtime.stage.is_some_and(|stage| Stage::DELETION.contains(&stage))
                        })
                    {
                        self.cloud_prototype.production.close.deleting.insert(id);
                    }
                }
                _ => {}
            }
        }
    }

    pub(super) fn finish_closing_clouds(&mut self, ctx: &egui::Context) {
        let finished: Vec<_> = self
            .cloud_prototype
            .production
            .close
            .deleting
            .iter()
            .copied()
            .filter(|id| {
                self.cloud_prototype
                    .production
                    .runtimes
                    .get(id)
                    .is_none_or(|runtime| runtime.receiver.is_none())
            })
            .collect();
        for id in finished {
            self.cloud_prototype.production.close.deleting.remove(&id);
            if self
                .cloud_prototype
                .production
                .runtimes
                .get(&id)
                .is_some_and(|runtime| runtime.stage == Some(Stage::Deleted))
            {
                // The saved record is checked again by removal; a stale UI snapshot cannot discard resources.
                self.remove_deleted_cloud(id, ctx);
            }
        }
    }
}

fn close_action(runtime: &Runtime, deployment_started: bool) -> Result<Action, &'static str> {
    if runtime.busy() {
        return Err("Wait for the current cloud operation to finish before closing.");
    }
    if runtime.state_unavailable || (runtime.state.is_none() && deployment_started) {
        return Err("Cloud resource state is unavailable. Reconnect or check the provider before closing.");
    }
    Ok(
        if runtime.state.as_ref().is_none_or(|state| {
            state.stage == Stage::Deleted
                || (state.operation == horizon_core::cloud_runtime::CreateState::Prepared && state.spec.is_none())
        }) {
            Action::Remove
        } else {
            Action::Delete
        },
    )
}

#[cfg(test)]
mod tests;
