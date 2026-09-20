use super::{Confirmation, Event, HorizonApp, Settings, Stage, Store, channel, cloud_runtime, deployment};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Deploy,
    Stop,
    Resume,
    Delete,
    Desktop,
    Remove,
}

#[cfg(test)]
mod tests;

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
        if runtime.receiver.is_some() && runtime.stage != Some(Stage::Ready) {
            return;
        }
        let settings = match Settings::load(&root.join("settings.json")) {
            Ok(settings) => settings,
            Err(error) => {
                runtime.error = Some(error.to_string());
                return;
            }
        };
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
            let root = root.join(&launch.id);
            let result = match action {
                Action::Stop => cloud_runtime::lifecycle::stop(&root, &settings, &cancel)
                    .map(|state| Event::Stopped(Box::new(state))),
                Action::Resume => cloud_runtime::lifecycle::resume(&root, &settings, &cancel).map(|()| Event::Resumed),
                Action::Delete => deployment::terminate(&root, &settings, &cancel).map(|()| Event::Deleted),
                Action::Deploy | Action::Desktop | Action::Remove => return,
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
        let store = match Store::lock(&root.join(&launch.id)) {
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
