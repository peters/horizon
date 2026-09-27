//! Ephemeral attachment authorization and retained, pinned SSH material.
use super::{Cancellation, Error, Journal, Owner, Result, Snapshot, bootstrap_initialization, bootstrap_recovery};
use horizon_cloud_protocol::{
    OperationId, ProjectIdentity,
    bootstrap::RecoveryRequest,
    membership::{Request as Mutation, SessionId, State},
    session_attachment::Request,
    signed::{Action, Intent, Target},
};

/// A private, nonserializable selection. Transport creation revalidates the owning
/// journal; it never clears a pending mutation or obtains launch authority.
pub struct SessionAttachment {
    journal: Journal,
    project: ProjectIdentity,
    session: SessionId,
    launch: OperationId,
}
/// Keep this value alive until the SSH process exits: it owns private copies of
/// the verified key and host pin. Drop the allocation Owner before waiting on
/// the interactive process so status/stop can acquire the owning-host lock.
/// Do not log arguments or persist them in panel state.
pub struct SessionTransport {
    arguments: Vec<String>,
    _snapshot: Snapshot,
}
impl SessionTransport {
    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }
}
/// Select an existing launched session without contacting or changing a worker.
/// # Errors
/// Rejects missing, pending, stopped, unsupported or unprepared reservations.
pub fn prepare_attachment(owner: &Owner, project: &ProjectIdentity, session: SessionId) -> Result<SessionAttachment> {
    let journal = Journal::load(owner)?.ok_or(Error::Missing)?;
    if journal.pending.is_some() {
        return Err(Error::Pending);
    }
    let member = journal
        .manifest
        .members
        .iter()
        .find(|m| &m.identity == project)
        .ok_or(Error::Invalid)?;
    if member.state != State::Importing
        || !member.preparations.contains(&session)
        || !member.launches.contains(&session)
        || member.stops.contains(&session)
        || member.capabilities.desktop
        || member.capabilities.browser_tools()
        || !member.ports.is_empty()
        || !member
            .sessions
            .iter()
            .any(|s| s.id == session && s.agent == horizon_cloud::Agent::Claude)
    {
        return Err(Error::Invalid);
    }
    let launch = journal
        .manifest
        .operations
        .iter()
        .find(|entry| {
            &entry.receipt.identity == project
                && serde_json::from_str::<Mutation>(&entry.payload)
                    .is_ok_and(|r| r == (Mutation::StartSession { session_id: session }))
        })
        .map(|entry| entry.receipt.operation)
        .ok_or(Error::Invalid)?;
    Ok(SessionAttachment {
        journal,
        project: project.clone(),
        session,
        launch,
    })
}
impl SessionAttachment {
    /// Construct a fresh transport immediately before handing it to a PTY launcher.
    /// A disconnect detaches only; reconnect requires a fresh descriptor and never
    /// starts an agent. Multiple clients share the server's smallest terminal size.
    /// # Errors
    /// Rejects changed owner, journal, provider identity, key material or host pins.
    pub fn transport(
        &self,
        owner: &Owner,
        allocation: &bootstrap_initialization::Request,
        cancellation: &Cancellation,
    ) -> Result<SessionTransport> {
        cancellation.check().map_err(super::super::Error::from)?;
        if Journal::load(owner)?.as_ref() != Some(&self.journal) {
            return Err(Error::Invalid);
        }
        let target =
            bootstrap_initialization::project_target(owner, allocation, &self.journal.image_digest, cancellation)?;
        self.transport_with(owner, &target)
    }
    pub(in crate::cloud_runtime) fn transport_with(
        &self,
        owner: &Owner,
        target: &bootstrap_recovery::Target,
    ) -> Result<SessionTransport> {
        if Journal::load(owner)?.as_ref() != Some(&self.journal) {
            return Err(Error::Invalid);
        }
        let snapshot = Snapshot::capture(target)?;
        if snapshot.binding != self.journal.binding
            || bootstrap_recovery::require_existing(owner, target)? != snapshot.binding
        {
            return Err(Error::Invalid);
        }
        let payload = serde_json::to_string(&Request {
            startup: snapshot.binding.startup.clone(),
            worker_id: snapshot.binding.worker_id.clone(),
            session_id: self.session,
            launch: self.launch,
        })
        .map_err(|_| Error::Invalid)?;
        let intent = Intent::new(
            &snapshot.binding.startup.controller,
            OperationId::generate(),
            self.journal.manifest.revision,
            Target::Project {
                identity: self.project.clone(),
            },
            Action::AttachProjectSession,
            payload.as_bytes(),
        )
        .map_err(|_| Error::Invalid)?;
        let message = serde_json::to_string(&owner.sign(intent)?).map_err(|_| Error::Invalid)?;
        let encoded = horizon_cloud_protocol::session_attachment::encode(&RecoveryRequest { message, payload })
            .map_err(|_| Error::Invalid)?;
        if Journal::load(owner)?.as_ref() != Some(&self.journal)
            || bootstrap_recovery::require_existing(owner, target)? != snapshot.binding
        {
            return Err(Error::Invalid);
        }
        Ok(SessionTransport {
            arguments: snapshot.connection.pinned_attachment(&encoded),
            _snapshot: snapshot,
        })
    }
}
