use super::{Confirmation, Event, HorizonApp, Runtime, Settings, Stage, Store, channel, cloud_runtime, deployment};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Deploy,
    Stop,
    Resume,
    Delete,
    Desktop,
    Remove,
    RevokeBrowserstack,
    Reconcile,
    Rebuild,
    ContinueRebuild,
    CancelRebuild,
}

#[cfg(test)]
mod tests;

impl Runtime {
    fn start_reconciliation(&mut self, state_root: std::path::PathBuf, settings: Settings, ctx: &egui::Context) {
        if self.receiver.is_some() {
            return;
        }
        let (tx, rx) = channel();
        self.error = None;
        self.recovery_receiver = Some(rx);
        let worker_id = self.recovery_worker_id.trim().to_owned();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = cloud_runtime::lifecycle::reconcile(
                &state_root,
                &settings,
                (!worker_id.is_empty()).then_some(worker_id.as_str()),
                &cloud_runtime::Cancellation::default(),
            );
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    pub(super) fn poll_recovery(&mut self) {
        let Some(receiver) = &self.recovery_receiver else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(cloud_runtime::Error::Invalid(
                "Provider reconciliation ended without a result; the operation remains fenced",
            )),
        };
        self.recovery_receiver = None;
        match result {
            Ok(recovered) => {
                // A stopped worker, including one that stopped itself when idle, only needs Resume.
                let stopped = recovered.state.stage == Stage::Stopped;
                self.stage = Some(recovered.state.stage);
                self.state = Some(recovered.state);
                if self
                    .state
                    .as_ref()
                    .is_some_and(|state| matches!(state.operation, cloud_runtime::CreateState::Bound { .. }))
                {
                    self.recovery_worker_id.clear();
                }
                self.state_unavailable = false;
                self.error = (recovered.report.outcome.needs_attention() && !stopped)
                    .then(|| recovered.report.outcome.explanation().into());
                self.push_log(if stopped {
                    "The provider confirmed this worker is stopped. Resume starts the same worker again.".into()
                } else {
                    recovered.report.outcome.explanation().into()
                });
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    /// Another operation owns the cloud's worker or record.
    pub(super) fn busy(&self) -> bool {
        self.remote_release.is_some()
            || self.recovery_receiver.is_some()
            || (self.receiver.is_some() && self.stage != Some(Stage::Ready))
    }

    fn start_device_release(&mut self, state_root: std::path::PathBuf, settings: Settings, ctx: &egui::Context) {
        if !self.can_release_remote_devices() {
            return;
        }
        let (tx, rx) = channel();
        self.remote_release_error = None;
        self.remote_release = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = cloud_runtime::lifecycle::revoke_browserstack(
                &state_root,
                &settings,
                &cloud_runtime::Cancellation::default(),
            );
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    pub(super) fn can_release_remote_devices(&self) -> bool {
        self.remote_release.is_none()
            && (self.receiver.is_none() || self.stage == Some(Stage::Ready))
            && self.state.as_ref().is_some_and(|state| {
                state.requires_browserstack_release()
                    && matches!(state.operation, cloud_runtime::CreateState::Bound { .. })
                    // The release acts on the worker as recorded, so it waits for a pending replacement.
                    && state.image_replacement.is_none()
                    && state.stage != Stage::Replace
            })
    }

    pub(super) fn poll_remote_release(&mut self) {
        if self.state.as_ref().is_some_and(|state| state.browserstack_released) {
            self.remote_release_error = None;
        }
        let Some(receiver) = &self.remote_release else { return };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(cloud_runtime::Error::Invalid(
                "Remote device release ended without a result",
            )),
        };
        self.remote_release = None;
        match result {
            Ok(state) => {
                self.remote_release_error = None;
                self.state = Some(state);
                self.push_log("Remote devices released and copied credentials removed".into());
            }
            Err(error) => self.remote_release_error = Some(error.to_string()),
        }
    }
}

/// Resets what the card shows for a newly started worker operation.
fn begin_operation(runtime: &mut Runtime, action: Action) {
    if action == Action::Delete {
        // Until core reports its first step, the step it will start with stands
        // in, so Cancel shows only when that step can still be cancelled.
        runtime.progress.begin_deletion();
        // Both error channels the card shows belong to earlier attempts.
        runtime.error = None;
        runtime.remote_release_error = None;
        runtime.stage = Some(first_deletion_step(runtime.state.as_ref()));
    } else {
        // A failed deletion's frozen steps must not stand in for another operation.
        if runtime.progress.is_deletion() {
            runtime.progress.reset();
        }
        runtime.stage = Some(Stage::Provision);
    }
}

/// The deletion step core starts with for this record. Only hosted-device release
/// for a requested worker can still be cancelled; every other deletion starts with
/// a request that cannot be recalled.
fn first_deletion_step(state: Option<&cloud_runtime::state::Deployment>) -> Stage {
    match state {
        Some(state) if state.operation == cloud_runtime::CreateState::Prepared => Stage::DeleteStorage,
        Some(state) if state.requires_browserstack_release() => Stage::ReleaseDevices,
        _ => Stage::DeleteWorker,
    }
}

impl HorizonApp {
    pub(super) fn change_production_worker(&mut self, id: u32, action: Action, ctx: &egui::Context) {
        if let Some(kind) = super::rebuild::Kind::of(action) {
            self.start_production_rebuild(id, kind, ctx);
            return;
        }
        let Some(launch) = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.issue == id)
            .and_then(|group| group.remote.clone())
        else {
            return;
        };
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
        if runtime.busy() {
            return;
        }
        let settings = match Settings::load(&root.join("settings.json")) {
            Ok(settings) => settings,
            Err(error) => {
                runtime.error = Some(error.to_string());
                return;
            }
        };
        let state_root = match cloud_runtime::state::cloud_directory(&root, &launch.id) {
            Ok(path) => path,
            Err(error) => {
                runtime.error = Some(error.to_string());
                runtime.state_unavailable = true;
                return;
            }
        };
        if action == Action::Reconcile {
            runtime.start_reconciliation(state_root, settings, ctx);
            return;
        }
        if action == Action::RevokeBrowserstack {
            runtime.start_device_release(state_root, settings, ctx);
            return;
        }
        if let Some(cancel) = runtime.cancel.take() {
            cancel.cancel();
        }
        runtime.desktop = None;
        runtime.confirmation = Confirmation::None;
        runtime.rebuild = None;
        let (tx, rx) = channel();
        runtime.receiver = Some(rx);
        runtime.sender = Some(tx.clone());
        begin_operation(runtime, action);
        let cancel = cloud_runtime::Cancellation::default();
        runtime.cancel = Some(cancel.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let root = state_root;
            let emit = |event| {
                let _ = tx.send(event);
                ctx.request_repaint();
            };
            let result = match action {
                Action::Stop => cloud_runtime::lifecycle::stop(&root, &settings, &cancel)
                    .map(|state| Event::Stopped(Box::new(state))),
                Action::Resume => cloud_runtime::lifecycle::resume(&root, &settings, &cancel).map(|()| Event::Resumed),
                Action::RevokeBrowserstack => cloud_runtime::lifecycle::revoke_browserstack(&root, &settings, &cancel)
                    .map(|state| Event::Snapshot(Box::new(state))),
                Action::Delete => deployment::terminate(&root, &settings, &cancel, &emit).map(|()| Event::deleted()),
                Action::Deploy
                | Action::Desktop
                | Action::Remove
                | Action::Reconcile
                | Action::Rebuild
                | Action::ContinueRebuild
                | Action::CancelRebuild => return,
            };
            if let Ok(store) = Store::lock(&root)
                && let Ok(Some(state)) = store.load()
            {
                let _ = tx.send(Event::Snapshot(Box::new(state)));
            }
            let _ = tx.send(result.unwrap_or_else(|error| Event::failed(error.to_string())));
            ctx.request_repaint();
        });
    }

    pub(super) fn remove_deleted_cloud(&mut self, id: u32, ctx: &egui::Context) {
        let Some(index) = self.cloud_prototype.groups.0.iter().position(|group| group.issue == id) else {
            return;
        };
        if self
            .cloud_prototype
            .production
            .runtimes
            .get(&id)
            .is_some_and(|runtime| runtime.receiver.is_some())
        {
            return;
        }
        let Some(launch) = self.cloud_prototype.groups.0[index].remote.as_ref() else {
            return;
        };
        let Some(root) = &self.cloud_prototype.root else { return };
        let store = match cloud_runtime::state::cloud_directory(root, &launch.id).and_then(|path| Store::lock(&path)) {
            Ok(store) => store,
            Err(error) => {
                self.cloud_removal_error(id, error.to_string());
                return;
            }
        };
        let allowed = match store.load() {
            Ok(Some(state)) => match cloud_runtime::lifecycle::can_remove(&store, &state) {
                Ok(allowed) => allowed,
                Err(error) => {
                    self.cloud_removal_error(id, error.to_string());
                    return;
                }
            },
            Ok(None) => !launch.deployment_started,
            Err(error) => {
                self.cloud_removal_error(id, error.to_string());
                return;
            }
        };
        if !allowed {
            self.cloud_removal_error(
                id,
                "Delete the worker and workspace storage before removing this cloud".into(),
            );
            return;
        }
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
        self.save_cloud_prototype();
        self.release_removed_cloud_workspace(&group.workspace, ctx);
    }
    fn cloud_removal_error(&mut self, id: u32, message: String) {
        if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) {
            runtime.error = Some(message.clone());
        }
        self.cloud_prototype.error = Some(message);
    }
}
