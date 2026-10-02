use std::collections::{BTreeMap, BTreeSet};

use super::{HorizonApp, Runtime, Stage, cards::wording, lifecycle::Action};

#[derive(Default)]
pub(super) struct State {
    confirming: Option<u32>,
    deleting: BTreeSet<u32>,
    /// Per closing cloud, the panels that were showing when its disposal took over.
    hidden: BTreeMap<u32, Vec<String>>,
}

impl State {
    /// A confirmed close is deleting this cloud's resources.
    pub(in crate::app::cloud_panel) fn closing(&self, id: u32) -> bool {
        self.deleting.contains(&id)
    }
}

#[cfg(test)]
impl State {
    /// Starts a close the way a confirmed dialog does, for rendering tests.
    pub(in crate::app::cloud_panel::production) fn start_closing(&mut self, id: u32) {
        self.deleting.insert(id);
    }
}

impl super::Production {
    /// A confirmed close is deleting this cloud's resources.
    pub(in crate::app::cloud_panel) fn closing(&self, id: u32) -> bool {
        self.close.closing(id)
    }
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

    /// The panels of a cloud being closed end with it: whichever are showing stay out of
    /// sight while its disposal is shown, and only those return if the deletion could not
    /// finish. Every frame, because expanding a collapsed cloud shows its members again.
    /// What is saved is unaffected, so quitting meanwhile restores the cloud as it was.
    fn hide_closing_panels(&mut self, id: u32) {
        let Some(group) = self.cloud_prototype.groups.0.iter().find(|group| group.issue == id) else {
            return;
        };
        let members = group.panels.clone();
        for local in members {
            let Some(panel) = self.board.panel_id_by_local_id(&local) else {
                continue;
            };
            if !self.board.hide_for_disposal(panel) {
                continue;
            }
            let hidden = self.cloud_prototype.production.close.hidden.entry(id).or_default();
            if !hidden.contains(&local) {
                hidden.push(local);
            }
        }
    }

    /// Shows the panels the disposal hid. A collapsed cloud shows them when it expands.
    fn restore_closing_panels(&mut self, id: u32) {
        let Some(hidden) = self.cloud_prototype.production.close.hidden.remove(&id) else {
            return;
        };
        let Some(index) = self.cloud_prototype.groups.0.iter().position(|group| group.issue == id) else {
            return;
        };
        let collapsed = self.cloud_prototype.groups.0[index].collapsed;
        for local in hidden {
            if let Some(panel) = self.board.panel_id_by_local_id(&local) {
                self.board.end_disposal_hiding(panel, !collapsed);
            }
            if collapsed {
                self.cloud_prototype.groups.0[index].show_on_expand(&local);
            }
        }
    }

    pub(super) fn finish_closing_clouds(&mut self, ctx: &egui::Context) {
        let closing: Vec<_> = self.cloud_prototype.production.close.deleting.iter().copied().collect();
        for id in closing {
            self.hide_closing_panels(id);
        }
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
                // Returned first, while the panels still exist and drop their disposal marker:
                // removal then closes them, or declines while the record still holds resources
                // and leaves the cloud, with its panels, as it was.
                self.restore_closing_panels(id);
                self.remove_deleted_cloud(id, ctx);
            } else {
                // The deletion stopped short: the cloud stays, with its failure in the header.
                self.restore_closing_panels(id);
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
