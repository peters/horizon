use super::{Confirmation, HorizonApp, Stage, Store, cloud_runtime, lifecycle::Action};
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
        let mut resize = None;
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
                        if let Some(size) = profile_details(ui, group.issue, launch, runtime) {
                            resize = Some((group.issue, size));
                        }
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
        if let Some((id, size)) = resize {
            self.resize_production_cloud(id, size);
        }
        if let Some((id, layout)) = layout
            && let Some(index) = self.cloud_prototype.groups.0.iter().position(|g| g.issue == id)
        {
            self.cloud_prototype.groups.0[index].set_layout(&mut self.board, layout);
            self.cloud_prototype.groups.make_room(&mut self.board, index);
            self.save_cloud_prototype();
        }
    }

    /// The next deployment attempt adopts the size. The saved record, not the
    /// cached snapshot, decides whether a worker may already be requested.
    fn resize_production_cloud(&mut self, id: u32, (cpu, memory_gb): (u16, u16)) {
        let Some(index) = self.cloud_prototype.groups.0.iter().position(|group| group.issue == id) else {
            return;
        };
        let Some(launch) = self.cloud_prototype.groups.0[index].remote.clone() else {
            return;
        };
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        let loaded = cloud_runtime::state::cloud_directory(&root, &launch.id)
            .and_then(|path| Store::lock(&path))
            .and_then(|store| store.load());
        let allowed = match loaded {
            Ok(Some(state)) if !state.resizable() => {
                let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
                runtime.stage = Some(state.stage);
                runtime.state = Some(state);
                false
            }
            Ok(Some(_)) => true,
            Ok(None) => !launch.deployment_started,
            Err(error) => {
                self.cloud_prototype.error = Some(error.to_string());
                return;
            }
        };
        if !allowed {
            self.cloud_prototype.error =
                Some("A worker was requested for this cloud; its size can no longer change".into());
            return;
        }
        if let Some(launch) = self.cloud_prototype.groups.0[index].remote.as_mut() {
            launch.profile.cpu = cpu;
            launch.profile.memory_gb = memory_gb;
        }
        self.save_cloud_prototype();
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

fn profile_details(
    ui: &mut egui::Ui,
    id: u32,
    launch: &horizon_core::cloud_panel::CloudLaunch,
    runtime: &super::Runtime,
) -> Option<(u16, u16)> {
    if ui.small_button("Copy cloud ID").on_hover_text(&launch.id).clicked() {
        ui.ctx().copy_text(launch.id.clone());
    }
    ui.label(RichText::new(&launch.profile_name).size(15.0).color(theme::FG_DIM()));
    let resize = machine_size(ui, id, launch, runtime);
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
    resize
}

/// CPU and memory can change until a worker is requested; `RunPod` cannot resize an existing pod.
fn machine_size(
    ui: &mut egui::Ui,
    id: u32,
    launch: &horizon_core::cloud_panel::CloudLaunch,
    runtime: &super::Runtime,
) -> Option<(u16, u16)> {
    use horizon_core::cloud_runtime::flavors::{memory_options, offered, resize_vcpu, vcpu_options};
    let profile = &launch.profile;
    let kind = if profile.gpu { "GPU" } else { "CPU only" };
    let idle = runtime.receiver.is_none() && runtime.recovery_receiver.is_none() && !runtime.state_unavailable;
    if profile.gpu
        || !idle
        || !runtime
            .state
            .as_ref()
            .is_none_or(horizon_core::cloud_runtime::state::Deployment::resizable)
    {
        // A requested worker's saved size is authoritative.
        let fixed = runtime.state.as_ref().filter(|state| !state.resizable());
        let shown = fixed.map_or(profile, |state| &state.profile);
        let label = ui.label(format!("{} vCPU · {} GB · {kind}", shown.cpu, shown.memory_gb));
        if fixed.is_some() {
            label.on_hover_text("RunPod cannot resize a requested worker. Create a new cloud for a different size.");
        }
        return None;
    }
    let hint = format!(
        "CPU-only RunPod worker. Only sizes offered with this cloud's {} GB container disk are listed.",
        profile.storage.container_gb
    );
    let mut size = None;
    // Top alignment keeps equally tall drop-downs level when the theme pads them above row height.
    ui.horizontal_top(|ui| {
        egui::ComboBox::from_id_salt(("cloud-vcpu", id))
            .selected_text(format!("{} vCPU", profile.cpu))
            .show_ui(ui, |ui| {
                for cpu in vcpu_options(profile.storage.container_gb) {
                    if ui.selectable_label(cpu == profile.cpu, format!("{cpu} vCPU")).clicked() && cpu != profile.cpu {
                        size = resize_vcpu(profile, cpu);
                    }
                }
            })
            .response
            .on_hover_text(&hint);
        egui::ComboBox::from_id_salt(("cloud-memory", id))
            .selected_text(format!("{} GB", profile.memory_gb))
            .show_ui(ui, |ui| {
                for (memory, family) in memory_options(profile.cpu, profile.storage.container_gb) {
                    if ui
                        .selectable_label(memory == profile.memory_gb, format!("{memory} GB · {family}"))
                        .clicked()
                        && memory != profile.memory_gb
                    {
                        size = Some((profile.cpu, memory));
                    }
                }
            })
            .response
            .on_hover_text(&hint);
    });
    if !offered(profile) {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            format!(
                "RunPod offers no CPU worker with this size and {} GB container disk",
                profile.storage.container_gb
            ),
        );
    }
    size
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
        ui.label(runtime.deleted_message());
        return ui.button("Remove cloud").clicked().then_some(Action::Remove);
    }
    progress_output(ui, runtime);
    ui.add_space(8.0);
    if runtime.remote_release.is_some() {
        ui.spinner();
        ui.label("Releasing remote devices…");
        return None;
    }
    if runtime.recovery_receiver.is_some()
        || (runtime.receiver.is_none() && runtime.state.as_ref().is_some_and(super::Runtime::needs_provider_check))
    {
        return recovery_actions(ui, runtime);
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
    bound_provider_check(ui, runtime)
        .or_else(|| deletion_action(ui, runtime))
        .or(action)
}

fn bound_provider_check(ui: &mut egui::Ui, runtime: &super::Runtime) -> Option<Action> {
    if runtime.receiver.is_none()
        && runtime
            .state
            .as_ref()
            .is_some_and(|state| matches!(state.operation, horizon_core::cloud_runtime::CreateState::Bound { .. }))
    {
        return ui.button("Check provider").clicked().then_some(Action::Reconcile);
    }
    None
}

fn deletion_action(ui: &mut egui::Ui, runtime: &mut super::Runtime) -> Option<Action> {
    if runtime.state.is_some() {
        if runtime.confirmation == Confirmation::Delete {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                if runtime.retains_network_volume() {
                    "Delete this worker? Running sessions cannot be recovered. Its network volume, files and credentials remain and storage charges continue."
                } else {
                    "Delete this worker and its files? Running sessions cannot be recovered."
                },
            );
            if ui.button("Delete worker permanently").clicked() {
                return Some(Action::Delete);
            }
            if ui.button("Keep worker").clicked() {
                runtime.confirmation = Confirmation::None;
            }
        } else if ui.button("Delete worker…").clicked() {
            runtime.confirmation = Confirmation::Delete;
        }
    }
    None
}

fn recovery_actions(ui: &mut egui::Ui, runtime: &mut super::Runtime) -> Option<Action> {
    ui.label("Worker status needs confirmation");
    ui.small("Check the original request with the provider. This check cannot allocate, start or delete a worker.");
    ui.collapsing("Provider-confirmed worker ID (optional)", |ui| {
        ui.small("Use an ID supplied by the provider. Horizon verifies that it belongs to this cloud.");
        ui.add(egui::TextEdit::singleline(&mut runtime.recovery_worker_id));
    });
    if runtime.recovery_receiver.is_some() {
        ui.spinner();
        ui.label("Checking provider…");
        None
    } else {
        let action = ui.button("Check provider").clicked().then_some(Action::Reconcile);
        if runtime
            .state
            .as_ref()
            .is_some_and(|state| matches!(state.operation, horizon_core::cloud_runtime::CreateState::Bound { .. }))
        {
            deletion_action(ui, runtime).or(action)
        } else {
            action
        }
    }
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
    if runtime.stage == Some(Stage::Ready)
        && let Some(seconds) = runtime.state.as_ref().and_then(|state| state.ready_after_seconds)
    {
        ui.small(format!(
            "Worker ready in {}",
            horizon_core::cloud_runtime::progress::duration(std::time::Duration::from_secs(seconds))
        ))
        .on_hover_text(
            "Time for the successful deployment attempt. Application startup and reconnect are measured separately.",
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
