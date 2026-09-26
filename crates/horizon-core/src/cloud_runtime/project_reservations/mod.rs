//! Owning-host journals for logical project reservations, not project admission.
pub(super) mod journal;
pub(super) mod source;
use super::{
    Cancellation, bootstrap_initialization, bootstrap_recovery, bootstrap_recovery::connection::Snapshot,
    command::Runner, owner::Owner, ssh::Connection,
};
use horizon_cloud::Capabilities;
use horizon_cloud_protocol::{ProjectIdentity, membership::Receipt};
use journal::Journal;
pub use source::import_source;
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Private source artifact I/O failed")]
    ArtifactsIo(#[from] std::io::Error),
    #[error("Project reservation context, request or receipt is invalid or changed")]
    Invalid,
    #[error("A retained project operation must be resumed before another operation")]
    Pending,
    #[error("No pending project reservation operation exists")]
    Missing,
    #[error("Project reservation deadline expired")]
    Deadline,
    #[error(transparent)]
    Owner(#[from] super::owner::Error),
    #[error(transparent)]
    Bootstrap(#[from] bootstrap_initialization::Error),
    #[error(transparent)]
    Recovery(#[from] bootstrap_recovery::Error),
    #[error(transparent)]
    Transport(#[from] super::Error),
}

/// Calling interfaces must resolve the immutable owning-workspace identity before
/// invoking this controller. No paths, source or credential grants are accepted.
#[derive(Clone, PartialEq, Eq)]
pub struct Reservation {
    pub project: ProjectIdentity,
    pub image_digest: String,
    pub capabilities: Capabilities,
    pub ports: BTreeSet<u16>,
}

pub(super) enum Change {
    Reserve(Reservation),
    PrepareNamespace(ProjectIdentity),
    ImportSource(ProjectIdentity, horizon_cloud_protocol::membership::Source),
    Cancel(ProjectIdentity),
    Resume,
}

/// Durably anchor one signed reserve request before contacting the worker.
/// Identical retries reuse the original operation; receipts are historical logical
/// reservations, never fresh readiness or permission to provision a project.
/// # Errors
/// Rejects changed ownership, image, capabilities, SSH material and pending inputs.
/// Provider reads and synchronous durability checks retain their own time bounds;
/// expiry of `timeout` prevents further SSH sends. No provider mutations occur.
pub fn reserve(
    owner: &mut Owner,
    allocation: &bootstrap_initialization::Request,
    request: &Reservation,
    cancel: &Cancellation,
    timeout: Duration,
) -> Result<Receipt> {
    execute(owner, allocation, &Change::Reserve(request.clone()), cancel, timeout)
}

/// Prepare the reserved project's private directory layout without importing source
/// or starting processes. A receipt is historical evidence, not current readiness.
/// # Errors
/// Refuses conflicting pending operations, cancelled projects and changed bindings.
pub fn prepare_namespace(
    owner: &mut Owner,
    allocation: &bootstrap_initialization::Request,
    project: &ProjectIdentity,
    cancellation: &Cancellation,
    timeout: Duration,
) -> Result<Receipt> {
    execute(
        owner,
        allocation,
        &Change::PrepareNamespace(project.clone()),
        cancellation,
        timeout,
    )
}

/// Cancel only a known logical reservation. No project or provider data is deleted.
/// # Errors
/// Refuses conflicting pending operations, unknown identities and changed bindings.
pub fn cancel(
    owner: &mut Owner,
    allocation: &bootstrap_initialization::Request,
    project: &ProjectIdentity,
    cancellation: &Cancellation,
    timeout: Duration,
) -> Result<Receipt> {
    execute(
        owner,
        allocation,
        &Change::Cancel(project.clone()),
        cancellation,
        timeout,
    )
}

/// Resume the exact anchored request after a lost response or host restart.
/// # Errors
/// Never signs a replacement operation; missing or conflicting state remains fenced.
pub fn resume(
    owner: &mut Owner,
    allocation: &bootstrap_initialization::Request,
    cancellation: &Cancellation,
    timeout: Duration,
) -> Result<Receipt> {
    execute(owner, allocation, &Change::Resume, cancellation, timeout)
}

pub(super) fn started(owner: &Owner) -> std::result::Result<bool, super::owner::Error> {
    // Even malformed retained state must fence the older pre-admission lifecycle.
    Ok(owner.load()?.get(journal::KEY).is_some())
}

fn execute(
    owner: &mut Owner,
    allocation: &bootstrap_initialization::Request,
    change: &Change,
    cancellation: &Cancellation,
    timeout: Duration,
) -> Result<Receipt> {
    let deadline = Instant::now() + timeout.min(Duration::from_secs(180));
    remaining(deadline)?;
    cancellation.check().map_err(super::Error::from)?;
    let saved = Journal::load(owner)?;
    let image = match change {
        Change::Reserve(request) => request.image_digest.clone(),
        _ => saved.as_ref().ok_or(Error::Missing)?.image_digest.clone(),
    };
    // Refuse changed inputs and successor actions before provider reads as well.
    if let Some(saved) = &saved {
        saved.require_change(change)?;
    }
    let target = bootstrap_initialization::project_target(owner, allocation, &image, cancellation)?;
    let runner = Runner {
        cancel: cancellation,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let artifact_root = owner.artifact_root()?.to_owned();
    coordinate(owner, &target, &image, change, &mut |connection, command, bytes| {
        remaining(deadline)?;
        if command == "horizon-cloud-worker import-project-source" {
            let artifacts = source::for_request(saved.as_ref().ok_or(Error::Missing)?, bytes)?;
            let input = artifacts.frame(&artifact_root, bytes, cancellation)?;
            Ok(runner.private_file_exchange(&mut connection.pinned_command(command), input, remaining(deadline)?)?)
        } else {
            if command == "horizon-cloud-worker prepare-project-source" {
                source::for_request(saved.as_ref().ok_or(Error::Missing)?, bytes)?
                    .verify(&artifact_root, cancellation)?;
            }
            Ok(runner.private_exchange(&mut connection.pinned_command(command), bytes, remaining(deadline)?)?)
        }
    })
}

pub(super) fn coordinate(
    owner: &mut Owner,
    target: &bootstrap_recovery::Target,
    image: &str,
    change: &Change,
    exchange: &mut impl FnMut(&Connection, &str, &[u8]) -> Result<Vec<u8>>,
) -> Result<Receipt> {
    let snapshot = Snapshot::capture(target)?;
    if bootstrap_recovery::require_existing(owner, target)? != snapshot.binding {
        return Err(Error::Invalid);
    }
    let mut journal = Journal::load(owner)?.unwrap_or_else(|| Journal::new(snapshot.binding.clone(), image.into()));
    if journal.binding != snapshot.binding || journal.image_digest != image {
        return Err(Error::Invalid);
    }
    journal.prepare(owner, change)?;
    journal.save(owner)?;
    let pending = journal.pending.as_ref().ok_or(Error::Missing)?;
    // Recheck the anchored payload and original material after the pending save.
    if Journal::load(owner)?.as_ref() != Some(&journal)
        || bootstrap_recovery::require_existing(owner, target)? != snapshot.binding
    {
        return Err(Error::Invalid);
    }
    let reply = exchange(&snapshot.connection, pending.command()?, pending.request.as_bytes())?;
    if reply.len() > horizon_cloud_protocol::membership::MAX_MANIFEST_BYTES
        || serde_json::from_slice::<Receipt>(&reply).map_err(|_| Error::Invalid)? != pending.receipt
    {
        return Err(Error::Invalid);
    }
    if Journal::load(owner)?.as_ref() != Some(&journal)
        || bootstrap_recovery::require_existing(owner, target)? != snapshot.binding
    {
        return Err(Error::Invalid);
    }
    if pending.command()? == "horizon-cloud-worker prepare-project-source" {
        let reply = exchange(
            &snapshot.connection,
            "horizon-cloud-worker import-project-source",
            pending.request.as_bytes(),
        )?;
        if reply.len() > horizon_cloud_protocol::membership::MAX_MANIFEST_BYTES
            || serde_json::from_slice::<Receipt>(&reply).map_err(|_| Error::Invalid)? != pending.receipt
            || Journal::load(owner)?.as_ref() != Some(&journal)
            || bootstrap_recovery::require_existing(owner, target)? != snapshot.binding
        {
            return Err(Error::Invalid);
        }
    }
    let receipt = pending.receipt.clone();
    journal.manifest = pending.next.clone();
    journal.pending = None;
    journal.save(owner)?;
    Ok(receipt)
}

fn remaining(deadline: Instant) -> Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(Error::Deadline)
    } else {
        Ok(remaining)
    }
}
