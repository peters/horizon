//! The GitHub access requests of a ready cloud's agents: polled from the worker in
//! the background, decided on the cloud card, and sent back to the worker.
use super::{HorizonApp, Settings, Stage, cloud_runtime};
use horizon_core::cloud_runtime::github::{
    Prompt,
    requests::{self, Decision, Request},
};
use std::{
    sync::mpsc::{Receiver, TryRecvError, channel},
    time::{Duration, Instant},
};

/// How often a ready cloud's worker is asked for new requests.
const POLL: Duration = Duration::from_secs(15);

type Answer = (Vec<Request>, Option<String>);

/// A cloud's pending requests and the last decision the worker refused.
#[derive(Default)]
pub(super) struct State {
    pub(super) list: Vec<Request>,
    pub(super) refused: Option<String>,
    inflight: Option<Receiver<Answer>>,
    next: Option<Instant>,
}

impl State {
    fn receive(&mut self) {
        let Some(rx) = &self.inflight else { return };
        match rx.try_recv() {
            Ok((list, refused)) => {
                self.list = list;
                self.refused = refused;
                self.inflight = None;
            }
            Err(TryRecvError::Disconnected) => self.inflight = None,
            Err(TryRecvError::Empty) => {}
        }
    }
}

impl HorizonApp {
    /// Asks each ready, GitHub-connected cloud's worker for its pending requests.
    pub(super) fn poll_github_requests(&mut self, ctx: &egui::Context) {
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        let now = Instant::now();
        for group in &self.cloud_prototype.groups.0 {
            let Some(launch) = &group.remote else { continue };
            let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&group.issue) else {
                continue;
            };
            runtime.github_requests.receive();
            let connected = matches!(runtime.github, Some(Prompt::Connected { .. }));
            let ready = runtime.stage == Some(Stage::Ready);
            if !connected || !ready {
                runtime.github_requests.list.clear();
                continue;
            }
            let requests = &mut runtime.github_requests;
            if requests.inflight.is_some() || requests.next.is_some_and(|next| now < next) {
                continue;
            }
            let Some(state) = runtime.state.clone().filter(|state| state.worker.is_some()) else {
                continue;
            };
            requests.next = Some(now + POLL);
            requests.inflight = Some(spawn(root.clone(), launch.id.clone(), state, None, ctx.clone()));
        }
    }

    /// Sends the person's decision on a request, then lists the requests again.
    pub(super) fn decide_github_request(&mut self, id: u32, request: String, decision: Decision, ctx: &egui::Context) {
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
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
        let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) else {
            return;
        };
        let Some(state) = runtime.state.clone().filter(|state| state.worker.is_some()) else {
            return;
        };
        // The decided request leaves the card at once; the next list confirms it.
        runtime.github_requests.list.retain(|pending| pending.id != request);
        runtime.github_requests.inflight = Some(spawn(root, launch.id, state, Some((request, decision)), ctx.clone()));
    }
}

fn spawn(
    root: std::path::PathBuf,
    cloud_id: String,
    state: cloud_runtime::state::Deployment,
    decision: Option<(String, Decision)>,
    ctx: egui::Context,
) -> Receiver<Answer> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let answer = (|| -> cloud_runtime::Result<Answer> {
            let settings = Settings::load(&root.join("settings.json"))?;
            let state_root = cloud_runtime::state::cloud_directory(&root, &cloud_id)?;
            let worker = state
                .worker
                .as_ref()
                .ok_or(cloud_runtime::Error::Invalid("Cloud has no worker"))?;
            let connection = cloud_runtime::ssh::Connection::new(worker, &settings, &state_root)?;
            let cancel = cloud_runtime::Cancellation::default();
            let runner = cloud_runtime::command::Runner {
                cancel: &cancel,
                emit: &|_| {},
                secrets: Vec::new(),
            };
            let refused = match decision {
                Some((id, decision)) => requests::decide(&connection, &runner, &id, decision)?,
                None => None,
            };
            Ok((requests::list(&connection, &runner)?, refused))
        })();
        if let Ok(answer) = answer {
            let _ = tx.send(answer);
            ctx.request_repaint();
        }
    });
    rx
}
