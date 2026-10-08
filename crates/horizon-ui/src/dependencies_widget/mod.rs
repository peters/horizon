//! The Dependencies panel: the setup that connects GitHub and a worker, then the
//! portfolio of repositories the worker maintains. The panel only observes the worker
//! and edits its instructions; closing it leaves the worker running.

mod debug;
mod detail;
mod instructions;
mod portfolio;
mod setup;
mod summary;
mod table;
mod tone;
mod widgets;

use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, channel},
    time::{Duration, Instant},
};

use horizon_core::maintenance::{
    Endpoint, GitHubApp, Poller, connected_github_app,
    setup::{Setup, WorkerProbe},
};
use serde_json::Value;

pub(crate) use debug::Launch;

/// How often Cloud settings are read again to notice a new GitHub connection.
const GITHUB_RECHECK: Duration = Duration::from_secs(1);

/// Work the panel asks the app to do; drained after the panel draws.
pub(crate) enum Request {
    CloudSettings,
    WorkerTerminal { arguments: Vec<String>, cwd: PathBuf },
    LocalAgent(Launch),
}

pub(crate) struct DependenciesUiState {
    cloud_root: PathBuf,
    endpoint: Option<Result<Endpoint, String>>,
    poller: Option<Poller>,
    status: Value,
    transport_error: Option<String>,
    github: Option<GitHubApp>,
    github_checked: Option<Instant>,
    portfolio: portfolio::State,
    debug: debug::State,
    save: Option<Receiver<Result<(), String>>>,
    requests: Vec<Request>,
}

impl Default for DependenciesUiState {
    fn default() -> Self {
        Self {
            cloud_root: horizon_core::HorizonHome::resolve().root().join("cloud"),
            endpoint: Endpoint::fixture_from_env(),
            poller: None,
            status: Value::Null,
            transport_error: None,
            github: None,
            github_checked: None,
            portfolio: portfolio::State::default(),
            debug: debug::State::default(),
            save: None,
            requests: Vec::new(),
        }
    }
}

impl DependenciesUiState {
    pub(crate) fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        self.poll(ui.ctx());
        let setup = Setup::evaluate(self.github.is_some(), self.probe());
        egui::Frame::new().inner_margin(16).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            if setup.complete() {
                let action = portfolio::show(ui, &self.status, self.transport_error.as_deref(), &mut self.portfolio);
                if let Some(action) = action {
                    self.apply(action, ui.ctx());
                }
            } else if let Some(setup::Request::CloudSettings) = setup::show(ui, &setup, self.github.as_ref()) {
                self.requests.push(Request::CloudSettings);
            }
        });
        if let Some(launch) = self.debug.show(ui.ctx()) {
            self.requests.push(Request::LocalAgent(launch));
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if self.github_checked.is_none_or(|at| at.elapsed() >= GITHUB_RECHECK) {
            self.github = connected_github_app(&self.cloud_root);
            self.github_checked = Some(Instant::now());
        }
        if self.poller.is_none()
            && let Some(Ok(endpoint)) = &self.endpoint
        {
            let ctx = ctx.clone();
            self.poller = Some(Poller::start(endpoint.clone(), move || ctx.request_repaint()));
        }
        let updates: Vec<_> = self.poller.iter().flat_map(Poller::updates).collect();
        for update in updates {
            match update {
                Ok(status) => {
                    self.status = status;
                    self.status["ssh_connected"] = true.into();
                    self.transport_error = None;
                }
                Err(error) => {
                    if self.status.is_object() {
                        self.status["ssh_connected"] = false.into();
                    }
                    self.transport_error = Some(error);
                }
            }
        }
        if let Some(result) = self.save.as_ref().and_then(|receiver| receiver.try_recv().ok()) {
            self.save = None;
            self.portfolio.set_save_result(result);
        }
    }

    fn probe(&self) -> WorkerProbe<'_> {
        match &self.endpoint {
            None => WorkerProbe::Unavailable,
            Some(Err(error)) => WorkerProbe::Unreachable(error),
            Some(Ok(_)) if self.status.is_object() => WorkerProbe::Reporting(&self.status),
            Some(Ok(_)) => self
                .transport_error
                .as_deref()
                .map_or(WorkerProbe::Connecting, WorkerProbe::Unreachable),
        }
    }

    fn apply(&mut self, action: portfolio::Action, ctx: &egui::Context) {
        let Some(Ok(endpoint)) = &self.endpoint else {
            return;
        };
        match action {
            portfolio::Action::OpenTerminal => self.requests.push(Request::WorkerTerminal {
                arguments: endpoint.terminal_arguments(),
                cwd: endpoint.workdir().to_path_buf(),
            }),
            portfolio::Action::DebugLocalAgent => self.debug.open(endpoint.clone(), ctx),
            portfolio::Action::SaveInstructions { global, repositories } => {
                let (sender, receiver) = channel();
                self.save = Some(receiver);
                let worker = endpoint.clone();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let _ = sender.send(worker.configure(&global, &repositories));
                    ctx.request_repaint();
                });
            }
        }
    }
}
