//! Keeps each ready Upkeep cloud in step with its worker. Every 30 seconds, and every 8
//! while a run is active, one background exchange allows the loop's GitHub access requests
//! for the chosen repositories, installs or repairs the loop, and writes the board's status
//! document. Once the loop has survived its start, the live panel attaches to it.
//!
//! Everything the worker answers is untrusted: the agent account writes it. Only a request
//! from the loop's own session, for a repository the person chose, is allowed without the
//! person, and only for that session.
use std::path::{Path, PathBuf};
use std::sync::mpsc::{TryRecvError, channel};
use std::time::{Duration, Instant};

use horizon_core::cloud_runtime::{
    self, Cancellation,
    command::Runner,
    github::requests::{self, Decision, Request},
    settings::Settings,
    ssh::Connection,
};
use horizon_core::upkeep::worker::{self, Config, Signals};
use horizon_core::{PanelKind, PanelOptions};

use super::super::super::{HorizonApp, Stage};
use crate::app::upkeep::{Answer, Exchanged, Saved};

/// How often a ready Upkeep cloud's worker is asked for its state.
const POLL: Duration = Duration::from_secs(30);
/// How often while a run is active: the agent's `github_access` call waits about a minute.
const POLL_RUNNING: Duration = Duration::from_secs(8);
/// Failed reads of the saved Upkeeps in a row before one leaves out what it cannot read.
const READ_ATTEMPTS: u32 = 3;
/// How many allowed requests are remembered to keep them off the cloud card.
const ALLOWED_KEPT: usize = 64;

impl HorizonApp {
    pub(super) fn sync_upkeeps(&mut self, ctx: &egui::Context) {
        if self.cloud_prototype.root.is_none() {
            return;
        }
        let Some(saved) = self.saved_upkeeps(ctx) else { return };
        let now = Instant::now();
        for (cloud_id, config) in saved.iter() {
            let Some(index) = self
                .cloud_prototype
                .groups
                .0
                .iter()
                .position(|group| group.remote.as_ref().is_some_and(|launch| launch.id == *cloud_id))
            else {
                continue;
            };
            let sync = match self.upkeep.syncs.get_mut(cloud_id) {
                Some(sync) => sync,
                None => self.upkeep.syncs.entry(cloud_id.clone()).or_default(),
            };
            let mut answered = false;
            if let Some(rx) = &sync.inflight {
                match rx.try_recv() {
                    Ok((allowed, answer)) => {
                        sync.inflight = None;
                        answered = true;
                        sync.allowed.extend(allowed);
                        let excess = sync.allowed.len().saturating_sub(ALLOWED_KEPT);
                        sync.allowed.drain(..excess);
                        match answer {
                            Ok(answer) => {
                                sync.marker_left = answer.marker_left;
                                sync.last = answer;
                            }
                            Err(error) => {
                                tracing::warn!(cloud = %cloud_id, %error, "the Upkeep exchange failed");
                                requeue(&mut sync.signals, sync.sent);
                                sync.last = Answer::default();
                            }
                        }
                        // A run that just started is polled at its own pace from now on.
                        if sync.last.running {
                            let soon = now + POLL_RUNNING;
                            sync.next = Some(sync.next.map_or(soon, |next| next.min(soon)));
                        }
                    }
                    Err(TryRecvError::Disconnected) => {
                        sync.inflight = None;
                        requeue(&mut sync.signals, sync.sent);
                    }
                    Err(TryRecvError::Empty) => continue,
                }
            }
            let group = &self.cloud_prototype.groups.0[index];
            let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&group.issue) else {
                continue;
            };
            // The card never offers a request that this Upkeep allowed, also when the card's
            // own poll listed it just before the exchange allowed it. The id comes from the
            // worker, so a request must also be one Upkeep would allow to be hidden.
            if !sync.allowed.is_empty() {
                let session = worker::session(cloud_id);
                runtime
                    .github_requests
                    .list
                    .retain(|request| !(sync.allowed.contains(&request.id) && allowed(request, &session, config)));
            }
            let Some(state) = runtime
                .state
                .as_ref()
                .filter(|state| runtime.stage == Some(Stage::Ready) && state.worker.is_some())
            else {
                continue;
            };
            // Tried once per exchange, and not while a preparation holds the cloud record, so
            // a refused attach is tried again with a later exchange.
            let show = answered && sync.last.settled && !sync.agent_shown && runtime.preparation.is_none();
            if let Some(next) = sync.next.filter(|next| now < *next) {
                ctx.request_repaint_after(next - now);
            } else if let (Some(revision), Some(root)) = (
                group.remote.as_ref().map(|launch| launch.revision.clone()),
                self.cloud_prototype.root.clone(),
            ) {
                sync.next = Some(now + if sync.last.running { POLL_RUNNING } else { POLL });
                let signals = std::mem::take(&mut sync.signals).with_run_token();
                sync.sent = signals;
                let (tx, rx) = channel();
                let repaint = ctx.clone();
                let (cloud_id, state) = (cloud_id.clone(), state.clone());
                let marker_left = sync.marker_left;
                std::thread::spawn(move || {
                    let exchanged = exchange(&root, &cloud_id, &revision, &state, signals, marker_left);
                    let _ = tx.send(exchanged);
                    repaint.request_repaint();
                });
                sync.inflight = Some(rx);
            }
            if show {
                let shown = self.show_upkeep_agent(index, cloud_id);
                if let Some(sync) = self.upkeep.syncs.get_mut(cloud_id) {
                    sync.agent_shown = shown;
                }
            }
        }
    }

    /// The saved Upkeeps, once read. Reading starts on a worker thread the first time and
    /// again after a start saved a new one.
    fn saved_upkeeps(&mut self, ctx: &egui::Context) -> Option<Saved> {
        if let Some(saved) = &self.upkeep.saved {
            return Some(saved.clone());
        }
        let now = Instant::now();
        if let Some(after) = self.upkeep.read_after.filter(|after| now < *after) {
            ctx.request_repaint_after(after - now);
            return None;
        }
        // A few failed reads in a row: the next one leaves out what it cannot read.
        let strict = self.upkeep.read_failures < READ_ATTEMPTS;
        let rx = self.upkeep.reading.get_or_insert_with(|| {
            let (tx, rx) = channel();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let read = (|| -> std::io::Result<crate::app::upkeep::Read> {
                    let saved = worker::read_saved(strict)?;
                    let mut first_runs = Vec::new();
                    for (cloud, _) in &saved {
                        match worker::first_run_pending(cloud) {
                            Ok(Some(token)) => first_runs.push((cloud.clone(), token)),
                            // A marker that cannot be read now may hold a first run.
                            Err(error) if strict => return Err(error),
                            Ok(None) | Err(_) => {}
                        }
                    }
                    Ok((Saved::new(saved), first_runs))
                })()
                .map_err(|error| error.to_string());
                let _ = tx.send(read);
                repaint.request_repaint();
            });
            rx
        });
        match rx.try_recv() {
            // A list that could not be read whole is not kept: it would hide an Upkeep until
            // Horizon restarts. It is read again a little later.
            Ok(Err(error)) => {
                tracing::warn!(%error, "could not read the saved Upkeeps");
                self.upkeep.read_failures += 1;
                self.upkeep.reading = None;
                self.upkeep.read_after = Some(now + POLL);
                ctx.request_repaint_after(POLL);
                None
            }
            Ok(Ok((saved, first_runs))) => {
                self.upkeep.reading = None;
                self.upkeep.read_failures = 0;
                self.upkeep.saved = Some(saved.clone());
                // Only for a cloud this run of Horizon has not exchanged with yet: a marker
                // that is still there may belong to an exchange under way.
                for (cloud, token) in first_runs {
                    if let std::collections::hash_map::Entry::Vacant(entry) = self.upkeep.syncs.entry(cloud) {
                        let signals = &mut entry.insert(crate::app::upkeep::Sync::default()).signals;
                        signals.run_now = true;
                        signals.run_token = Some(token);
                    }
                }
                self.link_upkeep_boards(&saved);
                Some(saved)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.upkeep.reading = None;
                self.upkeep.read_after = Some(now + POLL);
                ctx.request_repaint_after(POLL);
                None
            }
        }
    }

    /// Opens the live view of the loop: a Codex panel that attaches to its session. A panel
    /// that is already open, or restored, is kept. Returns whether the panel is there.
    fn show_upkeep_agent(&mut self, index: usize, cloud_id: &str) -> bool {
        let session = worker::session(cloud_id);
        if self.board.panels.iter().any(|panel| panel.local_id == session) {
            return true;
        }
        let Some(workspace) = self
            .board
            .workspace_id_by_local_id(&self.cloud_prototype.groups.0[index].workspace)
        else {
            return false;
        };
        let mut options = PanelOptions {
            name: Some("Upkeep agent".into()),
            name_is_custom: Some(true),
            kind: PanelKind::Codex,
            local_id: Some(session),
            ..PanelOptions::default()
        };
        match self.prepare_cloud_remote_panel(index, &mut options) {
            Ok(()) => match self.create_cloud_member(index, options, workspace) {
                Ok(_) => true,
                Err(error) => {
                    tracing::warn!(%error, "could not open the live panel of an Upkeep");
                    false
                }
            },
            Err(error) => {
                tracing::warn!(%error, "could not prepare the live panel of an Upkeep");
                false
            }
        }
    }
}

/// One exchange: allows the loop's own GitHub access requests first, since its tool call
/// waits for them only about a minute, then installs or repairs the loop, applies the
/// board's signals, and writes the board's status document.
///
/// It holds this Upkeep's [`worker::settings_lock`] and uses the choice saved now, not the one read
/// before: a repository removed meanwhile never reaches the worker again, as a list or as an
/// allowed request. A save waits for the exchange, and the next exchange carries it.
fn exchange(
    root: &Path,
    cloud_id: &str,
    revision: &str,
    state: &cloud_runtime::state::Deployment,
    signals: Signals,
    marker_left: bool,
) -> Exchanged {
    let connection = (|| -> Result<Connection, String> {
        let settings = Settings::load(&root.join("settings.json")).map_err(|error| error.to_string())?;
        let state_root = cloud_runtime::state::cloud_directory(root, cloud_id).map_err(|error| error.to_string())?;
        let worker_state = state.worker.as_ref().ok_or("The cloud has no worker")?;
        Connection::new(worker_state, &settings, &state_root).map_err(|error| error.to_string())
    })();
    let connection = match connection {
        Ok(connection) => connection,
        Err(error) => return (Vec::new(), Err(error)),
    };
    let _lock = match worker::settings_lock(cloud_id) {
        Ok(lock) => lock,
        Err(error) => return (Vec::new(), Err(error.to_string())),
    };
    let config = match worker::load(cloud_id) {
        Ok(config) => config,
        Err(error) => return (Vec::new(), Err(error.to_string())),
    };
    // A press that gave the first run a new token moves its marker along, so a restart
    // sends again the token that the worker got, never an older one.
    if let Some(token) = signals.run_token.filter(|_| signals.run_now)
        && worker::first_run_pending(cloud_id)
            .ok()
            .flatten()
            .is_some_and(|marked| marked != token)
        && let Err(error) = worker::first_run_moved(cloud_id, token)
    {
        tracing::warn!(cloud = %cloud_id, %error, "could not move the Upkeep first-run marker");
    }
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let allowed = allow_chosen(&connection, &runner, cloud_id, &config);
    let answer = worker::exchange(&connection, &runner, cloud_id, revision, &config, signals)
        .map(|answer| learned(cloud_id, &config, signals, marker_left, &answer))
        .map_err(|error| error.to_string());
    (allowed, answer)
}

/// What Horizon keeps of an exchange that reached the worker: it forgets the first-run
/// marker once a Run now was delivered, and writes the board's status document. A marker is
/// never removed before its Run now reached the worker.
fn learned(cloud_id: &str, config: &Config, signals: Signals, marker_left: bool, answer: &worker::Exchange) -> Answer {
    // A marker left behind would ask for the first run again after a restart, with the same
    // token: each later exchange tries again to remove it, without sending Run now again.
    let marker_left = (signals.run_now || marker_left)
        && worker::first_run_sent(cloud_id)
            .inspect_err(
                |error| tracing::warn!(cloud = %cloud_id, %error, "could not remove the Upkeep first-run marker"),
            )
            .is_err();
    // The worker has the signals now: a status that cannot be written is only logged, so
    // they are not sent again. The next exchange writes it again.
    if let Err(error) = write_status(cloud_id, answer, config) {
        tracing::warn!(cloud = %cloud_id, %error, "could not write the Upkeep status");
    }
    Answer {
        loop_running: Some(answer.loop_running),
        settled: answer.loop_running && answer.settled,
        running: answer.running(),
        marker_left,
    }
}

/// Writes the board's status document of `cloud_id`.
fn write_status(cloud_id: &str, answer: &worker::Exchange, config: &Config) -> Result<(), String> {
    let folder: PathBuf = worker::directory(cloud_id);
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let text = serde_json::to_string_pretty(&answer.document(config)).map_err(|error| error.to_string())?;
    worker::write_atomically(&folder.join("status.json"), &text).map_err(|error| error.to_string())
}

/// Allows, for the loop's session only, its requests for repositories the person chose.
/// Every other request, and one the worker refused, stays on the cloud card for the person.
/// Returns the ids of the requests the worker applied.
fn allow_chosen(connection: &Connection, runner: &Runner<'_>, cloud_id: &str, config: &Config) -> Vec<String> {
    let Ok(pending) = requests::list(connection, runner) else {
        return Vec::new();
    };
    let session = worker::session(cloud_id);
    let mut applied = Vec::new();
    for request in pending.iter().filter(|request| allowed(request, &session, config)) {
        match requests::decide(connection, runner, &request.id, Decision::AllowTask) {
            Ok(None) => {
                tracing::info!(repository = %request.repository, "Upkeep allowed GitHub access");
                applied.push(request.id.clone());
            }
            Ok(Some(refused)) => {
                tracing::warn!(repository = %request.repository, %refused, "the worker refused Upkeep's GitHub access");
            }
            Err(error) => {
                tracing::warn!(repository = %request.repository, %error, "Upkeep's GitHub access decision did not reach the worker");
            }
        }
    }
    applied
}

/// Puts back the signals of a failed exchange: the worker may not have them. One the board
/// asked for meanwhile is newer and wins.
fn requeue(pending: &mut Signals, sent: Signals) {
    // The same token, so a worker that applied it already does not run twice; a Run now the
    // board asked for meanwhile is a new one and keeps its own.
    if sent.run_now && !pending.run_now {
        pending.run_token = sent.run_token;
    }
    pending.run_now |= sent.run_now;
    pending.paused = pending.paused.or(sent.paused);
}

/// Whether Upkeep allows `request` without the person: from the loop's own session, for a
/// repository of this Upkeep, and one the worker can allow for that session alone.
fn allowed(request: &Request, session: &str, config: &Config) -> bool {
    request.task
        && request.session == session
        && matches!(request.access.as_str(), "push" | "read")
        && config
            .repositories
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&request.repository))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(repository: &str, session: &str, task: bool) -> Request {
        Request {
            id: "a1".into(),
            repository: repository.into(),
            access: "push".into(),
            reason: "fix a pull request".into(),
            session: session.into(),
            agent: "codex".into(),
            task,
        }
    }

    #[test]
    fn a_failed_exchange_sends_its_signals_again_unless_newer_ones_came() {
        let mut pending = Signals::default();
        let sent = Signals {
            run_now: true,
            paused: Some(true),
            ..Signals::default()
        }
        .with_run_token();
        requeue(&mut pending, sent);
        assert_eq!(pending, sent, "the same Run now, with its token, goes again");
        assert!(pending.run_token.is_some());
        let mut newer = Signals {
            run_now: true,
            ..Signals::default()
        };
        requeue(&mut newer, sent);
        assert_eq!(newer.run_token, None, "a Run now asked for again gets a new token");
        let mut pending = Signals {
            run_now: false,
            paused: Some(false),
            ..Signals::default()
        };
        requeue(
            &mut pending,
            Signals {
                run_now: false,
                paused: Some(true),
                ..Signals::default()
            },
        );
        assert_eq!(pending.paused, Some(false), "the later choice of the board wins");
    }

    #[test]
    fn only_the_loops_own_requests_for_chosen_repositories_are_allowed() {
        let config = Config {
            repositories: vec!["Acme/Web".into()],
            ..Config::default()
        };
        let session = worker::session("c1");
        assert!(allowed(&request("acme/web", &session, true), &session, &config));
        assert!(
            !allowed(&request("acme/other", &session, true), &session, &config),
            "a repository the person did not choose"
        );
        assert!(
            !allowed(&request("acme/web", "codex-1", true), &session, &config),
            "another session of the cloud"
        );
        assert!(
            !allowed(&request("acme/web", &session, false), &session, &config),
            "a request the worker can only allow for the whole cloud"
        );
        let mut admin = request("acme/web", &session, true);
        admin.access = "admin".into();
        assert!(!allowed(&admin, &session, &config));
    }
}
