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
                self.stage = Some(recovered.state.stage);
                self.state = Some(recovered.state);
                self.state_unavailable = false;
                self.error = None;
                self.logs.push_back(recovered.report.outcome.explanation().into());
                while self.logs.len() > 150 {
                    self.logs.pop_front();
                }
                if matches!(
                    self.state.as_ref().map(|state| &state.operation),
                    Some(cloud_runtime::CreateState::Requested)
                ) {
                    self.error = Some(recovered.report.outcome.explanation().into());
                }
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub(super) fn can_release_remote_devices(&self) -> bool {
        self.remote_release.is_none()
            && (self.receiver.is_none() || self.stage == Some(Stage::Ready))
            && self.state.as_ref().is_some_and(|state| {
                state.requires_browserstack_release()
                    && matches!(state.operation, cloud_runtime::CreateState::Bound { .. })
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
                self.logs
                    .push_back("Remote devices released and copied credentials removed".into());
                while self.logs.len() > 150 {
                    self.logs.pop_front();
                }
            }
            Err(error) => self.remote_release_error = Some(error.to_string()),
        }
    }
}

impl HorizonApp {
    pub(super) fn change_production_worker(&mut self, id: u32, action: Action, ctx: &egui::Context) {
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
        if runtime.remote_release.is_some()
            || runtime.recovery_receiver.is_some()
            || (runtime.receiver.is_some() && runtime.stage != Some(Stage::Ready))
        {
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
            if !runtime.can_release_remote_devices() {
                return;
            }
            let (tx, rx) = channel();
            runtime.remote_release_error = None;
            runtime.remote_release = Some(rx);
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
            return;
        }
        if let Some(cancel) = runtime.cancel.take() {
            cancel.cancel();
        }
        runtime.desktop = None;
        runtime.confirmation = Confirmation::None;
        let (tx, rx) = channel();
        runtime.receiver = Some(rx);
        runtime.sender = Some(tx.clone());
        runtime.stage = Some(Stage::Provision);
        let cancel = cloud_runtime::Cancellation::default();
        runtime.cancel = Some(cancel.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let root = state_root;
            let result = match action {
                Action::Stop => cloud_runtime::lifecycle::stop(&root, &settings, &cancel)
                    .map(|state| Event::Stopped(Box::new(state))),
                Action::Resume => cloud_runtime::lifecycle::resume(&root, &settings, &cancel).map(|()| Event::Resumed),
                Action::RevokeBrowserstack => cloud_runtime::lifecycle::revoke_browserstack(&root, &settings, &cancel)
                    .map(|state| Event::Snapshot(Box::new(state))),
                Action::Delete => deployment::terminate(&root, &settings, &cancel).map(|()| Event::Deleted),
                Action::Deploy | Action::Desktop | Action::Remove | Action::Reconcile => return,
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
                self.cloud_prototype.error = Some(error.to_string());
                return;
            }
        };
        let allowed = match store.load() {
            Ok(Some(state)) => matches!(
                state.operation,
                cloud_runtime::CreateState::Prepared | cloud_runtime::CreateState::Terminated { .. }
            ),
            Ok(None) => !launch.deployment_started,
            Err(error) => {
                self.cloud_prototype.error = Some(error.to_string());
                return;
            }
        };
        if !allowed {
            self.cloud_prototype.error = Some("Reconcile and delete the worker before removing this cloud".into());
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
    }
}
