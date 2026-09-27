//! Waits for the server to run and the worker to pass its contract over SSH.
//! A new server first answers with the host's own sshd and then with the
//! container's, whose key can change once while the worker settles, so each
//! probe uses a scratch known-hosts file. The key is pinned for this server
//! only after the worker contract is validated.
use super::{Compute, Journal, worker};
use crate::cloud_runtime::{
    Error, Event, Result, Stage, WorkerContract,
    command::Runner,
    deployment::Request,
    progress::Progress,
    ssh::Connection,
    state::{Deployment, Store},
    timeline::{AWAITING_ENDPOINT, AWAITING_SERVICES},
};
use horizon_cloud::{CreateState, WorkerSpec, WorkerStatus};
use std::time::{Duration, Instant};

const TIMEOUT: &str = "Worker readiness timed out; the server remains allocated for inspection or explicit deletion";
const PROBE_INTERVAL: Duration = Duration::from_secs(2);

pub(in crate::cloud_runtime::deployment) fn wait(
    request: &Request,
    compute: &Compute,
    store: &Store,
    runner: &Runner<'_>,
    state: &mut Deployment,
    spec: &WorkerSpec,
) -> Result<(Connection, WorkerContract)> {
    let deadline = Instant::now() + Duration::from_secs(u64::from(state.profile.bootstrap.readiness_seconds));
    state.stage = Stage::Readiness;
    store.save(state)?;
    (runner.emit)(Event::stage(state.stage));
    let CreateState::Bound { worker_id } = &state.operation else {
        return Err(Error::Invalid("Missing worker identity"));
    };
    let id: u64 = worker_id.parse().map_err(|_| Error::Invalid("Invalid worker ID"))?;
    let journal = Journal::load(store.root())?;
    let CreateState::Bound { worker_id: volume_id } = &journal.volume else {
        return Err(Error::Invalid("Missing workspace volume identity"));
    };
    let volume_id: u64 = volume_id.parse().map_err(|_| Error::Invalid("Invalid volume ID"))?;
    let volume = compute
        .client
        .inspect_volume_within(volume_id, runner.cancel, Some(remaining(deadline, runner)?))?
        .ok_or(horizon_cloud::CloudError::WorkerLost)?;
    volume.verify(&state.cloud_id)?;
    loop {
        let server = compute
            .client
            .inspect_server_within(id, runner.cancel, Some(remaining(deadline, runner)?))?
            .ok_or(horizon_cloud::CloudError::WorkerLost)?;
        remaining(deadline, runner)?;
        server.verify(&state.cloud_id)?;
        let described = worker(&server, spec, &volume)?;
        // The provider's answer, not the offer chosen earlier, must meet the profile.
        described.verify(spec)?;
        described.verify_resources(spec)?;
        if state.worker.as_ref().map(|known| serde_json::to_value(known).ok())
            != Some(serde_json::to_value(&described).ok())
        {
            state.worker = Some(described.clone());
            store.save(state)?;
        }
        if server.status() == WorkerStatus::Running {
            (runner.emit)(Event::Progress(Progress::activity(AWAITING_SERVICES)));
            if let Some(ready) = probe(request, store, runner, &described, deadline)? {
                return Ok(ready);
            }
        } else {
            (runner.emit)(Event::Progress(Progress::activity(AWAITING_ENDPOINT)));
        }
        std::thread::sleep(PROBE_INTERVAL.min(remaining(deadline, runner)?));
    }
}

fn probe(
    request: &Request,
    store: &Store,
    runner: &Runner<'_>,
    worker: &horizon_cloud::Worker,
    deadline: Instant,
) -> Result<Option<(Connection, WorkerContract)>> {
    let mut connection = Connection::new(worker, &request.settings, store.root())?;
    let pinned = connection.known_hosts.clone();
    let scratch = !pinned.exists();
    if scratch {
        let probe = store.root().join(format!("known-hosts-{}.probe", worker.id));
        match std::fs::remove_file(&probe) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        connection.known_hosts = probe;
    }
    let Ok(contract) = connection.ready(runner, &request.profile.capabilities, remaining(deadline, runner)?) else {
        return Ok(None);
    };
    if scratch {
        std::fs::rename(&connection.known_hosts, &pinned)?;
        connection.known_hosts = pinned;
    }
    Ok(Some((connection, contract)))
}

fn remaining(deadline: Instant, runner: &Runner<'_>) -> Result<Duration> {
    runner.cancel.check()?;
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or(Error::Invalid(TIMEOUT))
}
