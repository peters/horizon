use std::collections::{BTreeMap, BTreeSet};

mod decisions;
mod offer;

use super::{HorizonApp, Runtime, Stage, cards::wording, lifecycle::Action};
pub(super) use decisions::{Origin, RemovalStart};
use offer::{Failure, Primary, Record};

#[derive(Default)]
pub(super) struct State {
    confirming: Option<u32>,
    jobs: super::removal::Jobs,
    owners: BTreeMap<u32, super::removal::Owner>,
    removing: BTreeMap<u32, decisions::Origin>,
    generation: u64,
    deleting: BTreeSet<u32>,
    /// Per cloud, what stopped its close: the dialog then asks again with the reason.
    failed: BTreeMap<u32, Failure>,
    /// Per cloud, what its saved record held when its close was asked.
    records: BTreeMap<u32, Record>,
    /// When the open dialog last read the record of its cloud.
    record_read: Option<std::time::Instant>,
    /// Per closing cloud, the panels that were showing when its disposal took over.
    hidden: BTreeMap<u32, Vec<String>>,
}

impl State {
    pub(super) fn reset(&mut self) {
        self.jobs.reset();
        let jobs = std::mem::take(&mut self.jobs);
        let generation = self.generation;
        *self = Self {
            jobs,
            generation,
            ..Self::default()
        };
    }

    /// A confirmed close is deleting this cloud's resources.
    pub(in crate::app::cloud_panel) fn closing(&self, id: u32) -> bool {
        self.deleting.contains(&id)
    }

    /// The close dialog of cloud `id` is open, or its close is deleting its resources.
    pub(in crate::app::cloud_panel::production) fn open_for(&self, id: u32) -> bool {
        self.confirming == Some(id) || self.closing(id)
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
        if let Some(previous) = self.cloud_prototype.production.close.confirming {
            self.end_close_confirmation(previous);
        }
        if !self.pin_removal_owner(id) {
            return;
        }
        let close = &mut self.cloud_prototype.production.close;
        close.failed.remove(&id);
        close.records.insert(id, Record::Checking);
        close.confirming = Some(id);
        close.record_read = None;
    }

    /// The record of cloud `id` is read again while its dialog is open and this instance
    /// runs no operation of it, so the dialog waits for an operation that another Horizon
    /// instance starts, and offers the close again when that operation ends.
    fn reread_close_record(&mut self, id: u32, ctx: &egui::Context) {
        let production = &self.cloud_prototype.production;
        if production.runtimes.get(&id).is_some_and(Runtime::busy)
            || production.close.jobs.pending(id)
            || production.close.removing.contains_key(&id)
        {
            return;
        }
        let second = std::time::Duration::from_secs(1);
        ctx.request_repaint_after(second);
        if production.close.record_read.is_some_and(|read| read.elapsed() < second) {
            return;
        }
        let Some(owner) = production.close.owners.get(&id).cloned() else {
            return;
        };
        let close = &mut self.cloud_prototype.production.close;
        close.record_read = Some(std::time::Instant::now());
        close.jobs.request(owner, super::removal::Purpose::Display, ctx);
    }

    /// Forgets the dialog and what an earlier attempt of cloud `id`'s close left.
    fn end_close_confirmation(&mut self, id: u32) {
        self.restore_closing_panels(id);
        let close = &mut self.cloud_prototype.production.close;
        close.deleting.remove(&id);
        close.jobs.cancel(id);
        close.owners.remove(&id);
        close.removing.remove(&id);
        if close.confirming == Some(id) {
            close.confirming = None;
        }
        close.failed.remove(&id);
        close.records.remove(&id);
        close.record_read = None;
    }

    pub(in crate::app::cloud_panel) fn render_cloud_close_confirmation(&mut self, ctx: &egui::Context) {
        self.poll_cloud_removals(ctx);
        let Some(id) = self.cloud_prototype.production.close.confirming else {
            return;
        };
        if !self.removal_owner_matches(id) {
            self.end_close_confirmation(id);
            return;
        }
        self.reread_close_record(id, ctx);
        let Some((group, launch)) = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.issue == id)
            .and_then(|group| Some((group, group.remote.as_ref()?)))
        else {
            self.end_close_confirmation(id);
            return;
        };
        let close = &self.cloud_prototype.production.close;
        let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
        let offer = offer::offer(
            runtime,
            launch.deployment_started,
            close.records.get(&id).unwrap_or(&Record::Holds),
            close.failed.get(&id),
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
                paint_reason(ui, offer.reason.as_deref(), close.records.get(&id), runtime.busy());
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
            self.end_close_confirmation(id);
            return;
        }
        match chosen {
            Some(Choice::Primary(Primary::Remove)) => self.remove_for_close(id, ctx),
            Some(Choice::Primary(Primary::Delete)) => self.delete_for_close(id, ctx),
            Some(Choice::RemoveAnyway) => self.remove_cloud_anyway(id, ctx),
            None => {}
        }
    }

    /// Removes cloud `id` from Horizon after its deletion failed or could not run,
    /// leaving whatever its provider still holds.
    /// An operation that started while the dialog was open keeps it open, which then shows
    /// the operation. A record that cannot be locked keeps the cloud, and the dialog asks
    /// again with the reason.
    fn remove_cloud_anyway(&mut self, id: u32, ctx: &egui::Context) {
        self.begin_cloud_removal(id, decisions::Origin::Anyway, ctx);
    }

    fn remove_for_close(&mut self, id: u32, ctx: &egui::Context) {
        self.begin_cloud_removal(id, decisions::Origin::Close, ctx);
    }

    /// Starts deleting the resources of cloud `id`; the cloud closes once they are gone.
    /// A deletion that cannot start keeps the dialog open with the reason.
    fn delete_for_close(&mut self, id: u32, ctx: &egui::Context) {
        if self
            .cloud_prototype
            .production
            .runtimes
            .get(&id)
            .is_some_and(Runtime::busy)
        {
            return;
        }
        self.cloud_prototype.production.close.failed.remove(&id);
        let (started, error) = self.close_step(id, |app| {
            app.change_production_worker(id, Action::Delete, ctx);
            app.cloud_prototype.production.runtimes.get(&id).is_some_and(|runtime| {
                runtime.receiver.is_some() && runtime.stage.is_some_and(|stage| Stage::DELETION.contains(&stage))
            })
        });
        if started {
            self.cloud_prototype.production.close.confirming = None;
            self.cloud_prototype.production.close.deleting.insert(id);
        } else {
            self.close_failed(id, "Could not delete the cloud resources", true, error);
        }
    }

    /// Runs one step of cloud `id`'s close, which reports whether it went ahead, and
    /// returns the error the step itself reported. An earlier error the card showed is
    /// not the step's: it stays shown only when the step stopped without one.
    fn close_step(&mut self, id: u32, step: impl FnOnce(&mut Self) -> bool) -> (bool, Option<String>) {
        let earlier = self
            .cloud_prototype
            .production
            .runtimes
            .get_mut(&id)
            .and_then(|runtime| runtime.error.take());
        let went_ahead = step(self);
        let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) else {
            return (went_ahead, None);
        };
        if runtime.error.is_none() && !went_ahead {
            runtime.error = earlier;
            return (went_ahead, None);
        }
        (went_ahead, runtime.error.clone())
    }

    /// Records why closing cloud `id` stopped short, and asks again unless another
    /// cloud's close is being asked. `error` is what the attempt itself reported.
    fn close_failed(&mut self, id: u32, what: &str, deletion_tried: bool, error: Option<String>) {
        if !self.cloud_prototype.groups.0.iter().any(|group| group.issue == id) {
            return;
        }
        let error = error.filter(|error| error != super::DELETED_RESOURCES_MESSAGE);
        if !self.cloud_prototype.production.close.owners.contains_key(&id) {
            self.pin_removal_owner(id);
        }
        let close = &mut self.cloud_prototype.production.close;
        close.records.insert(id, Record::Checking);
        close.record_read = None;
        close
            .failed
            .insert(id, Failure::new(what, deletion_tried, error.as_deref()));
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
        let close = &mut self.cloud_prototype.production.close;
        close.jobs.cancel(id);
        close.owners.remove(&id);
        close.removing.remove(&id);
        close.failed.remove(&id);
        close.records.remove(&id);
        close.deleting.remove(&id);
        close.hidden.remove(&id);
        if close.confirming == Some(id) {
            close.confirming = None;
            close.record_read = None;
        }
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
            if self.cloud_prototype.production.close.removing.contains_key(&id) {
                continue;
            }
            if self
                .cloud_prototype
                .production
                .runtimes
                .get(&id)
                .is_some_and(|runtime| runtime.stage == Some(Stage::Deleted))
            {
                self.begin_cloud_removal(id, decisions::Origin::AfterDelete, ctx);
            } else {
                self.cloud_prototype.production.close.deleting.remove(&id);
                // The deletion stopped short: the cloud stays, with its failure in the header,
                // and the dialog asks again, now offering to remove it anyway.
                self.restore_closing_panels(id);
                let error = self
                    .cloud_prototype
                    .production
                    .runtimes
                    .get(&id)
                    .and_then(|runtime| runtime.error.clone());
                self.close_failed(id, "Could not delete the cloud resources", true, error);
            }
        }
    }
}

fn paint_reason(ui: &mut egui::Ui, reason: Option<&str>, record: Option<&Record>, busy: bool) {
    let Some(reason) = reason else {
        return;
    };
    if matches!(record, Some(Record::Checking)) && !busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(reason);
        });
    } else {
        ui.colored_label(crate::theme::PALETTE_RED(), reason);
    }
}

#[cfg(test)]
impl HorizonApp {
    /// Waits only in tests that previously expected a synchronous record decision.
    pub(in crate::app::cloud_panel::production) fn settle_cloud_removals(&mut self) {
        self.settle_cloud_removals_at(&egui::Context::default());
    }

    pub(in crate::app::cloud_panel::production) fn settle_cloud_removals_at(&mut self, ctx: &egui::Context) {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            self.poll_cloud_removals(ctx);
            if let Some(id) = self.cloud_prototype.production.close.confirming
                && self.removal_owner_matches(id)
            {
                self.reread_close_record(id, ctx);
            }
            if self.cloud_prototype.production.close.jobs.idle() {
                return;
            }
            assert!(std::time::Instant::now() < until, "cloud removal did not settle");
            std::thread::sleep(std::time::Duration::from_millis(1));
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
