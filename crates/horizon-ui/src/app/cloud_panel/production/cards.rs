use super::{Confirmation, HorizonApp, Stage, lifecycle::Action};
use crate::{app::view::canvas_scene_transform, theme};
use egui::{Id, Order, Pos2, RichText, Stroke, Vec2};
use horizon_core::cloud_panel::{RUNTIME_HEIGHT, RUNTIME_WIDTH};
#[cfg(test)]
mod tests;
impl HorizonApp {
    pub(in crate::app::cloud_panel) fn render_production_runtimes(&mut self, ctx: &egui::Context) {
        self.ensure_cloud_provider_logo(ctx);
        let canvas = self.canvas_rect(ctx);
        let transform = canvas_scene_transform(canvas, self.canvas_view);
        let clip = transform.inverse() * canvas;
        let mut action = None;
        let mut fullscreen = None;
        let mut layout = None;
        for group in &self.cloud_prototype.groups.0 {
            let Some(launch) = &group.remote else { continue };
            if self
                .cloud_prototype
                .fullscreen
                .as_ref()
                .is_some_and(|f| f.id != group.issue)
            {
                continue;
            }
            let runtime = self.cloud_prototype.production.runtimes.entry(group.issue).or_default();
            egui::Area::new(Id::new(("cloud-runtime", group.issue)))
                .order(Order::Middle)
                .fixed_pos(Pos2::from(group.runtime_bounds().0))
                .constrain(false)
                .show(ctx, |ui| {
                    ctx.set_transform_layer(ui.layer_id(), transform);
                    ui.set_clip_rect(clip);
                    runtime_frame(ui, group.issue, |ui| {
                        super::super::runtime::runtime_heading(ui, group, self.cloud_prototype.provider_logo.as_ref());
                        profile_details(ui, launch);
                        ui.add_space(10.0);
                        ui.label("Panel layout");
                        let mut selected = group.layout;
                        if ui
                            .horizontal(|ui| {
                                crate::app::workspace::workspace_layout_buttons(
                                    ui,
                                    &mut selected,
                                    theme::workspace_accent(group.issue.saturating_sub(101) as usize),
                                )
                            })
                            .inner
                        {
                            layout = Some((group.issue, selected));
                        }
                        if ui
                            .button(if self.cloud_prototype.fullscreen.is_some() {
                                "Exit full screen"
                            } else {
                                "Full screen"
                            })
                            .clicked()
                        {
                            fullscreen = Some(group.issue);
                        }
                        if runtime.can_release_remote_devices()
                            && ui.button("Release devices and remove remote credentials").on_hover_text("Stops this cloud’s hosted browser sessions and private tunnel, then deletes its copied credentials. Reconnect transfers them again only while the local grant remains configured.").clicked()
                        {
                            action = Some((group.issue, Action::RevokeBrowserstack));
                        }
                        if let Some(next) = runtime_actions(ui, runtime) {
                            action = Some((group.issue, next));
                        }
                    });
                });
        }
        if let Some((id, action)) = action {
            match action {
                Action::Deploy => self.start_production_deployment(id, ctx),
                Action::Desktop => self.cloud_add_panel(ctx, id, horizon_core::PanelKind::Device, None),
                Action::Remove => self.remove_deleted_cloud(id, ctx),
                _ => self.change_production_worker(id, action, ctx),
            }
        }
        if let Some(id) = fullscreen {
            self.toggle_cloud_fullscreen(ctx, id);
        }
        if let Some((id, layout)) = layout
            && let Some(index) = self.cloud_prototype.groups.0.iter().position(|g| g.issue == id)
        {
            self.cloud_prototype.groups.0[index].set_layout(&mut self.board, layout);
            self.cloud_prototype.groups.make_room(&mut self.board, index);
            self.save_cloud_prototype();
        }
    }
}

fn runtime_frame(ui: &mut egui::Ui, id: u32, contents: impl FnOnce(&mut egui::Ui)) -> egui::InnerResponse<()> {
    let frame = egui::Frame::new()
        .fill(theme::BG_ELEVATED())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(14)
        .inner_margin(18);
    let inner = Vec2::new(RUNTIME_WIDTH, RUNTIME_HEIGHT) - frame.total_margin().sum();
    frame.show(ui, |ui| {
        ui.set_width(inner.x);
        egui::ScrollArea::vertical()
            .id_salt(("cloud-runtime-body", id))
            .max_height(inner.y)
            .min_scrolled_height(inner.y)
            .auto_shrink([false, false])
            .show(ui, contents);
    })
}

fn profile_details(ui: &mut egui::Ui, launch: &horizon_core::cloud_panel::CloudLaunch) {
    ui.label(RichText::new(&launch.profile_name).size(15.0).color(theme::FG_DIM()));
    ui.label(format!(
        "{} vCPU · {} GB · {}",
        launch.profile.cpu,
        launch.profile.memory_gb,
        if launch.profile.gpu { "GPU" } else { "CPU only" }
    ));
    ui.label(RichText::new(&launch.profile.image).monospace().size(12.0));
    ui.small(format!(
        "Agents: {}",
        if launch.profile.capabilities.agents.is_empty() {
            "none".into()
        } else {
            launch.profile.capabilities.agents_argument()
        }
    ));
    ui.small(format!(
        "Browsers: {}",
        if launch.profile.capabilities.browsers.is_empty() {
            "disabled".into()
        } else {
            launch.profile.capabilities.browsers_argument()
        }
    ));
    if let Some(selected) = &launch.profile.capabilities.browserstack {
        ui.small(format!("Remote account: {}", selected.provider));
    }
    ui.small(if launch.profile.capabilities.desktop {
        "Desktop: enabled"
    } else {
        "Desktop: disabled"
    });
}

fn runtime_actions(ui: &mut egui::Ui, runtime: &mut super::Runtime) -> Option<Action> {
    let mut action = None;
    ui.separator();
    if runtime.state.as_ref().is_some_and(|state| {
        matches!(
            state.operation,
            horizon_core::cloud_runtime::CreateState::Terminated { .. }
        )
    }) {
        ui.label("Worker deleted. Its processes and files are no longer available.");
        return ui.button("Remove cloud").clicked().then_some(Action::Remove);
    }
    progress_output(ui, runtime);
    ui.add_space(8.0);
    if runtime.remote_release.is_some() {
        ui.spinner();
        ui.label("Releasing remote devices…");
        return None;
    }
    if !runtime.state_unavailable
        && runtime.receiver.is_none()
        && runtime
            .state
            .as_ref()
            .is_none_or(|state| state.operation == horizon_core::cloud_runtime::CreateState::Prepared)
        && ui.button("Remove cloud").clicked()
    {
        return Some(Action::Remove);
    }
    if runtime.receiver.is_some() && runtime.stage != Some(Stage::Ready) {
        if ui.button("Cancel operation").clicked()
            && let Some(cancel) = &runtime.cancel
        {
            cancel.cancel();
        }
    } else if runtime.stage == Some(Stage::Stopping) {
        ui.label("Stop requested; provider confirmation is pending.");
        if ui.button("Reconcile stop").clicked() {
            action = Some(Action::Stop);
        }
    } else if runtime.stage == Some(Stage::Stopped) {
        ui.label("Stopped. Storage can remain billable; previous processes may be lost.");
        if ui.button("Resume worker").clicked() {
            action = Some(Action::Resume);
        }
    } else if ui
        .add(
            egui::Button::new(if runtime.state.is_some() {
                "Reconnect cloud"
            } else {
                "Deploy cloud"
            })
            .min_size(Vec2::new(ui.available_width(), 34.0))
            .fill(theme::blend(theme::PANEL_BG(), theme::ACCENT(), 0.20)),
        )
        .clicked()
    {
        action = Some(Action::Deploy);
    }
    if let Some(worker) = runtime.state.as_ref().and_then(|s| s.worker.as_ref())
        && let Some(rate) = worker.cost_per_hr
    {
        ui.label(format!("Worker rate: ${rate:.3}/hour"));
    }
    ui.small("Sessions continue while disconnected.");
    if runtime.stage == Some(Stage::Ready) {
        if desktop_button(ui, runtime) {
            action = Some(Action::Desktop);
        }
        if runtime.confirmation == Confirmation::Stop {
            ui.label("Stop this worker? Running processes will end. Storage remains billable.");
            if ui.button("Stop worker").clicked() {
                action = Some(Action::Stop);
            }
            if ui.button("Keep running").clicked() {
                runtime.confirmation = Confirmation::None;
            }
        } else if ui.button("Stop worker…").clicked() {
            runtime.confirmation = Confirmation::Stop;
        }
    }
    if runtime.state.is_some() {
        if runtime.confirmation == Confirmation::Delete {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                "Delete this worker and its files? Running sessions cannot be recovered.",
            );
            if ui.button("Delete worker permanently").clicked() {
                action = Some(Action::Delete);
            }
            if ui.button("Keep worker").clicked() {
                runtime.confirmation = Confirmation::None;
            }
        } else if ui.button("Delete worker…").clicked() {
            runtime.confirmation = Confirmation::Delete;
        }
    }
    action
}

fn desktop_button(ui: &mut egui::Ui, runtime: &super::Runtime) -> bool {
    let enabled = runtime
        .state
        .as_ref()
        .is_some_and(|state| state.profile.capabilities.desktop);
    ui.add_enabled(
        enabled && runtime.desktop.is_some(),
        egui::Button::new("Add desktop viewer"),
    )
    .on_disabled_hover_text(if enabled {
        "Desktop tunnel is not connected"
    } else {
        "Desktop is disabled by this cloud profile"
    })
    .clicked()
}

fn progress_output(ui: &mut egui::Ui, runtime: &super::Runtime) {
    for stage in Stage::ALL {
        let current = runtime.stage == Some(stage);
        ui.label(
            RichText::new(runtime.progress.stage_label(stage))
                .size(14.0)
                .color(if current {
                    theme::PALETTE_CYAN()
                } else {
                    theme::FG_DIM()
                }),
        );
    }
    runtime.progress.render(ui);
    for error in runtime.error.iter().chain(&runtime.remote_release_error) {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    egui::CollapsingHeader::new("Verbose output").show(ui, |ui| {
        for line in &runtime.logs {
            ui.label(RichText::new(line).monospace().size(11.0));
        }
    });
}
