//! Check, stop, resume and delete. A powered-off Hetzner server is still billed,
//! so stopping releases it: the server is deleted and the workspace volume kept.
//! The cloud stays bound to the released server; resuming clears that fence so
//! the next provisioning creates a new server in the volume's location that
//! attaches the same volume.
use super::{Journal, Policy, Records, admitted, holds, worker};
use crate::{
    Cancellation, CloudError, CreateState, Worker, WorkerSpec, WorkerStatus,
    hetzner::{Hetzner, volumes::Volume},
};
use std::time::{Duration, Instant};

/// How long a graceful shutdown may take before power is cut.
const SHUTDOWN_GRACE: Duration = Duration::from_mins(1);
/// How long an uncertain create request may still be processed after its
/// response was lost, before nothing found counts as nothing created.
pub const UNRESOLVED_GRACE: Duration = Duration::from_secs(30);

/// The cloud a lifecycle action works on.
#[derive(Clone, Copy)]
pub struct Cloud<'a> {
    pub client: &'a Hetzner,
    /// The operation ID that names and labels the cloud's resources.
    pub operation_id: &'a str,
    pub cancel: &'a Cancellation,
}

/// How far a stop has come, for the caller to record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The release is recorded, but the server may still exist.
    Stopping,
    /// The released server is proven gone.
    Stopped,
}

/// The records a stop keeps besides the journal and the server fence.
pub trait StopRecords: Records {
    /// # Errors
    /// Reports a record that could not be made durable.
    fn stop(&mut self, stop: Stop) -> Result<(), CloudError>;
}

/// What a check found.
#[derive(Clone, Debug)]
pub enum Check {
    /// No server was requested.
    Prepared,
    Terminated {
        worker_id: String,
    },
    /// A server request whose result is unknown, and no server carries its label.
    Unresolved,
    Conflicting {
        worker_ids: Vec<String>,
    },
    /// A server a stop released; `gone` once it is proven absent, which finishes the stop.
    Released {
        worker_id: String,
        gone: bool,
    },
    Missing {
        worker_id: String,
    },
    /// A server that meets the spec and holds exactly the cloud's volume where the policy allows.
    Found {
        worker_id: String,
        worker: Box<Worker>,
    },
}

/// Checks the recorded server without creating, starting or deleting anything.
/// A server found by label for an uncertain request is bound once its identity
/// is verified, so a delete can clean it up. Ownership checks alone settle a
/// released or missing server; `policy` is asked for only when a live server is
/// reported as the worker, so a changed policy never blocks finishing a stop.
/// # Errors
/// Refuses a server that is not the cloud's worker, including a powered-off one
/// a stop did not release, which is still billed.
pub fn check(
    cloud: Cloud<'_>,
    spec: Option<&WorkerSpec>,
    operation: &mut CreateState,
    journal: &Journal,
    policy: &dyn Fn() -> Result<Policy, CloudError>,
    records: &mut impl StopRecords,
) -> Result<Check, CloudError> {
    let Cloud {
        client,
        operation_id,
        cancel,
    } = cloud;
    let bound = match operation.clone() {
        CreateState::Prepared => return Ok(Check::Prepared),
        CreateState::Terminated { worker_id } => return Ok(Check::Terminated { worker_id }),
        CreateState::Requested => {
            let mut found = client.find_servers(operation_id, cancel)?;
            match found.len() {
                0 => return Ok(Check::Unresolved),
                1 => {
                    let server = found.remove(0);
                    server.verify(operation_id)?;
                    let next = CreateState::Bound {
                        worker_id: server.id.to_string(),
                    };
                    records.operation(&next)?;
                    *operation = next;
                    server.id.to_string()
                }
                _ => {
                    return Ok(Check::Conflicting {
                        worker_ids: found.iter().map(|server| server.id.to_string()).collect(),
                    });
                }
            }
        }
        CreateState::Bound { worker_id } => worker_id,
    };
    let id = parse(&bound)?;
    if journal.released.as_deref() == Some(bound.as_str()) {
        // A stop records the release before deleting the server. Once the server
        // is proven gone the stop is finished, even if it was interrupted before
        // recording so; until then it is unfinished and stopping again finishes it.
        let gone = client.inspect_server(id, cancel)?.is_none();
        records.stop(if gone { Stop::Stopped } else { Stop::Stopping })?;
        return Ok(Check::Released { worker_id: bound, gone });
    }
    let Some(server) = client.inspect_server(id, cancel)? else {
        return Ok(Check::Missing { worker_id: bound });
    };
    server.verify(operation_id)?;
    let spec = spec.ok_or(CloudError::Invalid("No worker was requested"))?;
    let volume = bound_volume(client, journal, operation_id, cancel)?;
    // A found server is a worker only as readiness would accept it.
    if !holds(&server, &volume) || !admitted(&server, &policy()?, journal.location.as_deref()) {
        return Err(CloudError::Invalid(
            "The server does not hold this cloud's workspace volume where the settings allow; delete the cloud",
        ));
    }
    // A powered-off server is still billed, so it is never reported as stopped;
    // only a stop, which deletes it, stops the cloud.
    if server.status() == WorkerStatus::Stopped {
        return Err(CloudError::Invalid(
            "The Hetzner server is powered off but still billed; stop the cloud to release it",
        ));
    }
    let described = worker(&server, spec, &volume)?;
    described.verify(spec)?;
    described.verify_resources(spec)?;
    Ok(Check::Found {
        worker_id: bound,
        worker: Box::new(described),
    })
}

/// Releases the bound server and keeps the workspace volume. The release is
/// recorded first, then the stopping stop, both before the shutdown and the
/// delete, so a crash or lost response at any point reads as an unfinished stop.
/// # Errors
/// Refuses an unbound or foreign server and reports provider failures.
pub fn stop(
    cloud: Cloud<'_>,
    operation: &CreateState,
    journal: &mut Journal,
    records: &mut impl StopRecords,
) -> Result<(), CloudError> {
    let Cloud {
        client,
        operation_id,
        cancel,
    } = cloud;
    let CreateState::Bound { worker_id } = operation else {
        return Err(CloudError::Invalid("Reconcile a bound worker before stopping it"));
    };
    let id = parse(worker_id)?;
    if journal.released.as_deref() != Some(worker_id.as_str()) {
        client
            .inspect_server(id, cancel)?
            .ok_or(CloudError::WorkerLost)?
            .verify(operation_id)?;
        save(
            journal,
            Journal {
                released: Some(worker_id.clone()),
                ..journal.clone()
            },
            records,
        )?;
    }
    records.stop(Stop::Stopping)?;
    // A retried stop also shuts down a server that still runs before deleting it.
    // Deleting proves the server gone.
    settle_shutdown(client, operation_id, id, cancel)?;
    let mut fence = operation.clone();
    client.delete_server(operation_id, &mut fence, cancel, |_| Ok(()), |_| {})?;
    records.stop(Stop::Stopped)
}

/// Whether a stopped cloud can resume: only a finished stop, which proved the
/// released server gone. Resuming clears the server fence and the release.
/// # Errors
/// Refuses anything but a finished stop.
pub fn resumable(operation: &CreateState, journal: &Journal, stopped: bool) -> Result<(), CloudError> {
    match operation {
        CreateState::Bound { worker_id } if stopped && journal.released.as_deref() == Some(worker_id.as_str()) => {
            Ok(())
        }
        _ => Err(CloudError::Invalid("Stop the Hetzner cloud before resuming it")),
    }
}

/// Deletes the server, the workspace volume and the SSH key, each proven absent.
/// The delete intent is recorded first, so nothing provisions the cloud again
/// until it finishes. An uncertain request that seems to have created nothing is
/// looked for again after `grace` (see [`UNRESOLVED_GRACE`]).
/// # Errors
/// Reports provider and persistence failures, leaving records a retry resumes from.
pub fn delete(
    cloud: Cloud<'_>,
    operation: &mut CreateState,
    journal: &mut Journal,
    records: &mut impl Records,
    grace: Duration,
) -> Result<(), CloudError> {
    let Cloud {
        client,
        operation_id,
        cancel,
    } = cloud;
    if !journal.deleting {
        save(
            journal,
            Journal {
                deleting: true,
                ..journal.clone()
            },
            records,
        )?;
    }
    if *operation == CreateState::Requested {
        let mut found = unresolved(grace, cancel, || client.find_servers(operation_id, cancel))?;
        let next = match found.len() {
            // The request never created a server.
            0 => CreateState::Prepared,
            1 => {
                let server = found.remove(0);
                server.verify(operation_id)?;
                CreateState::Bound {
                    worker_id: server.id.to_string(),
                }
            }
            _ => return Err(CloudError::DuplicateWorkers),
        };
        records.operation(&next)?;
        *operation = next;
    }
    // A released server is deleted too: its stop may not have confirmed the delete.
    if matches!(operation, CreateState::Bound { .. }) {
        client.delete_server(operation_id, operation, cancel, |next| records.operation(next), |_| {})?;
    }
    let mut fence = journal.volume.clone();
    if fence == CreateState::Requested {
        fence = match unresolved(grace, cancel, || client.find_volumes(operation_id, cancel))?.as_slice() {
            [] => CreateState::Prepared,
            [volume] => {
                volume.verify(operation_id)?;
                CreateState::Bound {
                    worker_id: volume.id.to_string(),
                }
            }
            _ => return Err(CloudError::DuplicateWorkers),
        };
    }
    if matches!(fence, CreateState::Bound { .. }) {
        let snapshot = journal.clone();
        client.delete_volume(
            operation_id,
            &mut fence,
            cancel,
            |next| {
                let mut saved = snapshot.clone();
                saved.volume = next.clone();
                records.journal(&saved)
            },
            |_| {},
        )?;
    }
    save(
        journal,
        Journal {
            volume: fence,
            released: None,
            ..journal.clone()
        },
        records,
    )?;
    // The key is recorded before it is registered, so without a record there is
    // none; with one, a registration whose response was lost is looked for twice.
    if journal.key.is_some() && !unresolved(grace, cancel, || client.find_ssh_keys(operation_id, cancel))?.is_empty() {
        client.delete_ssh_key(operation_id, cancel)?;
    }
    // The key is gone and proven absent, so the cloud no longer keeps anything.
    save(
        journal,
        Journal {
            key: None,
            ..journal.clone()
        },
        records,
    )
}

/// Saves `next` and only then lets the caller's journal take it, so a failed
/// save leaves the journal as it is on disk and a retry saves the change again.
fn save(journal: &mut Journal, next: Journal, records: &mut impl Records) -> Result<(), CloudError> {
    records.journal(&next)?;
    *journal = next;
    Ok(())
}

/// The last known worker, as a stopped worker with no endpoint.
/// # Errors
/// Fails only if the worker cannot be described.
pub fn released(worker: &Worker) -> Result<Worker, CloudError> {
    let invalid = |_| CloudError::Invalid("The released worker could not be described");
    let mut value = serde_json::to_value(worker).map_err(invalid)?;
    value["desiredStatus"] = "EXITED".into();
    value["publicIp"] = "".into();
    value["portMappings"] = serde_json::Value::Null;
    serde_json::from_value(value).map_err(invalid)
}

/// What an uncertain create request left, found by label. Nothing found is
/// trusted only after a second look `grace` later, so a request Hetzner was
/// still processing when its response was lost has become visible.
fn unresolved<T>(
    grace: Duration,
    cancel: &Cancellation,
    find: impl Fn() -> Result<Vec<T>, CloudError>,
) -> Result<Vec<T>, CloudError> {
    let found = find()?;
    if !found.is_empty() {
        return Ok(found);
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500).min(grace));
        cancel.check()?;
    }
    find()
}

/// Asks the operating system to shut down so the workspace is written out, and
/// cuts power if it has not finished within the grace period.
fn settle_shutdown(client: &Hetzner, operation_id: &str, id: u64, cancel: &Cancellation) -> Result<(), CloudError> {
    let off = |cancel: &Cancellation| -> Result<bool, CloudError> {
        Ok(client
            .inspect_server(id, cancel)?
            .is_none_or(|server| server.status() == WorkerStatus::Stopped))
    };
    if off(cancel)? {
        return Ok(());
    }
    client.shutdown(operation_id, id, cancel)?;
    let deadline = Instant::now() + SHUTDOWN_GRACE;
    while Instant::now() < deadline {
        if off(cancel)? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2));
        cancel.check()?;
    }
    client.power_off(operation_id, id, cancel)
}

/// The cloud's bound workspace volume, checked to be its own.
fn bound_volume(
    client: &Hetzner,
    journal: &Journal,
    operation_id: &str,
    cancel: &Cancellation,
) -> Result<Volume, CloudError> {
    let CreateState::Bound { worker_id } = &journal.volume else {
        return Err(CloudError::Invalid("Missing workspace volume identity"));
    };
    let volume = client
        .inspect_volume(parse(worker_id)?, cancel)?
        .ok_or(CloudError::WorkerLost)?;
    volume.verify(operation_id)?;
    Ok(volume)
}

fn parse(id: &str) -> Result<u64, CloudError> {
    id.parse().map_err(|_| CloudError::Invalid("Invalid worker ID"))
}
