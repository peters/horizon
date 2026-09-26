//! One-shot process launch and conservative, signed lifecycle observation.
pub(super) mod attachment;
mod policy;
mod process;
mod records;
mod supervisor;
use super::{
    membership,
    recovery::{BOOTSTRAP, Bootstrap, decode, read_request},
    runtime::Runtime,
    sessions,
    store::{Store, invalid},
};
use horizon_cloud_protocol::{
    OperationId, ProjectIdentity,
    bootstrap::RecoveryRequest,
    membership::{Manifest, Receipt, Request, State},
    session_runtime::{Observation, Request as Query, Status},
    signed::{Action, SignedIntent, Target},
};
use records::Record;
use std::{
    io::{self, Write},
    time::{Duration, Instant},
};
pub(super) fn open() -> io::Result<Store> {
    supervisor::open(Instant::now() + Duration::from_secs(10))
}
pub(super) fn supervise() -> io::Result<()> {
    supervisor::run()
}

pub(super) fn preflight(store: &Store, manifest: &Manifest, receipt: &Receipt, payload: &Request) -> io::Result<()> {
    if let Request::StartSession { session_id } = payload {
        sessions::published(store, manifest, &receipt.identity, *session_id)?;
        if records::launch(manifest, &receipt.identity, *session_id)?.is_none() {
            if store.read(&Record::name(*session_id))?.is_some() {
                return Err(invalid());
            }
            policy::qualify(store)?;
        }
    }
    Ok(())
}
pub(super) fn commit(
    store: &Store,
    manifest: &Manifest,
    receipt: &Receipt,
    payload: &Request,
    first_application: bool,
) -> io::Result<()> {
    let Request::StartSession { session_id } = payload else {
        return Ok(());
    };
    let member = manifest
        .members
        .iter()
        .find(|m| m.identity == receipt.identity)
        .ok_or_else(invalid)?;
    if !first_application || member.stops.contains(session_id) {
        return Ok(());
    }
    if Record::load(store, manifest, &receipt.identity, *session_id)?.is_some() {
        return Ok(());
    }
    // A committed logical intent is not evidence that no process ran. Only this
    // first exclusive durable fence can lead to a supervisor; exact retries of
    // any later phase merely observe it, including failed spawn and lost handoff.
    let mut record = Record {
        version: 1,
        launch: receipt.clone(),
        session: *session_id,
        nonce: OperationId::generate(),
        supervisor: None,
        agent: None,
        status: Status::Launching,
        endpoint: None,
    };
    record.save(store, None)?;
    supervisor::spawn(store, manifest, &mut record)
}

pub(super) fn validate(store: &Store, manifest: &Manifest, cancelling: Option<&ProjectIdentity>) -> io::Result<()> {
    for member in &manifest.members {
        for id in &member.launches {
            let saved = Record::load(store, manifest, &member.identity, *id)?;
            if (cancelling == Some(&member.identity) || member.state == State::Removed)
                && saved
                    .as_ref()
                    .is_none_or(|(record, _)| record.status != Status::Stopped)
            {
                return Err(invalid());
            }
            if let Some((record, bytes)) = saved
                && record.status == Status::Stopped
            {
                record.terminal_barrier(store, &bytes)?;
            }
        }
    }
    Ok(())
}
pub(super) fn inspect() -> io::Result<()> {
    if std::env::args().len() != 2 {
        return Err(invalid());
    }
    let request = read_request(io::stdin().lock())?;
    let store = open()?;
    let result = inspect_with(&store, &Runtime::captured()?, &request)?;
    serde_json::to_writer(io::stdout().lock(), &result)?;
    io::stdout().lock().write_all(b"\n")
}
pub(super) fn inspect_with(store: &Store, runtime: &Runtime, request: &RecoveryRequest) -> io::Result<Observation> {
    let bootstrap: Bootstrap = decode(&store.read(BOOTSTRAP)?.ok_or_else(invalid)?)?;
    bootstrap.validate(store, runtime)?;
    let (_, manifest) = membership::load(store, &bootstrap)?;
    let signed = SignedIntent::parse(request.message.as_bytes()).map_err(|_| invalid())?;
    let intent = signed
        .verify(&bootstrap.startup.controller, request.payload.as_bytes())
        .map_err(|_| invalid())?;
    let Target::Project { identity } = intent.target() else {
        return Err(invalid());
    };
    let query: Query = decode(request.payload.as_bytes())?;
    if intent.action() != Action::InspectProjectSession || query.session_id.is_nil() {
        return Err(invalid());
    }
    if manifest.revision != intent.expected_revision() {
        let pending = query.pending.as_ref().ok_or_else(invalid)?;
        if intent.expected_revision().checked_add(1) != Some(pending.revision)
            || manifest.revision != pending.revision
            || !manifest.operations.last().is_some_and(|entry| {
                entry.receipt.operation == pending.operation && entry.receipt.fingerprint == pending.fingerprint
            })
        {
            return Err(invalid());
        }
    }
    let member = manifest
        .members
        .iter()
        .find(|m| &m.identity == identity && m.state != State::Removed)
        .ok_or_else(invalid)?;
    if !member.sessions.iter().any(|s| s.id == query.session_id) {
        return Err(invalid());
    }
    let launch = records::launch(&manifest, identity, query.session_id)?.map(|r| r.operation);
    let status = match Record::load(store, &manifest, identity, query.session_id)? {
        Some((record, bytes)) => {
            sessions::published(store, &manifest, identity, query.session_id)?;
            if record.status == Status::Stopped {
                record.terminal_barrier(store, &bytes)?;
            }
            let status = record.observed();
            if member.stops.contains(&query.session_id) && !matches!(status, Status::Stopped | Status::Uncertain) {
                Status::Stopping
            } else {
                status
            }
        }
        None if launch.is_some() => Status::Uncertain,
        None => Status::NotStarted,
    };
    Ok(Observation {
        version: 1,
        startup: bootstrap.startup,
        worker_id: bootstrap.worker_id,
        project: identity.clone(),
        session_id: query.session_id,
        operation: intent.operation(),
        fingerprint: intent.fingerprint().map_err(|_| invalid())?,
        revision: manifest.revision,
        launch,
        status,
    })
}
