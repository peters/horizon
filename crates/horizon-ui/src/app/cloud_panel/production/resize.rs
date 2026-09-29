//! Resource editing, confirmation and asynchronous recovery for a running cloud.
use super::{Event, HorizonApp, Runtime, Settings, Stage, Store, cloud_runtime, deployment, lifecycle::Action};
use crate::app::cloud_panel::runtime::{action_button, danger_button};
use deployment::ResizeTarget;
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, TryRecvError, channel},
};

#[derive(Default)]
pub(super) struct State {
    draft: Option<ResizeTarget>,
    confirming: bool,
    pub(super) pending: Option<ResizeTarget>,
    notice: Option<String>,
    result: Option<Job>,
}

struct Job {
    root: PathBuf,
    receiver: Receiver<Outcome>,
}

struct Outcome {
    pending: Option<ResizeTarget>,
    unavailable: bool,
    notice: Option<String>,
}

struct Completion {
    outcome: Outcome,
    ready: Option<cloud_runtime::state::Deployment>,
}

impl State {
    pub(super) fn busy(&self) -> bool {
        self.result.is_some()
    }
}

pub(super) fn restored(root: &Path) -> State {
    State {
        pending: deployment::pending_resize(root).ok().flatten(),
        ..State::default()
    }
}

impl Runtime {
    pub(super) fn poll_resize(&mut self) {
        let Some(job) = &self.resize.result else { return };
        let outcome = match job.receiver.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                let pending = deployment::pending_resize(&job.root);
                let loaded = Store::lock(&job.root).and_then(|store| store.load());
                let unavailable = pending.is_err() || loaded.is_err();
                self.state = loaded.ok().flatten();
                self.stage = self.state.as_ref().map(|state| state.stage);
                self.progress.finish(std::time::Instant::now());
                self.receiver = None;
                self.sender = None;
                self.idle_reports = None;
                if let Some(cancel) = self.cancel.take() {
                    cancel.cancel();
                }
                Outcome {
                    pending: pending.ok().flatten(),
                    unavailable,
                    notice: Some("Resize worker ended without a result; retry the pending resize or reconnect.".into()),
                }
            }
        };
        self.resize.pending = outcome.pending;
        self.resize.notice = outcome.notice;
        self.state_unavailable = outcome.unavailable;
        self.resize.result = None;
    }
}

fn supported(runtime: &Runtime) -> bool {
    !runtime.state_unavailable
        && runtime.state.as_ref().is_some_and(|state| {
            state.profile.provider == "runpod"
                && !state.profile.gpu
                && state.worker_ready()
                && !state.stop_requested
                && state.image_replacement.is_none()
                && !state.requires_browserstack_release()
        })
}

pub(super) fn controls(ui: &mut egui::Ui, id: u32, runtime: &mut Runtime) -> Option<Action> {
    if let Some(notice) = &runtime.resize.notice {
        ui.colored_label(egui::Color32::LIGHT_RED, notice);
    }
    if let Some(target) = runtime.resize.pending {
        ui.label(format!("Resize pending: {}", description(target)));
        ui.small("Retry the same change to finish recovery before other cloud operations.");
        return ui
            .add_enabled(!runtime.busy(), action_button("Retry resize"))
            .clicked()
            .then_some(Action::Resize(target));
    }
    if runtime.busy() {
        return None;
    }
    if !supported(runtime) {
        return None;
    }
    let state = runtime.state.as_ref()?;
    let profile = &state.profile;
    let current = (profile.cpu, profile.memory_gb);
    let workspace = profile.storage.volume_gb;
    let max_workspace = u16::try_from(cloud_runtime::provider::RUNPOD.cpu_volume_gb.1).unwrap_or(u16::MAX);
    ui.label(format!("Workspace: {workspace} GB"));
    let Some(mut target) = runtime.resize.draft else {
        ui.horizontal_wrapped(|ui| {
            if ui.add(action_button("Resize compute…")).clicked() {
                runtime.resize.draft = Some(ResizeTarget::Compute {
                    cpu: current.0,
                    memory_gb: current.1,
                });
            }
            if ui
                .add_enabled(workspace < max_workspace, action_button("Grow workspace…"))
                .clicked()
            {
                runtime.resize.draft = Some(ResizeTarget::Workspace {
                    size_gb: workspace.saturating_add(10).min(max_workspace),
                });
            }
        });
        return None;
    };
    if runtime.resize.confirming {
        ui.label(format!("Apply {}?", description(target)));
        ui.small(match target {
            ResizeTarget::Compute { .. } => "This replaces the worker. Processes stop; recorded sessions reconnect on the new worker. Workspace files stay on the same network volume. Container-disk files are lost. The new size changes compute charges and depends on available capacity.",
            ResizeTarget::Workspace { .. } => "The workspace grows without replacing the worker. Storage charges increase. This cannot be undone or shrunk.",
        });
        if ui.add(danger_button("Confirm resize")).clicked() {
            runtime.resize.draft = None;
            runtime.resize.confirming = false;
            return Some(Action::Resize(target));
        }
        if ui.add(action_button("Back")).clicked() {
            runtime.resize.confirming = false;
        }
    } else {
        runtime.resize.confirming = edit(ui, id, &mut target, profile);
        runtime.resize.draft = Some(target);
    }
    if ui.add(action_button("Cancel resize")).clicked() {
        runtime.resize.draft = None;
        runtime.resize.confirming = false;
    }
    None
}

fn edit(ui: &mut egui::Ui, id: u32, target: &mut ResizeTarget, profile: &cloud_runtime::prices::Profile) -> bool {
    let current = (profile.cpu, profile.memory_gb);
    let disk = profile.storage.container_gb;
    let workspace = profile.storage.volume_gb;
    let max_workspace = u16::try_from(cloud_runtime::provider::RUNPOD.cpu_volume_gb.1).unwrap_or(u16::MAX);
    match target {
        ResizeTarget::Compute { cpu, memory_gb } => {
            let mut selected = (*cpu, *memory_gb);
            ui.horizontal_top(|ui| {
                egui::ComboBox::from_id_salt(("running-cpu", id))
                    .selected_text(format!("{} vCPU", selected.0))
                    .show_ui(ui, |ui| {
                        if let Some(size) = super::machine_size::vcpu(selected, disk, |label, chosen| {
                            ui.selectable_label(chosen, label).clicked()
                        }) {
                            selected = size;
                        }
                    });
                egui::ComboBox::from_id_salt(("running-memory", id))
                    .selected_text(format!("{} GB", selected.1))
                    .show_ui(ui, |ui| {
                        if let Some(size) = super::machine_size::memory(selected, disk, |label, chosen| {
                            ui.selectable_label(chosen, label).clicked()
                        }) {
                            selected = size;
                        }
                    });
            });
            (*cpu, *memory_gb) = selected;
        }
        ResizeTarget::Workspace { size_gb } => {
            let label = ui.label("New workspace size");
            ui.add(
                egui::DragValue::new(size_gb)
                    .range(workspace.saturating_add(1)..=max_workspace)
                    .suffix(" GB"),
            )
            .labelled_by(label.id);
        }
    }
    let changed = match *target {
        ResizeTarget::Compute { cpu, memory_gb } => {
            (cpu, memory_gb) != current && super::machine_size::unoffered((cpu, memory_gb), disk).is_none()
        }
        ResizeTarget::Workspace { size_gb } => size_gb > workspace && size_gb <= max_workspace,
    };
    if ui.add_enabled(changed, action_button("Review resize…")).clicked() {
        return true;
    }
    false
}

fn description(target: ResizeTarget) -> String {
    match target {
        ResizeTarget::Compute { cpu, memory_gb } => format!("{cpu} vCPU and {memory_gb} GB memory"),
        ResizeTarget::Workspace { size_gb } => format!("{size_gb} GB workspace"),
    }
}

impl HorizonApp {
    pub(super) fn start_production_resize(&mut self, id: u32, target: ResizeTarget, ctx: &egui::Context) {
        let Some(launch) = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|g| g.issue == id)
            .and_then(|g| g.remote.clone())
        else {
            return;
        };
        let Some(root) = self.cloud_prototype.root.as_ref() else {
            return;
        };
        let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
        if runtime.busy() {
            return;
        }
        let setup = cloud_runtime::state::cloud_directory(root, &launch.id).and_then(|state_root| {
            Settings::for_cloud(&root.join("settings.json"), &launch.placement).map(|settings| (state_root, settings))
        });
        let (root, settings) = match setup {
            Ok(setup) => setup,
            Err(error) => {
                runtime.fail_preflight(error.to_string());
                return;
            }
        };
        if let Some(cancel) = runtime.cancel.take() {
            cancel.cancel();
        }
        runtime.idle_reports = None;
        runtime.desktop = None;
        runtime.rebuild = None;
        runtime.confirmation = super::Confirmation::None;
        runtime.progress.reset();
        runtime.operation = None;
        runtime.stage = Some(Stage::Provision);
        runtime.error = None;
        runtime.resize.notice = None;
        let (tx, rx) = channel();
        let (result_tx, result_rx) = channel();
        runtime.resize.result = Some(Job {
            root: root.clone(),
            receiver: result_rx,
        });
        runtime.receiver = Some(rx);
        runtime.sender = Some(tx.clone());
        let cancel = cloud_runtime::Cancellation::default();
        runtime.cancel = Some(cancel.clone());
        let idle = runtime.listen_idle();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let emit = |event| {
                let _ = tx.send(event);
                ctx.request_repaint();
            };
            emit(Event::Output(format!("Resizing to {}", description(target))));
            let result = match target {
                ResizeTarget::Compute { cpu, memory_gb } => {
                    deployment::resize_compute(&root, &settings, cpu, memory_gb, &cancel, &emit)
                }
                ResizeTarget::Workspace { size_gb } => deployment::grow_storage(&root, &settings, size_gb, &cancel)
                    .inspect(|state| emit(Event::ready(Box::new(state.clone())))),
            };
            let completion = complete(&root, result, &cancel, &emit);
            let _ = result_tx.send(completion.outcome);
            if let Some(state) = completion.ready {
                super::idle::watch(&state, &settings, &root, &cancel, idle, &ctx);
                super::presentation::watch(&state, &settings, &root, &cancel, &tx, &ctx);
            }
            ctx.request_repaint();
        });
    }

    pub(super) fn sync_resized_profiles(&mut self) {
        let mut changed = false;
        for group in &mut self.cloud_prototype.groups.0 {
            let Some(launch) = &mut group.remote else { continue };
            let Some(state) = self
                .cloud_prototype
                .production
                .runtimes
                .get(&group.issue)
                .and_then(|r| r.state.as_ref())
            else {
                continue;
            };
            if !matches!(state.operation, cloud_runtime::CreateState::Bound { .. }) {
                continue;
            }
            if launch.profile.cpu != state.profile.cpu
                || launch.profile.memory_gb != state.profile.memory_gb
                || launch.profile.storage != state.profile.storage
            {
                launch.profile.cpu = state.profile.cpu;
                launch.profile.memory_gb = state.profile.memory_gb;
                launch.profile.storage = state.profile.storage.clone();
                changed = true;
            }
        }
        if changed {
            self.save_cloud_prototype();
        }
    }
}

/// A preflight refusal must not leave a healthy cloud without its watches.
fn complete(
    root: &Path,
    result: cloud_runtime::Result<cloud_runtime::state::Deployment>,
    cancel: &cloud_runtime::Cancellation,
    emit: &dyn Fn(Event),
) -> Completion {
    let pending = deployment::pending_resize(root);
    let loaded = Store::lock(root).and_then(|store| store.load());
    let mut unavailable = loaded.is_err();
    let mut notice = None;
    let ready = match result {
        Ok(state) => Some(state),
        Err(error) => {
            let unchanged = (!cancel.is_cancelled() && matches!(pending, Ok(None)))
                .then(|| loaded.ok().flatten())
                .flatten()
                .filter(cloud_runtime::state::Deployment::worker_ready);
            if let Some(state) = unchanged {
                let message = format!("Resize refused: {error}");
                emit(Event::Output(message.clone()));
                emit(Event::ready(Box::new(state.clone())));
                notice = Some(message);
                Some(state)
            } else {
                // A failure on a busy record waits for its lock, so its read is the current one.
                unavailable = !super::report_failure(root, &error, emit);
                None
            }
        }
    };
    Completion {
        outcome: Outcome {
            pending: pending.ok().flatten(),
            unavailable,
            notice,
        },
        ready,
    }
}

#[cfg(test)]
mod tests;
