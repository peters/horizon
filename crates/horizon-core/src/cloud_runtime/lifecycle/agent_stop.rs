//! An agent's stop of a cloud whose worker holds no credential that can stop it
//! (Hetzner, a Docker host). The worker records a checked request
//! (`horizon-worker-stopd`, started with `HORIZON_WORKER_STOP=horizon`); Horizon takes it
//! over SSH ([`read_stop_request`]) and stops the cloud as Stop does
//! ([`stop_for_agent`]), keeping the workspace volume. Only a running Horizon can do this.
use super::{Cancellation, CreateState, Error, Result, Settings, Stage, Store, mutation, stop_locked};
use crate::cloud_runtime::{command::Runner, ssh::Connection, state::Deployment};
use horizon_cloud::Worker;
use serde::Deserialize;
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// Prints the recorded request, or nothing on an image without the stop service.
const TAKE: &str = "if command -v horizon-worker-stopd >/dev/null 2>&1; then exec horizon-worker-stopd --take; fi";
const TIMEOUT: Duration = Duration::from_secs(30);

/// A stop an agent asked for, as the worker's stop log keeps it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStop {
    /// When the agent asked, in milliseconds since the Unix epoch.
    pub at: u64,
    pub reason: String,
    pub session: String,
    pub agent: String,
    /// The worker the request was taken from; a stop applies only to it.
    #[serde(skip)]
    pub worker_id: String,
    /// That worker as it was read, to give the request back to it without the cloud's
    /// record, which another operation may hold.
    #[serde(skip)]
    pub worker: Option<Box<Worker>>,
}

impl PartialEq for AgentStop {
    fn eq(&self, other: &Self) -> bool {
        (self.at, &self.reason, &self.session, &self.agent, &self.worker_id)
            == (other.at, &other.reason, &other.session, &other.agent, &other.worker_id)
    }
}

impl Eq for AgentStop {}

/// What one read of the worker's stop request found.
#[derive(Debug, PartialEq, Eq)]
pub enum Requested {
    /// The cloud is not a ready cloud whose worker records requests, or none is recorded.
    Nothing,
    /// A request waits while the worker works; the worker's words.
    Waiting(String),
    /// The worker dropped a request that no longer counts; the worker's words.
    Refused(String),
    /// The worker checked the request again and put it on its stop log: Horizon is to
    /// stop the cloud with [`stop_for_agent`].
    Stop(AgentStop),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Waiting {
    waiting: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Refused {
    refused: String,
}

fn parse(output: &str, worker: &Worker) -> Result<Requested> {
    let output = output.trim();
    if output.is_empty() || output == "null" {
        return Ok(Requested::Nothing);
    }
    if let Ok(Waiting { waiting }) = serde_json::from_str(output) {
        return Ok(Requested::Waiting(waiting));
    }
    if let Ok(Refused { refused }) = serde_json::from_str(output) {
        return Ok(Requested::Refused(refused));
    }
    let mut stop: AgentStop =
        serde_json::from_str(output).map_err(|_| Error::Invalid("The worker's stop request is malformed"))?;
    stop.worker_id.clone_from(&worker.id);
    stop.worker = Some(Box::new(worker.clone()));
    Ok(Requested::Stop(stop))
}

/// Whether Horizon takes stop requests from the cloud `state` describes: a ready
/// Hetzner or Docker host cloud on a bound worker, with no stop or image replacement
/// under way. Its worker, if so.
fn watched(state: &Deployment) -> Option<(&str, &Worker)> {
    let provider = state.profile.provider.as_str();
    if provider != horizon_cloud::hetzner::PROVIDER && provider != horizon_cloud::docker_host::PROVIDER {
        return None;
    }
    let CreateState::Bound { worker_id } = &state.operation else {
        return None;
    };
    (state.stage == Stage::Ready && !state.stop_requested && state.refuse_pending_replacement().is_ok())
        .then_some(())?;
    Some((worker_id.as_str(), state.worker.as_ref()?))
}

/// Whether Horizon takes stop requests from the cloud `state` describes.
#[must_use]
pub fn takes_stop_requests(state: &Deployment) -> bool {
    watched(state).is_some()
}

/// Reads, and takes, the agent's stop request of the cloud at `root`, without holding
/// the cloud's lock. A taken request is given again by each later read until the
/// worker stops, so an answer lost on the way loses no stop.
/// # Errors
/// Reports a busy cloud and an unreadable or malformed answer.
pub fn read_stop_request(root: &Path, settings: &Settings, cancel: &Cancellation) -> Result<Requested> {
    read_with(root, |worker, command| run(worker, settings, root, cancel, command))
}

/// As `read_stop_request`, with the worker command given.
pub(super) fn read_with(root: &Path, run: impl Fn(&Worker, &str) -> Result<String>) -> Result<Requested> {
    // A cloud removed meanwhile has no folder, and a read must not make one again.
    if !root.try_exists()? {
        return Ok(Requested::Nothing);
    }
    let Some(worker) = Store::lock_existing(root)?
        .load()?
        .as_ref()
        .and_then(watched)
        .map(|(_, worker)| worker.clone())
    else {
        return Ok(Requested::Nothing);
    };
    parse(&run(&worker, TAKE)?, &worker)
}

/// Stops the cloud at `root` as Stop does, for the agent's request `stop`, and records
/// the agent and its reason for the card. A stop that does not happen, because it fails
/// or the cloud is no longer the one the request came from, is taken back on the
/// worker, so its stop log keeps no stop that did not happen.
/// # Errors
/// Reports a busy or changed cloud and a failed stop.
pub fn stop_for_agent(root: &Path, settings: &Settings, cancel: &Cancellation, stop: &AgentStop) -> Result<Deployment> {
    // The provider work of the stop as its records announce it: a request that may have
    // reached the provider stays pending until it settles.
    let announced = std::cell::Cell::new(None);
    let pending = std::cell::Cell::new(false);
    let observe = |state| {
        pending.set(pending.get() || state == mutation::State::Pending);
        Ok(announced.replace(Some(state)).unwrap_or(mutation::State::Settled))
    };
    stop_with(
        root,
        stop,
        // Taking a request back must still run after the person cancelled the stop.
        |worker, command| run(worker, settings, root, &Cancellation::default(), command),
        |store| stop_locked(store, settings, cancel, &observe).map(drop),
        |error| uncertain_outcome(announced.get(), pending.get(), error),
        || cancel.is_cancelled(),
        LOCK_PATIENCE,
    )
}

/// How long a stop for an agent waits for another operation that holds the cloud, such
/// as another Horizon's stop for the same request, before it leaves the request to it.
const LOCK_PATIENCE: Duration = Duration::from_secs(30);

/// Whether a failed stop may still have taken effect: its last announced provider
/// request is pending, or it was cancelled after a request was sent, which the provider
/// may still carry out (a Docker host rolls such a stop back as refused).
fn uncertain_outcome(last: Option<mutation::State>, sent: bool, error: &Error) -> bool {
    last == Some(mutation::State::Pending)
        || (sent && matches!(error, Error::Provider(horizon_cloud::CloudError::Cancelled)))
}

/// As `stop_for_agent`, with the worker command and the stop given.
pub(super) fn stop_with(
    root: &Path,
    request: &AgentStop,
    run: impl Fn(&Worker, &str) -> Result<String>,
    stop: impl FnOnce(&Store) -> Result<()>,
    uncertain: impl Fn(&Error) -> bool,
    cancelled: impl Fn() -> bool,
    patience: Duration,
) -> Result<Deployment> {
    // Best effort: a worker the stop already reached cannot answer, and keeps the stop.
    let untake = |worker: &Worker| {
        let _ = run(worker, &format!("horizon-worker-stopd --untake {}", request.at));
    };
    // Another operation holds the cloud, which may be another Horizon stopping it for this
    // very request: it is waited for, since that stop checks the request again as soon as
    // it holds the cloud. One that holds it longer, or a cancel, gives the request back to
    // the worker it came from, so no stop that did not happen stays on its log.
    let waited = Instant::now();
    let store = loop {
        match Store::lock(root) {
            Err(Error::Busy) if waited.elapsed() < patience && !cancelled() => {
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(error) => {
                if let Some(worker) = &request.worker {
                    untake(worker);
                }
                return Err(if cancelled() {
                    Error::Provider(horizon_cloud::CloudError::Cancelled)
                } else {
                    error
                });
            }
            Ok(store) => break store,
        }
    };
    let state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    // Another Horizon on this computer already stopped the cloud for this request, or is
    // stopping it: its stop decides, and the request stays with it.
    if state.stop_requested
        && state
            .last_self_stop
            .as_ref()
            .is_some_and(|stopped| stopped.at == request.at)
    {
        return if state.stage == Stage::Stopped {
            Ok(state)
        } else {
            Err(Error::Invalid(
                "Another stop of this cloud for the agent's request has not finished; reconcile it",
            ))
        };
    }
    let requester = state.worker.as_ref().filter(|worker| {
        matches!(&state.operation, CreateState::Bound { worker_id } if *worker_id == request.worker_id)
            && worker.id == request.worker_id
    });
    let Some((_, worker)) = watched(&state).filter(|_| requester.is_some()) else {
        // The worker that took the request, if the cloud still has it, drops it.
        if let Some(worker) = requester {
            untake(worker);
        }
        return Err(Error::Invalid(
            "The cloud is no longer ready on the worker that asked, so Horizon did not stop it for the agent",
        ));
    };
    let worker = worker.clone();
    // The request still holds on the worker now, not only when it was read: one that
    // lapsed while this computer slept, or was replaced, stops nothing.
    match run(&worker, TAKE).and_then(|answer| parse(&answer, &worker)) {
        Ok(Requested::Stop(now)) if now.at == request.at => {}
        Ok(_) => {
            return Err(Error::Invalid(
                "The agent's stop request no longer holds on the worker, so Horizon did not stop it",
            ));
        }
        // Nothing was sent to the provider: the request goes back, if the worker answers.
        Err(error) => {
            untake(&worker);
            return Err(error);
        }
    }
    // The agent and its reason are recorded before the provider is asked, so a stop
    // whose outcome is uncertain keeps them for the card.
    let earlier = state.last_self_stop.clone();
    let mut attributed = state;
    attributed.last_self_stop = Some(crate::cloud_runtime::SelfStop {
        at: request.at,
        reason: request.reason.clone(),
        agent: Some(request.agent.clone()),
        session: Some(request.session.clone()),
    });
    if let Err(error) = store.save(&attributed) {
        untake(&worker);
        return Err(error);
    }
    if let Err(error) = stop(&store) {
        // Only a stop that certainly did not happen gives the request back and drops the
        // attribution; one whose provider request may have taken effect keeps both.
        if !uncertain(&error) {
            let restored = store.load().and_then(|now| {
                now.map_or(Ok(()), |mut now| {
                    now.last_self_stop = earlier;
                    store.save(&now)
                })
            });
            untake(&worker);
            // The stop's own failure is what the card reports; a record that could not be
            // restored keeps its attribution until the next stop replaces it.
            drop(restored);
        }
        return Err(error);
    }
    store.load()?.ok_or(Error::Invalid("No cloud deployment"))
}

/// Gives the request `stop` back to the worker that took it, as `shown`, the card's copy
/// of the record of the cloud at `root`, names it, when Horizon will not carry it out
/// now, as while another operation holds the cloud: the worker drops it, and the agent
/// may ask again. The saved record is not read, so an operation that holds it does not
/// keep the request taken.
/// # Errors
/// Refuses a record without that worker; reports a worker that cannot be reached.
pub fn give_back_stop_request(
    root: &Path,
    shown: &Deployment,
    settings: &Settings,
    cancel: &Cancellation,
    stop: &AgentStop,
) -> Result<()> {
    let worker = stop
        .worker
        .as_deref()
        .or(shown.worker.as_ref())
        .filter(|worker| worker.id == stop.worker_id)
        .ok_or(Error::Invalid("The stop request came from another worker"))?;
    run(
        worker,
        settings,
        root,
        cancel,
        &format!("horizon-worker-stopd --untake {}", stop.at),
    )
    .map(drop)
}

/// Runs `command` on `worker` over its pinned connection.
fn run(worker: &Worker, settings: &Settings, root: &Path, cancel: &Cancellation, command: &str) -> Result<String> {
    let connection = Connection::new(worker, settings, root)?;
    Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    }
    .run(
        "Reading the agent's stop request",
        &mut connection.pinned_command(command),
        TIMEOUT,
    )
}

#[cfg(all(test, unix))]
mod tests;
