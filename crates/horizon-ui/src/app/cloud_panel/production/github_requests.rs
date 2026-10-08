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

/// The pending requests, and for a decision whether the worker refused it (`Some(None)`
/// when it applied the decision); a list-only poll leaves the last refusal as it is.
type Answer = (Vec<Request>, Option<Option<String>>);

/// A cloud's pending requests and the last decision the worker refused.
#[derive(Default)]
pub(super) struct State {
    pub(super) list: Vec<Request>,
    pub(super) refused: Option<String>,
    inflight: Option<Receiver<Answer>>,
    next: Option<Instant>,
}

impl State {
    /// Whether an exchange with the worker is under way. Decisions wait for it, so a
    /// second click never drops the outcome of the first.
    pub(super) fn busy(&self) -> bool {
        self.inflight.is_some()
    }

    fn receive(&mut self) {
        let Some(rx) = &self.inflight else { return };
        match rx.try_recv() {
            Ok((list, decided)) => {
                self.list = list;
                if let Some(refused) = decided {
                    self.refused = refused;
                }
                self.inflight = None;
            }
            Err(TryRecvError::Disconnected) => self.inflight = None,
            Err(TryRecvError::Empty) => {}
        }
    }
}

impl HorizonApp {
    /// The background exchanges with workers that each frame starts before it reads
    /// deployment events: the companion session and the GitHub access requests.
    pub(super) fn sync_cloud_worker_exchanges(&mut self, ctx: &egui::Context) {
        self.sync_cloud_companion_session(ctx);
        self.poll_github_requests(ctx);
    }

    /// Asks each ready, GitHub-connected cloud's worker for its pending requests.
    fn poll_github_requests(&mut self, ctx: &egui::Context) {
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
            // Only a service that takes requests is asked for them.
            let connected = matches!(runtime.github, Some(Prompt::Connected { requests: true, .. }));
            let ready = runtime.stage == Some(Stage::Ready);
            if !connected || !ready {
                runtime.github_requests.list.clear();
                runtime.github_requests.refused = None;
                continue;
            }
            let requests = &mut runtime.github_requests;
            if requests.inflight.is_some() {
                continue;
            }
            if let Some(next) = requests.next.filter(|next| now < *next) {
                // An idle card draws no frame by itself; wake it for the next poll.
                ctx.request_repaint_after(next - now);
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
        if runtime.github_requests.busy() {
            return;
        }
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
                Some((id, decision)) => Some(requests::decide(&connection, &runner, &id, decision)?),
                None => None,
            };
            Ok((requests::list(&connection, &runner)?, refused))
        })();
        if let Ok(answer) = answer {
            let _ = tx.send(answer);
        }
        // A failure drops the sender; the repaint lets the card see that and poll again.
        drop(tx);
        ctx.request_repaint();
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::State;

    #[test]
    fn an_exchange_holds_decisions_until_its_answer_arrives() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut state = State {
            inflight: Some(rx),
            ..State::default()
        };
        state.receive();
        assert!(state.busy(), "no answer yet");
        tx.send((Vec::new(), Some(Some("This request expired.".into()))))
            .unwrap();
        state.receive();
        assert!(!state.busy());
        assert_eq!(state.refused.as_deref(), Some("This request expired."));
    }
}
