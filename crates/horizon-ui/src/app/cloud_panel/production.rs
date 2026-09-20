//! UI actions and progress for real deployments. Provider/build/session work lives in core.
mod capabilities;
mod cards;
#[cfg(all(test, unix))]
mod creation_tests;
mod lifecycle;
mod presentation;
mod progress;
mod sessions;
use super::HorizonApp;
use horizon_core::cloud_panel::CloudConfig;
use horizon_core::{
    PanelKind, PanelOptions,
    cloud_panel::{CloudGroup, CloudLaunch},
    cloud_runtime::{
        self, Event, Stage,
        deployment::{self, Request},
        settings::Settings,
        ssh::Connection,
        state::{Deployment, Session, Store},
    },
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::{Receiver, channel},
};

#[derive(Default)]
pub(super) struct Production {
    pub creating: bool,
    pub(super) focus_title_on_open: bool,
    title: String,
    repository: String,
    revision: String,
    profiles: Option<CloudConfig>,
    selected_profile: String,
    session_id: Option<String>,
    pub runtimes: HashMap<u32, Runtime>,
}
#[derive(Default, PartialEq, Eq)]
pub(super) enum Confirmation {
    #[default]
    None,
    Stop,
    Delete,
}
#[derive(Default)]
pub(super) struct Runtime {
    receiver: Option<Receiver<Event>>,
    sender: Option<std::sync::mpsc::Sender<Event>>,
    cancel: Option<horizon_core::cloud_runtime::Cancellation>,
    stage: Option<Stage>,
    progress: progress::Timeline,
    logs: std::collections::VecDeque<String>,
    state: Option<Deployment>,
    error: Option<String>,
    confirmation: Confirmation,
    state_unavailable: bool,
    needs_attach: bool,
    pending_browser_attachments: std::collections::HashSet<String>,
    needs_desktop: bool,
    pub(in crate::app::cloud_panel) desktop_controller: Option<String>,
    pub(in crate::app::cloud_panel) desktop_last_input: Option<String>,
    desktop: Option<std::sync::Arc<cloud_runtime::tunnel::DesktopTunnel>>,
    browsers: Vec<horizon_core::browser::CloudViewState>,
    browsers_discovered: bool,
}
impl HorizonApp {
    pub(super) fn prepare_production_clouds(&mut self, ctx: &egui::Context) {
        if self.pending_startup_runtime_state.is_some() || self.startup_receiver.is_some() {
            return;
        }
        self.restore_cloud_state(ctx);
        let mut finished = Vec::new();
        let mut removed = Vec::new();
        let mut resumed = Vec::new();
        for (&id, runtime) in &mut self.cloud_prototype.production.runtimes {
            let events: Vec<_> = runtime
                .receiver
                .as_ref()
                .map_or_else(Vec::new, |rx| rx.try_iter().collect());
            for event in events {
                match event {
                    Event::Snapshot(state) => {
                        runtime.stage = Some(state.stage);
                        runtime.state = Some(*state);
                    }
                    Event::Stopped(state) => {
                        runtime.progress.stage(Stage::Stopped, std::time::Instant::now());
                        runtime.stage = Some(Stage::Stopped);
                        runtime.state = Some(*state);
                        runtime.desktop = None;
                        runtime.error = None;
                        runtime.receiver = None;
                        runtime.cancel = None;
                    }
                    Event::Resumed => {
                        runtime.receiver = None;
                        resumed.push(id);
                    }
                    Event::ClosedBrowsers(ids) => {
                        runtime.browsers.retain(|b| !ids.contains(&b.id));
                        removed.extend(ids.into_iter().map(|local| (id, local)));
                    }
                    Event::DesktopControl { active, last } => {
                        runtime.desktop_controller = active;
                        runtime.desktop_last_input = last;
                    }
                    Event::Desktop(tunnel) => {
                        runtime.desktop = Some(tunnel);
                        runtime.needs_desktop = true;
                    }
                    Event::Browsers(browsers) => {
                        runtime.browsers = browsers;
                        runtime.browsers_discovered = true;
                    }
                    Event::Deleted => {
                        runtime.progress.stage(Stage::Deleted, std::time::Instant::now());
                        runtime.stage = Some(Stage::Deleted);
                        runtime.desktop = None;
                        runtime.error = Some("Worker deleted; its processes and files are no longer available".into());
                        finished.push(id);
                    }
                    Event::Stage(stage, at) => {
                        runtime.progress.stage(stage, at);
                        runtime.stage = Some(stage);
                    }
                    Event::Progress(progress) => runtime.progress.update(progress),
                    Event::Output(line) => {
                        runtime.logs.push_back(line);
                        while runtime.logs.len() > 150 {
                            runtime.logs.pop_front();
                        }
                    }
                    Event::Ready(state, at) => {
                        runtime.progress.stage(Stage::Ready, at);
                        runtime.browsers.clear();
                        runtime.browsers_discovered = false;
                        runtime.needs_attach = true;
                        runtime.state = Some(*state);
                        runtime.stage = Some(Stage::Ready);
                        runtime.error = None;
                        finished.push(id);
                    }
                    Event::Failed(error, at) => {
                        runtime.progress.finish(at);
                        runtime.error = Some(error);
                        finished.push(id);
                    }
                }
            }
            if runtime.receiver.is_some() {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        self.finish_failed_cloud_operations(finished);
        for id in resumed {
            self.start_production_deployment(id, ctx);
        }
        self.remove_closed_cloud_browsers(removed);
        self.sync_cloud_presentations();
        self.cloud_prototype.groups.reconcile(&mut self.board);
        self.board.cloud_groups = self.cloud_prototype.groups.clone();
        for group in &self.cloud_prototype.groups.0 {
            if let Some(ws) = self.board.workspace_id_by_local_id(&group.workspace) {
                self.board.retain_workspace_when_empty(ws);
            }
        }
    }
    fn finish_failed_cloud_operations(&mut self, finished: Vec<u32>) {
        for id in finished {
            if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id)
                && runtime.error.is_some()
            {
                runtime.receiver = None;
                runtime.cancel = None;
            }
        }
    }
    fn remove_closed_cloud_browsers(&mut self, removed: Vec<(u32, String)>) {
        for (cloud, local) in removed {
            let member = self
                .cloud_prototype
                .groups
                .0
                .iter()
                .any(|group| group.issue == cloud && group.panels.contains(&local));
            if member
                && let Some(id) = self.board.panel_id_by_local_id(&local)
                && self
                    .board
                    .panel(id)
                    .is_some_and(|panel| panel.kind == PanelKind::Browser)
            {
                self.board.close_panel(id);
                self.panel_render_caches.browser_ui_state.remove(&id);
            }
        }
    }
    fn restore_cloud_state(&mut self, ctx: &egui::Context) {
        let session = self.active_session.as_ref().map(|s| s.session_id.clone());
        if !self.cloud_prototype.initialized || self.cloud_prototype.production.session_id != session {
            self.cloud_prototype.initialized = true;
            self.cloud_prototype.production.session_id = session;
            self.cloud_prototype.root = Some(horizon_core::HorizonHome::resolve().root().join("cloud"));
            self.cloud_prototype.groups = self.board.cloud_groups.clone();
            self.cloud_prototype.production.runtimes.clear();
            self.cloud_prototype.ready = true;
            let reconnect: Vec<_> = self
                .cloud_prototype
                .groups
                .0
                .iter()
                .filter_map(|group| {
                    let launch = group.remote.as_ref()?;
                    let result = Store::lock(&self.cloud_prototype.root.as_ref()?.join(&launch.id))
                        .and_then(|store| store.load());
                    let state = match result {
                        Ok(Some(state)) => state,
                        Ok(None) if !launch.deployment_started => return None,
                        other => {
                            let message = other.err().map_or_else(
                                || "Deployment record is missing; reconcile its worker before continuing".into(),
                                |error| error.to_string(),
                            );
                            self.cloud_prototype.production.runtimes.insert(
                                group.issue,
                                Runtime {
                                    error: Some(message),
                                    state_unavailable: true,
                                    ..Runtime::default()
                                },
                            );
                            return None;
                        }
                    };
                    self.cloud_prototype.production.runtimes.insert(
                        group.issue,
                        Runtime {
                            stage: Some(state.stage),
                            state: Some(state.clone()),
                            ..Runtime::default()
                        },
                    );
                    (matches!(
                        state.operation,
                        horizon_core::cloud_runtime::CreateState::Bound { .. }
                            | horizon_core::cloud_runtime::CreateState::Requested
                    ) && !state.stop_requested)
                        .then_some(group.issue)
                })
                .collect();
            for id in reconnect {
                self.start_production_deployment(id, ctx);
            }
        }
    }
    pub(super) fn render_cloud_creation(&mut self, ctx: &egui::Context) {
        if !self.cloud_prototype.production.creating {
            return;
        }
        let mut create = false;
        let mut load = false;
        let response = egui::Modal::new(egui::Id::new("cloud-creation")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading("New cloud");
            let form = &mut self.cloud_prototype.production;
            ui.label("Cloud title");
            let title = ui.add(egui::TextEdit::singleline(&mut form.title).id(egui::Id::new("cloud-title")));
            if std::mem::take(&mut form.focus_title_on_open) {
                title.request_focus();
            }
            ui.label("Repository");
            if ui
                .add(egui::TextEdit::singleline(&mut form.repository).id(egui::Id::new("cloud-repository")))
                .changed()
            {
                form.profiles = None;
            }
            ui.label("Committed base revision");
            ui.add(egui::TextEdit::singleline(&mut form.revision).id(egui::Id::new("cloud-revision")));
            ui.small("Only committed files are transferred. Local changes stay on this computer.");
            load = ui.button("Read .horizon/cloud.yml").clicked();
            if let Some(config) = &form.profiles {
                ui.label("Profile");
                ui.horizontal_wrapped(|ui| {
                    for name in config.profiles.keys() {
                        ui.selectable_value(&mut form.selected_profile, name.clone(), name);
                    }
                });
            }
            ui.horizontal(|ui| {
                create = ui
                    .add_enabled(
                        !form.title.trim().is_empty() && form.profiles.is_some(),
                        egui::Button::new("Create cloud"),
                    )
                    .clicked();
                if ui.button("Cancel").clicked() {
                    form.creating = false;
                }
            });
            if let Some(error) = &self.cloud_prototype.error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
        });
        if response.should_close() {
            self.cloud_prototype.production.creating = false;
            return;
        }
        if load {
            let form = &mut self.cloud_prototype.production;
            let result = std::fs::read_to_string(PathBuf::from(&form.repository).join(".horizon/cloud.yml"))
                .map_err(|_| "Cannot read .horizon/cloud.yml".to_owned())
                .and_then(|yaml| CloudConfig::parse(&yaml).map_err(|e| e.to_string()));
            match result {
                Ok(config) => {
                    form.selected_profile.clone_from(&config.default);
                    form.profiles = Some(config);
                    self.cloud_prototype.error = None;
                }
                Err(e) => self.cloud_prototype.error = Some(e),
            }
        }
        if create && let Err(error) = self.create_production_cloud(ctx) {
            self.cloud_prototype.error = Some(error.to_string());
        }
    }
    fn create_production_cloud(&mut self, ctx: &egui::Context) -> cloud_runtime::Result<()> {
        let form = &self.cloud_prototype.production;
        let repo = PathBuf::from(&form.repository).canonicalize()?;
        let revision = cloud_runtime::repository::resolve(
            &repo,
            if form.revision.is_empty() {
                "HEAD"
            } else {
                &form.revision
            },
        )?;
        let profile = form
            .profiles
            .as_ref()
            .and_then(|config| config.profiles.get(&form.selected_profile))
            .cloned()
            .ok_or(cloud_runtime::Error::Invalid("Choose a repository profile"))?;
        let launch = CloudLaunch {
            deployment_started: false,
            id: horizon_core::cloud_runtime::new_id(),
            revision,
            profile_name: form.selected_profile.clone(),
            profile,
        };
        let title = form.title.trim().to_owned();
        let ws = self.board.ensure_workspace();
        let workspace = self
            .board
            .workspace(ws)
            .map(|w| w.local_id.clone())
            .ok_or(cloud_runtime::Error::Invalid("No workspace selected"))?;
        let id = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .map(|g| g.issue)
            .max()
            .unwrap_or(100)
            .checked_add(1)
            .ok_or(cloud_runtime::Error::Invalid("Too many clouds"))?;
        let position = [
            24.0,
            self.cloud_prototype
                .groups
                .0
                .iter()
                .filter(|g| g.workspace == workspace)
                .map(|g| g.overview_bounds().1[1])
                .fold(80.0, f32::max)
                + 48.0,
        ];
        let mut group = CloudGroup::new(id, title, workspace, repo, position);
        group.environment.id.clone_from(&launch.id);
        group.environment.connection = horizon_core::cloud_panel::CloudConnection::ManagedWorker;
        group.environment.provider = Some("runpod".into());
        group.environment.profile = Some(launch.profile_name.clone());
        group.environment.image.clone_from(&launch.profile.image);
        group.remote = Some(launch);
        self.cloud_prototype.groups.0.push(group);
        self.cloud_prototype.production.creating = false;
        self.cloud_prototype.error = None;
        if let Some(ws) = self.board.workspace_mut(ws) {
            ws.layout = None;
        }
        self.save_cloud_prototype();
        self.cloud_overview(ctx);
        Ok(())
    }
    fn start_production_deployment(&mut self, id: u32, ctx: &egui::Context) {
        let Some(group) = self.cloud_prototype.groups.0.iter().find(|g| g.issue == id) else {
            return;
        };
        let Some(launch) = group.remote.clone() else { return };
        let repository = group.cwd.clone();
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        let loaded = Store::lock(&root.join(&launch.id)).and_then(|store| store.load());
        let existing = match loaded {
            Ok(Some(state)) => matches!(
                state.operation,
                cloud_runtime::CreateState::Bound { .. } | cloud_runtime::CreateState::Requested
            ),
            Ok(None) if !launch.deployment_started => false,
            other => {
                let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
                runtime.state_unavailable = true;
                runtime.error = Some(other.err().map_or_else(
                    || "Deployment record is missing; reconcile its worker before continuing".into(),
                    |error| error.to_string(),
                ));
                return;
            }
        };
        let settings = match Settings::load(&root.join("settings.json")) {
            Ok(settings) => settings,
            Err(error) => {
                self.cloud_prototype.production.runtimes.entry(id).or_default().error =
                    Some(format!("{error}. Configure {}", root.join("settings.json").display()));
                return;
            }
        };
        let request = Request {
            cloud_id: launch.id.clone(),
            repository,
            revision: launch.revision,
            profile: launch.profile,
            state_root: root.join(&launch.id),
            settings,
        };
        if let Err(error) = deployment::prepare(&request) {
            self.cloud_prototype.error = Some(error.to_string());
            return;
        }
        if let Some(launch) = self
            .cloud_prototype
            .groups
            .0
            .iter_mut()
            .find(|g| g.issue == id)
            .and_then(|g| g.remote.as_mut())
        {
            launch.deployment_started = true;
        }
        self.save_cloud_prototype();
        if !existing
            && (!self.active_session.as_ref().is_some_and(|session| session.persistent)
                || !self.auto_save_runtime_state()
                || self
                    .active_session
                    .as_ref()
                    .is_none_or(|session| self.session_store.sync_runtime_state(&session.session_id).is_err()))
        {
            self.cloud_prototype.error =
                Some("Save this workspace in a persistent Horizon session before allocating a worker".into());
            return;
        }
        let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
        if runtime.receiver.is_some() && runtime.stage != Some(Stage::Ready) {
            return;
        }
        if let Some(cancel) = runtime.cancel.take() {
            cancel.cancel();
        }
        runtime.desktop = None;
        runtime.progress.reset();
        let (tx, rx) = channel();
        let cancel = cloud_runtime::Cancellation::default();
        runtime.cancel = Some(cancel.clone());
        runtime.receiver = Some(rx);
        runtime.sender = Some(tx.clone());
        runtime.error = None;
        runtime.state_unavailable = false;
        let ctx = ctx.clone();
        std::thread::spawn(move || run_deployment(&request, &cancel, &tx, &ctx));
    }

    pub(in crate::app) fn prepare_cloud_remote_panel(
        &mut self,
        index: usize,
        options: &mut PanelOptions,
    ) -> horizon_core::Result<()> {
        let group = &self.cloud_prototype.groups.0[index];
        let Some(launch) = &group.remote else { return Ok(()) };
        let runtime = self.cloud_prototype.production.runtimes.get(&group.issue);
        let state = runtime
            .and_then(|r| r.state.as_ref())
            .filter(|s| s.stage == Stage::Ready)
            .ok_or_else(|| horizon_core::Error::Config("Deploy or reconnect the cloud before adding panels".into()))?;
        if let Some(reason) = group.unavailable_panel_reason(options.kind) {
            return Err(horizon_core::Error::Config(reason.into()));
        }
        let root = self
            .cloud_prototype
            .root
            .as_ref()
            .ok_or_else(|| horizon_core::Error::Config("No cloud settings".into()))?;
        let result = (|| -> cloud_runtime::Result<()> {
            let settings = Settings::load(&root.join("settings.json"))?;
            let store = Store::lock(&root.join(&launch.id))?;
            let mut saved = store
                .load()?
                .ok_or(cloud_runtime::Error::Invalid("Missing cloud deployment"))?;
            let id = options
                .local_id
                .clone()
                .unwrap_or_else(horizon_core::cloud_runtime::new_id);
            let worker = state
                .worker
                .as_ref()
                .ok_or(cloud_runtime::Error::Invalid("Cloud has no worker"))?;
            let connection = Connection::new(worker, &settings, store.root())?;
            if options.kind == PanelKind::Browser {
                let observed = runtime
                    .and_then(|runtime| runtime.browsers.iter().find(|browser| browser.id == id))
                    .map(|browser| browser.backend);
                let backend = capabilities::browser_backend(
                    &launch.profile.capabilities,
                    observed,
                    options.browser_config.as_ref().map(|config| config.backend),
                    options.is_restore,
                )?;
                options.browser_config.get_or_insert_with(Default::default).backend = backend;
                options.cloud_connection = Some(connection);
                options.local_id = Some(id);
                options.cwd = None;
                return Ok(());
            }
            if options.kind == PanelKind::Device {
                let endpoint = runtime
                    .and_then(|r| r.desktop.as_ref())
                    .ok_or(cloud_runtime::Error::Invalid("Desktop tunnel is connecting"))?
                    .endpoint;
                options.command = Some(endpoint.to_string());
                options.local_id = Some(id);
                options.cwd = None;
                return Ok(());
            }
            let agent = match options.kind {
                PanelKind::Codex => "codex",
                PanelKind::Claude => "claude",
                PanelKind::Grok => "grok",
                _ => "shell",
            };
            let session = Session {
                panel_id: id.clone(),
                agent: agent.into(),
                tmux: id.clone(),
                branch: format!("agent/{id}"),
                worktree: format!("/workspace/agents/{id}"),
            };
            if !saved.sessions.iter().any(|s| s.panel_id == id) {
                saved.sessions.push(session.clone());
                store.save(&saved)?;
            }
            let worker = state
                .worker
                .as_ref()
                .ok_or(cloud_runtime::Error::Invalid("Cloud has no worker"))?;
            options.cloud_connection = Some(connection);
            options.command = Some("ssh".into());
            options.args = Connection::new(worker, &settings, store.root())?.attach_args(&session, &launch.revision)?;
            options.cwd = None;
            options.local_id = Some(id);
            options.session_binding = None;
            Ok(())
        })();
        result.map_err(|e| horizon_core::Error::Config(e.to_string()))
    }
}

fn run_deployment(
    request: &Request,
    cancel: &cloud_runtime::Cancellation,
    tx: &std::sync::mpsc::Sender<Event>,
    ctx: &egui::Context,
) {
    let emit = |event| {
        let _ = tx.send(event);
        ctx.request_repaint();
    };
    let settings = request.settings.clone();
    let root = request.state_root.clone();
    match deployment::deploy(request, cancel, &emit) {
        Ok(state) => presentation::watch(&state, &settings, &root, cancel, tx, ctx),
        Err(error) => {
            if let Ok(store) = Store::lock(&root)
                && let Ok(Some(state)) = store.load()
            {
                emit(Event::Snapshot(Box::new(state)));
            }
            emit(Event::failed(error.to_string()));
        }
    }
}
