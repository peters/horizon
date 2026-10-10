use std::collections::{BTreeMap, BTreeSet};

mod offer;

use super::{HorizonApp, Runtime, Stage, cards::wording, lifecycle::Action};
use offer::Primary;

#[derive(Default)]
pub(super) struct State {
    confirming: Option<u32>,
    deleting: BTreeSet<u32>,
    /// Per cloud, what stopped its close: the dialog then offers to remove it anyway.
    failed: BTreeMap<u32, String>,
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

    /// The × of a cloud: its close always offers to delete its resources first, so a
    /// failure from an earlier close, possibly since retried from the card, is forgotten.
    pub(in crate::app::cloud_panel) fn request_cloud_close(&mut self, id: u32) {
        let close = &mut self.cloud_prototype.production.close;
        close.failed.remove(&id);
        close.confirming = Some(id);
    }

    pub(in crate::app::cloud_panel) fn render_cloud_close_confirmation(&mut self, ctx: &egui::Context) {
        let Some(id) = self.cloud_prototype.production.close.confirming else {
            return;
        };
        let Some(group) = self.cloud_prototype.groups.0.iter().find(|group| group.issue == id) else {
            self.cloud_prototype.production.close.confirming = None;
            self.cloud_prototype.production.close.failed.remove(&id);
            return;
        };
        let Some(launch) = &group.remote else { return };
        let close = &self.cloud_prototype.production.close;
        let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
        let offer = offer::offer(
            runtime,
            launch.deployment_started,
            close.failed.get(&id).map(String::as_str),
        );
        let remains = offer
            .remove_anyway
            .then(|| offer::remains(runtime, &launch.profile.provider));
        let mut chosen = None;
        let mut cancelled = false;
        let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let modal = egui::Id::new("cloud-close-confirmation");
        let response = egui::Modal::new(modal)
            .area(egui::Modal::default_area(modal).order(egui::Order::Tooltip))
            .show(ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 64.0).clamp(180.0, 460.0));
                ui.heading(format!("Close {}?", group.title));
                ui.add_space(10.0);
                match offer.primary {
                    Some(Primary::Remove) => {
                        ui.label(
                            "This cloud has no worker or storage at its provider. Remove it from Horizon and close its panels?",
                        );
                    }
                    Some(Primary::Delete) if offer.reason.is_none() => {
                        ui.label(wording::delete_confirmation(runtime));
                        ui.add_space(6.0);
                        ui.label("The cloud is removed from Horizon once they are deleted.");
                    }
                    _ => {}
                }
                if let Some(reason) = &offer.reason {
                    ui.colored_label(crate::theme::PALETTE_RED(), reason);
                }
                if let Some(remains) = &remains {
                    ui.add_space(8.0);
                    ui.label(remains);
                    ui.label("Removing the cloud from Horizon does not delete them.");
                }
                ui.add_space(12.0);
                ui.horizontal_wrapped(|ui| {
                    if let Some(primary) = offer.primary {
                        let label = match primary {
                            Primary::Delete => "Delete cloud resources",
                            Primary::Remove => "Remove cloud",
                        };
                        if ui
                            .add(egui::Button::new(label).fill(crate::theme::BTN_CLOSE()))
                            .clicked()
                        {
                            chosen = Some(Choice::Primary(primary));
                        }
                    }
                    if offer.remove_anyway && ui.button("Remove from Horizon anyway").clicked() {
                        chosen = Some(Choice::RemoveAnyway);
                    }
                    cancelled = ui.button("Cancel").clicked();
                });
            });
        let dismissed = response.should_close() && chosen.is_none();
        if dismissed && escape {
            self.consume_navigation_key(
                ctx,
                horizon_core::ShortcutBinding::new(
                    horizon_core::ShortcutModifiers::NONE,
                    horizon_core::ShortcutKey::Escape,
                ),
            );
        }
        if cancelled || dismissed {
            self.cloud_prototype.production.close.confirming = None;
            self.cloud_prototype.production.close.failed.remove(&id);
            return;
        }
        match chosen {
            Some(Choice::Primary(Primary::Remove)) => {
                if self.remove_deleted_cloud(id, ctx) {
                    self.cloud_prototype.production.close.confirming = None;
                } else {
                    self.close_failed(id, "Could not remove the cloud");
                }
            }
            Some(Choice::Primary(Primary::Delete)) => self.delete_for_close(id, ctx),
            Some(Choice::RemoveAnyway) => self.remove_cloud_anyway(id, ctx),
            None => {}
        }
    }

    /// Removes cloud `id` from Horizon after its deletion failed or could not run,
    /// leaving whatever its provider still holds.
    fn remove_cloud_anyway(&mut self, id: u32, ctx: &egui::Context) {
        self.cloud_prototype.production.close.confirming = None;
        let Some(index) = self.cloud_prototype.groups.0.iter().position(|group| group.issue == id) else {
            return;
        };
        if self
            .cloud_prototype
            .production
            .runtimes
            .get(&id)
            .is_some_and(Runtime::busy)
        {
            return;
        }
        if let Some(launch) = &self.cloud_prototype.groups.0[index].remote {
            tracing::warn!(cloud = %launch.id, "removed from Horizon while its provider resources may remain");
        }
        self.discard_cloud(index, ctx);
    }

    /// Starts deleting the resources of cloud `id`; the cloud closes once they are gone.
    /// A deletion that cannot start keeps the dialog open with the reason.
    fn delete_for_close(&mut self, id: u32, ctx: &egui::Context) {
        self.cloud_prototype.production.close.failed.remove(&id);
        self.change_production_worker(id, Action::Delete, ctx);
        let started = self
            .cloud_prototype
            .production
            .runtimes
            .get(&id)
            .is_some_and(|runtime| {
                runtime.receiver.is_some() && runtime.stage.is_some_and(|stage| Stage::DELETION.contains(&stage))
            });
        if started {
            self.cloud_prototype.production.close.confirming = None;
            self.cloud_prototype.production.close.deleting.insert(id);
        } else {
            self.close_failed(id, "Could not delete the cloud resources");
        }
    }

    /// Records why closing cloud `id` stopped short, and asks again unless another
    /// cloud's close is being asked.
    fn close_failed(&mut self, id: u32, what: &str) {
        let error = self
            .cloud_prototype
            .production
            .runtimes
            .get(&id)
            .and_then(|runtime| runtime.error.as_deref())
            .filter(|error| *error != super::DELETED_RESOURCES_MESSAGE);
        let reason = error.map_or_else(|| format!("{what}."), |error| format!("{what}: {error}"));
        let close = &mut self.cloud_prototype.production.close;
        close.failed.insert(id, reason);
        close.confirming.get_or_insert(id);
    }

    /// Takes cloud `index` out of Horizon with its panels. Its saved record stays on disk.
    pub(super) fn discard_cloud(&mut self, index: usize, ctx: &egui::Context) {
        let id = self.cloud_prototype.groups.0[index].issue;
        if self
            .cloud_prototype
            .fullscreen
            .as_ref()
            .is_some_and(|view| view.id == id)
        {
            self.exit_cloud_fullscreen(ctx);
        }
        let group = self.cloud_prototype.groups.0.remove(index);
        for local in group.panels {
            if let Some(panel) = self.board.panel_id_by_local_id(&local) {
                self.board.close_panel(panel);
                self.panel_render_caches.browser_ui_state.remove(&panel);
                self.panel_render_caches.device_ui_state.remove(&panel);
                self.panel_render_caches.terminal_grid_cache.remove(&panel);
            }
        }
        self.cloud_prototype.production.runtimes.remove(&id);
        self.cloud_prototype.production.close.failed.remove(&id);
        super::cards::forget_log_heights(ctx, id);
        self.save_cloud_prototype();
        self.release_removed_cloud_workspace(&group.workspace, ctx);
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

    /// Keeps every closing cloud's panels out of sight. Also run just before panels render,
    /// because a header action such as Expand runs between frame preparation and rendering.
    pub(in crate::app) fn hide_closing_cloud_panels(&mut self) {
        let closing: Vec<_> = self.cloud_prototype.production.close.deleting.iter().copied().collect();
        for id in closing {
            self.hide_closing_panels(id);
        }
    }

    pub(super) fn finish_closing_clouds(&mut self, ctx: &egui::Context) {
        self.hide_closing_cloud_panels();
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
                if !self.remove_deleted_cloud(id, ctx) {
                    self.close_failed(id, "Could not remove the cloud");
                }
            } else {
                // The deletion stopped short: the cloud stays, with its failure in the header,
                // and the dialog asks again, now offering to remove it anyway.
                self.restore_closing_panels(id);
                self.close_failed(id, "Could not delete the cloud resources");
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Choice {
    Primary(Primary),
    RemoveAnyway,
}

#[cfg(test)]
mod tests;
