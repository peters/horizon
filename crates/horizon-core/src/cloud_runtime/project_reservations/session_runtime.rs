use super::{
    Cancellation, Change, Connection, Error, Journal, Owner, Result, Runner, Snapshot, bootstrap_initialization,
    bootstrap_recovery,
};
use horizon_cloud_protocol::{
    OperationId, ProjectIdentity,
    bootstrap::RecoveryRequest,
    membership::{Receipt, Request as Mutation, SessionId},
    session_runtime::{Observation, Pending, Request},
    signed::{Action, Intent, Target},
};
use std::time::{Duration, Instant};

/// Anchor one launch request. A successful historical receipt does not promise
/// that an agent is running or authenticated; inspect the live runtime separately.
/// # Errors
/// Rejects changed ownership, unsupported grants and conflicting pending requests.
pub fn start_session(
    owner: &mut Owner,
    allocation: &bootstrap_initialization::Request,
    project: &ProjectIdentity,
    session_id: SessionId,
    cancellation: &Cancellation,
    timeout: Duration,
) -> Result<Receipt> {
    super::execute(
        owner,
        allocation,
        &Change::StartSession(project.clone(), session_id),
        cancellation,
        timeout,
    )
}
/// Persist terminal stop intent. Inspect until stopped; a lost supervisor leaves
/// cleanup uncertain. This preserves session data and cannot authorize relaunch.
/// # Errors
/// Rejects unknown sessions, changed bindings and conflicting pending requests.
pub fn stop_session(
    owner: &mut Owner,
    allocation: &bootstrap_initialization::Request,
    project: &ProjectIdentity,
    session_id: SessionId,
    cancellation: &Cancellation,
    timeout: Duration,
) -> Result<Receipt> {
    super::execute(
        owner,
        allocation,
        &Change::StopSession(project.clone(), session_id),
        cancellation,
        timeout,
    )
}
/// Inspect a reserved session through its pinned owning-host connection without
/// changing or discarding an outstanding mutation journal.
/// # Errors
/// Rejects changed ownership, unknown sessions, unrelated revisions and responses.
pub fn inspect_session(
    owner: &Owner,
    allocation: &bootstrap_initialization::Request,
    project: &ProjectIdentity,
    session_id: SessionId,
    cancellation: &Cancellation,
    timeout: Duration,
) -> Result<Observation> {
    let deadline = Instant::now() + timeout.min(Duration::from_secs(180));
    cancellation.check().map_err(super::super::Error::from)?;
    let saved = Journal::load(owner)?.ok_or(Error::Missing)?;
    let target = bootstrap_initialization::project_target(owner, allocation, &saved.image_digest, cancellation)?;
    let runner = Runner {
        cancel: cancellation,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    inspect_with(owner, &target, project, session_id, &mut |connection, bytes| {
        Ok(runner.private_exchange(
            &mut connection.pinned_command("horizon-cloud-worker inspect-project-session"),
            bytes,
            super::remaining(deadline)?,
        )?)
    })
}

pub(in crate::cloud_runtime) fn inspect_with(
    owner: &Owner,
    target: &bootstrap_recovery::Target,
    project: &ProjectIdentity,
    session_id: SessionId,
    exchange: &mut impl FnMut(&Connection, &[u8]) -> Result<Vec<u8>>,
) -> Result<Observation> {
    let journal = Journal::load(owner)?.ok_or(Error::Missing)?;
    let snapshot = Snapshot::capture(target)?;
    if journal.binding != snapshot.binding || bootstrap_recovery::require_existing(owner, target)? != snapshot.binding {
        return Err(Error::Invalid);
    }
    let known = journal
        .pending
        .as_ref()
        .map_or(&journal.manifest, |pending| &pending.next);
    if !known
        .members
        .iter()
        .any(|m| &m.identity == project && m.sessions.iter().any(|s| s.id == session_id))
    {
        return Err(Error::Invalid);
    }
    let payload = serde_json::to_string(&Request {
        session_id,
        pending: journal.pending.as_ref().map(|p| Pending {
            operation: p.receipt.operation,
            revision: p.receipt.revision,
            fingerprint: p.receipt.fingerprint,
        }),
    })
    .map_err(|_| Error::Invalid)?;
    let intent = Intent::new(
        &snapshot.binding.startup.controller,
        OperationId::generate(),
        journal.manifest.revision,
        Target::Project {
            identity: project.clone(),
        },
        Action::InspectProjectSession,
        payload.as_bytes(),
    )
    .map_err(|_| Error::Invalid)?;
    let operation = intent.operation();
    let fingerprint = intent.fingerprint().map_err(|_| Error::Invalid)?;
    let message = serde_json::to_string(&owner.sign(intent)?).map_err(|_| Error::Invalid)?;
    let bytes = serde_json::to_vec(&RecoveryRequest { message, payload }).map_err(|_| Error::Invalid)?;
    let reply = exchange(&snapshot.connection, &bytes)?;
    if reply.len() > 65536 {
        return Err(Error::Invalid);
    }
    let observation: Observation = serde_json::from_slice(&reply).map_err(|_| Error::Invalid)?;
    let observed = if observation.revision == journal.manifest.revision {
        &journal.manifest
    } else if observation.revision == known.revision {
        known
    } else {
        return Err(Error::Invalid);
    };
    let launch = observed
        .operations
        .iter()
        .find(|entry| {
            &entry.receipt.identity == project
                && serde_json::from_str::<Mutation>(&entry.payload)
                    .is_ok_and(|request| request == (Mutation::StartSession { session_id }))
        })
        .map(|entry| entry.receipt.operation);
    let stopped = observed
        .members
        .iter()
        .find(|m| &m.identity == project)
        .is_some_and(|m| m.stops.contains(&session_id));
    let status_valid = match observation.status {
        horizon_cloud_protocol::session_runtime::Status::NotStarted => launch.is_none(),
        horizon_cloud_protocol::session_runtime::Status::Stopped
        | horizon_cloud_protocol::session_runtime::Status::Stopping => launch.is_some() && stopped,
        horizon_cloud_protocol::session_runtime::Status::Uncertain => launch.is_some(),
        _ => launch.is_some() && !stopped,
    };
    if observation.version != 1
        || observation.startup != snapshot.binding.startup
        || observation.worker_id != snapshot.binding.worker_id
        || &observation.project != project
        || observation.session_id != session_id
        || observation.operation != operation
        || observation.fingerprint != fingerprint
        || observation.launch != launch
        || !status_valid
        || Journal::load(owner)?.as_ref() != Some(&journal)
        || bootstrap_recovery::require_existing(owner, target)? != snapshot.binding
    {
        return Err(Error::Invalid);
    }
    Ok(observation)
}
