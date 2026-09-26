//! Anchored, incremental setup of one project on an already initialized allocation.
mod capacity;
mod intent;
mod progress;
use super::{
    Cancellation, bootstrap_initialization, bootstrap_recovery,
    owner::Owner,
    project_reservations::{self as reservations, Change, journal::Journal},
};
use horizon_cloud::{Agent, Capabilities};
use horizon_cloud_protocol::{
    ProjectIdentity,
    membership::{Session, Source},
};
use intent::{Intent, Registry};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Project setup inputs, ownership or retained history are invalid or changed")]
    Invalid,
    #[error("Project setup exceeds retained membership capacity")]
    Capacity,
    #[error("Project setup has not been recorded")]
    Missing,
    #[error("Retained work blocks this project setup")]
    Blocked,
    #[error("Project setup deadline expired")]
    Deadline,
    #[error(transparent)]
    Owner(#[from] super::owner::Error),
    #[error(transparent)]
    Bootstrap(#[from] bootstrap_initialization::Error),
    #[error(transparent)]
    Recovery(#[from] bootstrap_recovery::Error),
    #[error(transparent)]
    Reservation(#[from] reservations::Error),
    #[error(transparent)]
    Transport(#[from] super::Error),
    #[error("Project setup repository I/O failed")]
    Io(#[from] std::io::Error),
}

/// Internal input, after caller ownership and immutable profile qualification.
/// This does not accept credentials, arbitrary commands, tools or service ports.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub project: ProjectIdentity,
    pub repository: PathBuf,
    pub selection: String,
    pub image_digest: String,
    pub capabilities: Capabilities,
    pub agents: Vec<Agent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Reserve,
    PrepareNamespace,
    ImportSource,
    ReserveSession(uuid::Uuid),
    PrepareSession(uuid::Uuid),
    StartSession(uuid::Uuid),
    /// Historical settled launch intents; not process or application readiness.
    Complete,
    /// A terminal stop or cancellation forbids further setup progress.
    Terminal,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub project: ProjectIdentity,
    pub revision: String,
    pub sessions: Vec<Session>,
    pub next: Step,
    pub pending: bool,
}

/// Save stable session identities and the selected commit before worker effects.
/// An identical retry returns retained identities even if the symbolic ref moved.
/// # Errors
/// Rejects unsupported policy, changed inputs, existing foreign membership and
/// unverified ownership. Provider reads retain their independent time limits.
pub fn begin(
    owner: &mut Owner,
    allocation: &bootstrap_initialization::Request,
    request: &Request,
    cancellation: &Cancellation,
) -> Result<Status> {
    request.validate()?;
    cancellation.check().map_err(super::Error::from)?;
    let target = bootstrap_initialization::project_target(owner, allocation, &request.image_digest, cancellation)?;
    begin_with(owner, &target, request, cancellation)
}

pub(in crate::cloud_runtime) fn begin_with(
    owner: &mut Owner,
    target: &bootstrap_recovery::Target,
    request: &Request,
    cancellation: &Cancellation,
) -> Result<Status> {
    cancellation.check().map_err(super::Error::from)?;
    request.validate()?;
    let mut request = request.clone();
    request.repository = request.repository.canonicalize()?;
    request.validate()?;
    let binding = bootstrap_recovery::require_existing(owner, target)?;
    let mut registry = Registry::load(owner)?;
    if let Some(saved) = registry.find(&request.project) {
        if saved.request != request || saved.binding != binding {
            return Err(Error::Invalid);
        }
        return status(owner, &request.project);
    }
    let journal = Journal::load(owner)?;
    registry.require_new(&request, journal.as_ref())?;
    if journal
        .as_ref()
        .is_some_and(|saved| saved.binding != binding || saved.image_digest != request.image_digest)
    {
        return Err(Error::Invalid);
    }
    let runner = super::command::Runner {
        cancel: cancellation,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let revision = super::repository::resolve_with_runner(&request.repository, &request.selection, &runner)?;
    let sessions = request
        .agents
        .iter()
        .map(|agent| Session::new(*agent, revision.clone()))
        .collect();
    let project = request.project.clone();
    registry.intents.push(Intent {
        request,
        binding,
        revision,
        sessions,
    });
    capacity::require(&registry, owner, journal.as_ref())?;
    cancellation.check().map_err(super::Error::from)?;
    registry.save(owner)?;
    status(owner, &project)
}

/// Read anchored progress only. Inspect sessions separately for current runtime state.
/// # Errors
/// Missing, changed or contradictory intent/history and unrelated pending work fail closed.
pub fn status(owner: &Owner, project: &ProjectIdentity) -> Result<Status> {
    let registry = Registry::load(owner)?;
    let intent = registry.find(project).ok_or(Error::Missing)?;
    progress::inspect(intent, Journal::load(owner)?.as_ref())
}

/// Advance one existing signed operation. Source import keeps its two-stage transfer.
/// Caller interruption retains state; an interrupted export can remain blocked.
/// A deadline prevents new SSH sends after export, but cannot interrupt synchronous
/// durability work or shorten the existing source/provider operation limits.
/// # Errors
/// Never adopts unrelated pending work, recreates uncertain exports or relaunches
/// stopped/exited sessions. No provider allocation or deletion is performed.
pub fn advance(
    owner: &mut Owner,
    allocation: &bootstrap_initialization::Request,
    project: &ProjectIdentity,
    cancellation: &Cancellation,
    timeout: Duration,
) -> Result<Status> {
    let deadline = Instant::now() + timeout.min(Source::CONTROLLER_TIMEOUT);
    advance_with(
        owner,
        project,
        cancellation,
        deadline,
        &mut |owner, project, repository, revision| {
            reservations::source::prepare(owner, project, repository, revision, cancellation)
        },
        &mut |owner, change, remaining| {
            reservations::execute(owner, allocation, change, cancellation, remaining)?;
            Ok(())
        },
    )
}

pub(in crate::cloud_runtime) fn advance_with(
    owner: &mut Owner,
    project: &ProjectIdentity,
    cancellation: &Cancellation,
    deadline: Instant,
    prepare: &mut impl FnMut(&mut Owner, &ProjectIdentity, &Path, &str) -> reservations::Result<Source>,
    execute: &mut impl FnMut(&mut Owner, &Change, Duration) -> reservations::Result<()>,
) -> Result<Status> {
    remaining(cancellation, deadline)?;
    let registry = Registry::load(owner)?;
    let intent = registry.find(project).ok_or(Error::Missing)?;
    let before = status(owner, project)?;
    if matches!(before.next, Step::Complete | Step::Terminal) {
        return Ok(before);
    }
    if before.next == Step::ImportSource && !before.pending {
        prepare(owner, project, &intent.request.repository, &intent.revision)?;
    }
    // Export may consume its own bounded allowance. Never restart the caller's
    // deadline afterward, nor send after cancellation during artifact publication.
    remaining(cancellation, deadline)?;
    if Registry::load(owner)? != registry || status(owner, project)? != before {
        return Err(Error::Invalid);
    }
    let journal = Journal::load(owner)?;
    let change = if before.pending {
        Change::Resume
    } else {
        progress::change(intent, &before.next, journal.as_ref())?
    };
    execute(owner, &change, remaining(cancellation, deadline)?)?;
    status(owner, project)
}
fn remaining(cancellation: &Cancellation, deadline: Instant) -> Result<Duration> {
    cancellation.check().map_err(super::Error::from)?;
    let duration = deadline.saturating_duration_since(Instant::now());
    if duration.is_zero() {
        Err(Error::Deadline)
    } else {
        Ok(duration)
    }
}
