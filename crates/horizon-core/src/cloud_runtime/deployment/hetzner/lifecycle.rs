//! Check, stop, resume and delete for Hetzner clouds. A powered-off Hetzner
//! server is still billed, so stopping releases it: the server is deleted and
//! the workspace volume kept. The deployment stays bound to the released server
//! and stopped; resuming clears that fence so the next reconnect creates a new
//! server in the volume's location, attaching the same volume.
use super::{Compute, Journal, worker};
use crate::cloud_runtime::{
    Error, Result, Stage,
    settings::Settings,
    state::{Deployment, Store},
};
use horizon_cloud::{
    Cancellation, CloudError, CreateState, Worker, WorkerStatus,
    runpod::recovery::{Outcome, Reconciliation},
};
use std::time::{Duration, Instant};

/// How long a graceful shutdown may take before power is cut.
const SHUTDOWN_GRACE: Duration = Duration::from_mins(1);

/// Checks the recorded server without creating, starting or deleting anything.
pub(in crate::cloud_runtime) fn reconcile(
    store: &Store,
    state: &mut Deployment,
    settings: &Settings,
    cancel: &Cancellation,
) -> Result<Reconciliation> {
    reconcile_with(&Compute::new(settings)?, store, state, cancel)
}

/// As `reconcile`, with the Hetzner client given.
pub(super) fn reconcile_with(
    compute: &Compute,
    store: &Store,
    state: &mut Deployment,
    cancel: &Cancellation,
) -> Result<Reconciliation> {
    let journal = Journal::load(store.root())?;
    let operation = state.cloud_id.clone();
    let mut report = Reconciliation {
        operation_id: operation.clone(),
        outcome: Outcome::Prepared,
        worker: None,
    };
    let bound = match state.operation.clone() {
        CreateState::Prepared => return Ok(report),
        CreateState::Terminated { worker_id } => {
            report.outcome = Outcome::Terminated { worker_id };
            return Ok(report);
        }
        CreateState::Requested => {
            let mut found = compute.client.find_servers(&operation, cancel)?;
            match found.len() {
                0 => {
                    report.outcome = Outcome::Unresolved;
                    return Ok(report);
                }
                1 => {
                    let server = found.remove(0);
                    server.verify(&operation)?;
                    state.operation = CreateState::Bound {
                        worker_id: server.id.to_string(),
                    };
                    store.save(state)?;
                    server.id.to_string()
                }
                _ => {
                    report.outcome = Outcome::Conflicting {
                        worker_ids: found.iter().map(|server| server.id.to_string()).collect(),
                    };
                    return Ok(report);
                }
            }
        }
        CreateState::Bound { worker_id } => worker_id,
    };
    let id = parse(&bound)?;
    if journal.released.as_deref() == Some(bound.as_str()) {
        // A stop records the release before deleting the server. Until the server
        // is proven gone the stop is unfinished and is not reported as stopped;
        // stopping again finishes it.
        if compute.client.inspect_server(id, cancel)?.is_none() {
            report.worker = state.worker.as_ref().map(released).transpose()?;
        } else if !(state.stop_requested && state.stage == Stage::Stopping) {
            // A stop that recorded only its release restores its stop intent, so
            // nothing reconnects to the server before stopping again finishes it.
            state.stop_requested = true;
            state.stage = Stage::Stopping;
            store.save(state)?;
        }
        report.outcome = Outcome::Inactive { worker_id: bound };
        return Ok(report);
    }
    let Some(server) = compute.client.inspect_server(id, cancel)? else {
        report.outcome = Outcome::Missing { worker_id: bound };
        return Ok(report);
    };
    server.verify(&operation)?;
    let spec = state.spec.as_ref().ok_or(Error::Invalid("No worker was requested"))?;
    let volume = volume(compute, &journal, &operation, cancel)?;
    // A found server is recorded as the worker only as readiness would accept it.
    if !super::readiness::holds(&server, &volume)
        || !super::readiness::admitted(&server, &compute.allowed, journal.location.as_deref())
    {
        return Err(Error::Invalid(
            "The server does not hold this cloud's workspace volume where the settings allow; delete the cloud",
        ));
    }
    // A powered-off server is still billed, so it is never reported as stopped;
    // only a stop, which deletes it, stops the cloud.
    if server.status() == WorkerStatus::Stopped {
        return Err(Error::Invalid(
            "The Hetzner server is powered off but still billed; stop the cloud to release it",
        ));
    }
    let described = worker(&server, spec, &volume)?;
    described.verify(spec)?;
    described.verify_resources(spec)?;
    report.outcome = Outcome::Found { worker_id: bound };
    report.worker = Some(described);
    Ok(report)
}

/// Releases the server and keeps the workspace volume.
pub(in crate::cloud_runtime) fn stop(
    store: &Store,
    state: &mut Deployment,
    settings: &Settings,
    cancel: &Cancellation,
) -> Result<()> {
    stop_with(&Compute::cleanup(settings)?, store, state, cancel)
}

/// As `stop`, with the Hetzner client given.
pub(super) fn stop_with(compute: &Compute, store: &Store, state: &mut Deployment, cancel: &Cancellation) -> Result<()> {
    let CreateState::Bound { worker_id } = state.operation.clone() else {
        return Err(Error::Invalid("Reconcile a bound worker before stopping it"));
    };
    let mut journal = Journal::load(store.root())?;
    let operation = state.cloud_id.clone();
    let id = parse(&worker_id)?;
    if journal.released.as_deref() != Some(worker_id.as_str()) {
        let server = compute
            .client
            .inspect_server(id, cancel)?
            .ok_or(CloudError::WorkerLost)?;
        server.verify(&operation)?;
        // Recorded first, before the shutdown and the delete, so until the server
        // is proven gone the cloud reads as an unfinished stop, never as stopped
        // or lost, and check reports no worker for it.
        journal.released = Some(worker_id.clone());
        journal.save(store.root())?;
    }
    state.stop_requested = true;
    state.stage = Stage::Stopping;
    store.save(state)?;
    // A retried stop also shuts down a server that still runs before deleting it.
    // Deleting proves the server gone.
    settle_shutdown(compute, &operation, id, cancel)?;
    let mut fence = state.operation.clone();
    compute
        .client
        .delete_server(&operation, &mut fence, cancel, |_| Ok(()), |_| {})?;
    state.worker = state.worker.as_ref().map(released).transpose()?;
    state.stop_requested = true;
    state.stage = Stage::Stopped;
    store.save(state)
}

/// Clears the released server's fence. The next reconnect creates a new server
/// that attaches the same volume, then waits for readiness as a first start does.
pub(in crate::cloud_runtime) fn resume(store: &Store, state: &mut Deployment) -> Result<()> {
    let mut journal = Journal::load(store.root())?;
    // Only a finished stop, which proved the released server gone, is resumed.
    let released = match &state.operation {
        CreateState::Bound { worker_id } => {
            state.stage == Stage::Stopped && journal.released.as_deref() == Some(worker_id.as_str())
        }
        _ => false,
    };
    if !released {
        return Err(Error::Invalid("Stop the Hetzner cloud before resuming it"));
    }
    state.operation = CreateState::Prepared;
    state.worker = None;
    state.stop_requested = false;
    state.stage = Stage::Readiness;
    state.timeline = Some(crate::cloud_runtime::timeline::Timeline::resume_requested(
        std::time::SystemTime::now(),
    ));
    store.save(state)?;
    journal.released = None;
    journal.save(store.root())
}

/// Deletes the server, the workspace volume and the SSH key, each proven absent.
pub(in crate::cloud_runtime) fn delete(
    store: &Store,
    state: &mut Deployment,
    settings: &Settings,
    cancel: &Cancellation,
) -> Result<()> {
    delete_with(&Compute::cleanup(settings)?, store, state, cancel)
}

/// As `delete`, with the Hetzner client given.
pub(super) fn delete_with(
    compute: &Compute,
    store: &Store,
    state: &mut Deployment,
    cancel: &Cancellation,
) -> Result<()> {
    let mut journal = Journal::load(store.root())?;
    let operation = state.cloud_id.clone();
    // Recorded first, so nothing provisions this cloud again until the delete finishes.
    if !journal.deleting {
        journal.deleting = true;
        journal.save(store.root())?;
    }
    if state.operation == CreateState::Requested {
        let mut found = unresolved(cancel, || Ok(compute.client.find_servers(&operation, cancel)?))?;
        match found.len() {
            0 => {
                // The request never created a server.
                state.operation = CreateState::Prepared;
                store.save(state)?;
            }
            1 => {
                let server = found.remove(0);
                server.verify(&operation)?;
                state.operation = CreateState::Bound {
                    worker_id: server.id.to_string(),
                };
                store.save(state)?;
            }
            _ => return Err(CloudError::DuplicateWorkers.into()),
        }
    }
    // A released server is deleted too: its stop may not have confirmed the delete.
    if matches!(state.operation, CreateState::Bound { .. }) {
        let mut fence = state.operation.clone();
        compute.client.delete_server(
            &operation,
            &mut fence,
            cancel,
            |next| {
                state.operation = next.clone();
                store.save(state).map_err(|_| CloudError::Persistence)
            },
            |_| {},
        )?;
    }
    let mut fence = journal.volume.clone();
    if fence == CreateState::Requested {
        let found = unresolved(cancel, || Ok(compute.client.find_volumes(&operation, cancel)?))?;
        match found.as_slice() {
            [] => fence = CreateState::Prepared,
            [volume] => {
                volume.verify(&operation)?;
                fence = CreateState::Bound {
                    worker_id: volume.id.to_string(),
                };
            }
            _ => return Err(CloudError::DuplicateWorkers.into()),
        }
    }
    if matches!(fence, CreateState::Bound { .. }) {
        let root = store.root().to_path_buf();
        let snapshot = journal.clone();
        compute.client.delete_volume(
            &operation,
            &mut fence,
            cancel,
            |next| {
                let mut saved = snapshot.clone();
                saved.volume = next.clone();
                saved.save(&root).map_err(|_| CloudError::Persistence)
            },
            |_| {},
        )?;
    }
    journal.volume = fence;
    journal.released = None;
    journal.save(store.root())?;
    // The key is recorded before it is registered, so without a record there is
    // none; with one, a registration whose response was lost is looked for twice.
    if journal.key.is_some()
        && !unresolved(cancel, || Ok(compute.client.find_ssh_keys(&operation, cancel)?))?.is_empty()
    {
        compute.client.delete_ssh_key(&operation, cancel)?;
    }
    // The key is gone and proven absent, so the cloud no longer keeps anything.
    journal.key = None;
    journal.save(store.root())
}

/// How long an uncertain request may still be processed after its response was lost.
#[cfg(not(test))]
const UNRESOLVED_GRACE: Duration = Duration::from_secs(30);
#[cfg(test)]
const UNRESOLVED_GRACE: Duration = Duration::ZERO;

/// What an uncertain create request left, found by label. Nothing found is trusted
/// only after a second look a grace period later, so a request Hetzner was still
/// processing when its response was lost has become visible.
fn unresolved<T>(cancel: &Cancellation, find: impl Fn() -> Result<Vec<T>>) -> Result<Vec<T>> {
    let found = find()?;
    if !found.is_empty() {
        return Ok(found);
    }
    let deadline = Instant::now() + UNRESOLVED_GRACE;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        cancel.check()?;
    }
    find()
}

/// Asks the operating system to shut down so the workspace is written out, and
/// cuts power if it has not finished within the grace period.
fn settle_shutdown(compute: &Compute, operation: &str, id: u64, cancel: &Cancellation) -> Result<()> {
    let off = |cancel: &Cancellation| -> Result<bool> {
        Ok(compute
            .client
            .inspect_server(id, cancel)?
            .is_none_or(|server| server.status() == WorkerStatus::Stopped))
    };
    if off(cancel)? {
        return Ok(());
    }
    compute.client.shutdown(operation, id, cancel)?;
    let deadline = Instant::now() + SHUTDOWN_GRACE;
    while Instant::now() < deadline {
        if off(cancel)? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2));
        cancel.check()?;
    }
    compute.client.power_off(operation, id, cancel)?;
    Ok(())
}

fn volume(
    compute: &Compute,
    journal: &Journal,
    operation: &str,
    cancel: &Cancellation,
) -> Result<horizon_cloud::hetzner::volumes::Volume> {
    let CreateState::Bound { worker_id } = &journal.volume else {
        return Err(Error::Invalid("Missing workspace volume identity"));
    };
    let volume = compute
        .client
        .inspect_volume(parse(worker_id)?, cancel)?
        .ok_or(CloudError::WorkerLost)?;
    volume.verify(operation)?;
    Ok(volume)
}

/// The last known worker, as a stopped worker with no endpoint.
fn released(worker: &Worker) -> Result<Worker> {
    let mut value = serde_json::to_value(worker).map_err(|_| Error::Json)?;
    value["desiredStatus"] = "EXITED".into();
    value["publicIp"] = "".into();
    value["portMappings"] = serde_json::Value::Null;
    serde_json::from_value(value).map_err(|_| Error::Json)
}

fn parse(id: &str) -> Result<u64> {
    id.parse().map_err(|_| Error::Invalid("Invalid worker ID"))
}
