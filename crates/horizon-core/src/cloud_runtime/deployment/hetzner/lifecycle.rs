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
    let compute = Compute::new(settings)?;
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
    if journal.released.as_deref() == Some(bound.as_str()) {
        report.worker = state.worker.as_ref().map(released).transpose()?;
        report.outcome = Outcome::Inactive { worker_id: bound };
        return Ok(report);
    }
    let id = parse(&bound)?;
    let Some(server) = compute.client.inspect_server(id, cancel)? else {
        report.outcome = Outcome::Missing { worker_id: bound };
        return Ok(report);
    };
    server.verify(&operation)?;
    let spec = state.spec.as_ref().ok_or(Error::Invalid("No worker was requested"))?;
    let volume = volume(&compute, &journal, &operation, cancel)?;
    // A found server is recorded as the worker only as readiness would accept it.
    if !super::readiness::holds(&server, &volume)
        || !super::readiness::admitted(&server, &compute.allowed, journal.location.as_deref())
    {
        return Err(Error::Invalid(
            "The server does not hold this cloud's workspace volume where the settings allow; delete the cloud",
        ));
    }
    let described = worker(&server, spec, &volume)?;
    described.verify(spec)?;
    described.verify_resources(spec)?;
    report.outcome = if server.status() == WorkerStatus::Stopped {
        Outcome::Inactive { worker_id: bound }
    } else {
        Outcome::Found { worker_id: bound }
    };
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
    let CreateState::Bound { worker_id } = state.operation.clone() else {
        return Err(Error::Invalid("Reconcile a bound worker before stopping it"));
    };
    let compute = Compute::new(settings)?;
    let mut journal = Journal::load(store.root())?;
    let operation = state.cloud_id.clone();
    if journal.released.as_deref() != Some(worker_id.as_str()) {
        let id = parse(&worker_id)?;
        let server = compute
            .client
            .inspect_server(id, cancel)?
            .ok_or(CloudError::WorkerLost)?;
        server.verify(&operation)?;
        let previous = (state.stop_requested, state.stage);
        state.stop_requested = true;
        state.stage = Stage::Stopping;
        store.save(state)?;
        if let Err(error) = settle_shutdown(&compute, &operation, id, cancel) {
            (state.stop_requested, state.stage) = previous;
            store.save(state)?;
            return Err(error);
        }
        // Recorded before the delete request, so a lost response still reads as a
        // stop of this server rather than a lost worker.
        journal.released = Some(worker_id.clone());
        journal.save(store.root())?;
        let mut fence = state.operation.clone();
        compute
            .client
            .delete_server(&operation, &mut fence, cancel, |_| Ok(()), |_| {})?;
    }
    state.worker = state.worker.as_ref().map(released).transpose()?;
    state.stop_requested = true;
    state.stage = Stage::Stopped;
    store.save(state)
}

/// Clears the released server's fence. The next reconnect creates a new server
/// that attaches the same volume, then waits for readiness as a first start does.
pub(in crate::cloud_runtime) fn resume(store: &Store, state: &mut Deployment) -> Result<()> {
    let mut journal = Journal::load(store.root())?;
    let released = match &state.operation {
        CreateState::Bound { worker_id } => journal.released.as_deref() == Some(worker_id.as_str()),
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
    delete_with(&Compute::new(settings)?, store, state, cancel)
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
    if state.operation == CreateState::Requested {
        let mut found = compute.client.find_servers(&operation, cancel)?;
        match found.len() {
            0 => {
                return Err(Error::Invalid(
                    "The server request is unresolved; check the Hetzner project before deleting",
                ));
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
    if let CreateState::Bound { worker_id } = state.operation.clone() {
        if journal.released.as_deref() == Some(worker_id.as_str()) {
            state.operation = CreateState::Terminated { worker_id };
            store.save(state)?;
        } else {
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
    }
    let mut fence = journal.volume.clone();
    if fence == CreateState::Requested {
        let found = compute.client.find_volumes(&operation, cancel)?;
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
    compute.client.delete_ssh_key(&operation, cancel)?;
    // The key is gone and proven absent, so the cloud no longer keeps anything.
    journal.key = None;
    journal.save(store.root())
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
