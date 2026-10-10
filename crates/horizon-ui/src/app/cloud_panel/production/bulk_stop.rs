//! The bulk stop of the sidebar: the user chooses which idle clouds of a group stop
//! their workers, and sees what that saves each hour before confirming.
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

use horizon_core::cloud_list::{self, Group};

use super::{HorizonApp, lifecycle::Action};
use crate::app::sidebar::IdleCloud;

/// How long a confirmed stop of a parked cloud waits for a new status read of its
/// sessions. When no read arrives in this time, the cloud keeps running.
const STATUS_WAIT: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(super) struct State {
    /// The open dialog; `None` while it is closed.
    dialog: Option<Dialog>,
    /// Confirmed stops of parked clouds, with the time of the confirmation. Each
    /// waits for a status read that started after it: an agent that started to work
    /// since the last read keeps its worker.
    waiting: Vec<(u32, Instant)>,
}

struct Dialog {
    /// The group whose idle clouds the dialog offers.
    group: Group,
    clouds: Vec<IdleCloud>,
    /// The clouds the user took out of the stop. All are chosen at first.
    unchecked: BTreeSet<u32>,
}

impl Dialog {
    fn chosen(&self) -> impl Iterator<Item = &IdleCloud> {
        self.clouds
            .iter()
            .filter(|idle| !self.unchecked.contains(&idle.cloud.id))
    }
}

impl HorizonApp {
    pub(in crate::app) fn idle_stop_open(&self) -> bool {
        self.cloud_prototype.production.bulk_stop.dialog.is_some()
    }

    /// Opens the bulk stop for the idle clouds of `group` that the sidebar read last.
    pub(in crate::app) fn request_idle_stop(&mut self, group: Group) {
        let clouds = self.sidebar_idle_clouds(group);
        if clouds.is_empty() {
            return;
        }
        self.cloud_prototype.production.bulk_stop.dialog = Some(Dialog {
            group,
            clouds,
            unchecked: BTreeSet::new(),
        });
    }

    pub(in crate::app::cloud_panel) fn render_idle_stop_confirmation(&mut self, ctx: &egui::Context) {
        let Some(state) = self.cloud_prototype.production.bulk_stop.dialog.as_mut() else {
            return;
        };
        let group = state.group;
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
                    "These clouds in {} can stop now, and no agent works on them.",
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
        let dialog = self.cloud_prototype.production.bulk_stop.dialog.take();
        if confirmed && let Some(dialog) = dialog {
            self.stop_chosen(&dialog, ctx, Instant::now());
        }
    }

    /// Stops the workers of the clouds chosen in `dialog`. The list is up to a second
    /// old, so a cloud that got busy since keeps running. The status of a parked cloud
    /// can be older: its stop waits for a new read.
    fn stop_chosen(&mut self, dialog: &Dialog, ctx: &egui::Context, now: Instant) {
        for id in dialog.chosen().map(|idle| idle.cloud.id) {
            // A parked cloud without a parked terminal has no session status to wait for.
            let reads = self.cloud_has_parked_terminal(id);
            let runtime = self.cloud_prototype.production.runtimes.get_mut(&id);
            if let Some(runtime) = runtime.filter(|runtime| reads && runtime.parking.is_parked()) {
                runtime.parking.read_now(now);
                self.cloud_prototype.production.bulk_stop.waiting.push((id, now));
                ctx.request_repaint();
            } else if self.cloud_can_stop_now(id) {
                self.change_production_worker(id, Action::Stop, ctx);
            }
        }
    }

    /// Stops each waiting parked cloud that a status read after its confirmation shows
    /// idle. A cloud that attached since is judged by its live terminals.
    pub(in crate::app::cloud_panel) fn finish_waiting_stops(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let waiting = std::mem::take(&mut self.cloud_prototype.production.bulk_stop.waiting);
        for (id, confirmed) in waiting {
            let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) else {
                continue;
            };
            if !runtime.parking.is_parked() || runtime.parking.read_since(confirmed) {
                if self.cloud_can_stop_now(id) {
                    self.change_production_worker(id, Action::Stop, ctx);
                }
            } else if now.duration_since(confirmed) < STATUS_WAIT {
                // A read in flight that started before the confirmation does not count.
                runtime.parking.read_now(now);
                self.cloud_prototype.production.bulk_stop.waiting.push((id, confirmed));
                ctx.request_repaint_after(Duration::from_secs(1));
            }
        }
    }

    /// Whether a terminal of cloud `id` is parked now, so a status read answers for it.
    fn cloud_has_parked_terminal(&self, id: u32) -> bool {
        self.cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.issue == id)
            .is_some_and(|group| {
                group.panels.iter().any(|local| {
                    self.board
                        .panel_id_by_local_id(local)
                        .and_then(|panel| self.board.panel(panel))
                        .is_some_and(|panel| panel.cloud_wait() == Some(horizon_core::CloudWait::Parked))
                })
            })
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
