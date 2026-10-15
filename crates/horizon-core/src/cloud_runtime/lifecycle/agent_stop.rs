//! An agent's stop of a cloud whose worker holds no credential that can stop it
//! (Hetzner, a Docker host). The worker records a checked request
//! (`horizon-worker-stopd`, started with `HORIZON_WORKER_STOP=horizon`); Horizon takes it
//! over SSH ([`read_stop_request`]) and stops the cloud as Stop does
//! ([`stop_for_agent`]), keeping the workspace volume. Only a running Horizon can do this.
use super::{Cancellation, CreateState, Error, Result, Settings, Stage, Store, mutation, stop_locked};
use crate::cloud_runtime::{command::Runner, ssh::Connection, state::Deployment};
use horizon_cloud::Worker;
use serde::Deserialize;
use std::{path::Path, time::Duration};

/// Prints the recorded request, or nothing on an image without the stop service.
const TAKE: &str = "if command -v horizon-worker-stopd >/dev/null 2>&1; then exec horizon-worker-stopd --take; fi";
const TIMEOUT: Duration = Duration::from_secs(30);

/// A stop an agent asked for, as the worker's stop log keeps it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
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
}

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

fn parse(output: &str, worker_id: &str) -> Result<Requested> {
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
    worker_id.clone_into(&mut stop.worker_id);
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
    let Some((worker_id, worker)) = Store::lock(root)?
        .load()?
        .as_ref()
        .and_then(watched)
        .map(|(id, worker)| (id.to_owned(), worker.clone()))
    else {
        return Ok(Requested::Nothing);
    };
    parse(&run(&worker, TAKE)?, &worker_id)
}

/// Stops the cloud at `root` as Stop does, for the agent's request `stop`, and records
/// the agent and its reason for the card. A stop that does not happen, because it fails
/// or the cloud is no longer the one the request came from, is taken back on the
/// worker, so its stop log keeps no stop that did not happen.
/// # Errors
/// Reports a busy or changed cloud and a failed stop.
pub fn stop_for_agent(root: &Path, settings: &Settings, cancel: &Cancellation, stop: &AgentStop) -> Result<Deployment> {
    stop_with(
        root,
        stop,
        |worker, command| run(worker, settings, root, cancel, command),
        |store| stop_locked(store, settings, cancel, mutation::IGNORE).map(drop),
    )
}

/// As `stop_for_agent`, with the worker command and the stop given.
pub(super) fn stop_with(
    root: &Path,
    request: &AgentStop,
    run: impl Fn(&Worker, &str) -> Result<String>,
    stop: impl FnOnce(&Store) -> Result<()>,
) -> Result<Deployment> {
    let store = Store::lock(root)?;
    let state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    // Another Horizon on this computer already stopped the cloud for this request.
    if state.stop_requested
        && state
            .last_self_stop
            .as_ref()
            .is_some_and(|stopped| stopped.at == request.at)
    {
        return Ok(state);
    }
    // Best effort: a worker the stop already reached cannot answer, and keeps the stop.
    let untake = |worker: &Worker| {
        let _ = run(worker, &format!("horizon-worker-stopd --untake {}", request.at));
    };
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
    if let Err(error) = stop(&store) {
        untake(&worker);
        return Err(error);
    }
    // The card names the agent and its reason, as after a stop the worker made itself.
    let mut state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    state.last_self_stop = Some(crate::cloud_runtime::SelfStop {
        at: request.at,
        reason: request.reason.clone(),
        agent: Some(request.agent.clone()),
        session: Some(request.session.clone()),
    });
    store.save(&state)?;
    Ok(state)
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
    let worker = shown
        .worker
        .as_ref()
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
