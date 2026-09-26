//! Durable reservation and namespace intent, never permission to run a project.
use crate::{
    OperationId, ProjectIdentity, SharingMode,
    bootstrap::Startup,
    signed::{Action, Intent, SignedIntent, Target},
};
use horizon_cloud::{Agent, Capabilities};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, time::Duration};

pub const MAX_PROJECTS: usize = 32;
pub const MAX_OPERATIONS: usize = 64;
pub const MAX_SESSIONS: usize = 8;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
// Includes the complete encoded mutation, not just its payload.
pub const CANCELLATION_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("Invalid project reservation or retained membership history")]
pub struct Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Attaching,
    Preparing,
    Importing,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Reserve {
        capabilities: Capabilities,
        ports: BTreeSet<u16>,
    },
    PrepareNamespace {},
    ImportSource {
        descriptor: Source,
    },
    ReserveSession {
        session: Session,
    },
    Cancel {},
}

impl Request {
    #[must_use]
    pub const fn action(&self) -> Action {
        match self {
            Self::Reserve { .. } => Action::AttachProject,
            Self::PrepareNamespace {} => Action::ReconcileProject,
            Self::ImportSource { .. } => Action::ImportProjectSource,
            Self::ReserveSession { .. } => Action::ReserveProjectSession,
            Self::Cancel {} => Action::RemoveProject,
        }
    }
}

/// Stable agent identity and initial source selection. Reserving this record does
/// not create a worktree, provision credentials or authorize a process launch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub id: uuid::Uuid,
    pub agent: Agent,
    pub revision: String,
}

impl Session {
    /// Generate once and persist before requesting the corresponding reservation.
    #[must_use]
    pub fn new(agent: Agent, revision: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            agent,
            revision,
        }
    }
}

/// Immutable byte identity of the committed-only transfer. SHA-1 repositories are
/// the currently supported export format; SHA-256 identifies the transport bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub version: u32,
    pub revision: String,
    pub pack: Artifact,
    pub material: Artifact,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub length: u64,
    pub sha256: [u8; 32],
}
impl Source {
    pub const MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
    pub const MAX_REQUEST_BYTES: usize = 65536;
    pub const WORKER_TIMEOUT: Duration = Duration::from_secs(600);
    pub const CONTROLLER_TIMEOUT: Duration = Duration::from_secs(Self::WORKER_TIMEOUT.as_secs() * 2 + 60);
    /// # Errors
    /// Rejects unsupported revisions, formats and transfer sizes.
    pub fn validate(&self) -> Result<(), Error> {
        if self.version != 1
            || self.revision.len() != 40
            || !self
                .revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.pack.length < 32
            || self.material.length < 1024
            || self.pack.length.saturating_add(self.material.length) > Self::MAX_BYTES
        {
            return Err(Error);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub identity: ProjectIdentity,
    pub namespace: String,
    pub capabilities: Capabilities,
    pub ports: BTreeSet<u16>,
    pub state: State,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<Session>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub version: u32,
    pub operation: OperationId,
    pub fingerprint: [u8; 32],
    pub revision: u64,
    pub identity: ProjectIdentity,
    pub state: State,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mutation {
    pub message: String,
    pub payload: String,
    pub receipt: Receipt,
}

/// Empty encoding stays compatible with bootstrap recovery. Retained signatures
/// reconstruct every supported state transition; receipts are never evicted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub startup: Startup,
    pub worker_id: String,
    pub revision: u64,
    pub members: Vec<Member>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<Mutation>,
}

impl Manifest {
    #[must_use]
    pub fn empty(startup: Startup, worker_id: String) -> Self {
        Self {
            version: 1,
            startup,
            worker_id,
            revision: 0,
            members: Vec::new(),
            operations: Vec::new(),
        }
    }

    /// # Errors
    /// Rejects corrupt, unsupported, oversized or unauthenticated retained state.
    pub fn validate(&self) -> Result<(), Error> {
        self.startup.validate().map_err(|_| Error)?;
        if self.version != 1
            || !horizon_cloud::valid_id(&self.worker_id)
            || self.members.len() > MAX_PROJECTS
            || self.operations.len() > MAX_OPERATIONS
            || serde_json::to_vec(self).map_err(|_| Error)?.len() > MAX_MANIFEST_BYTES
        {
            return Err(Error);
        }
        let mut rebuilt = Self::empty(self.startup.clone(), self.worker_id.clone());
        for mutation in &self.operations {
            let receipt = rebuilt.apply(&mutation.message, &mutation.payload)?;
            if receipt != mutation.receipt {
                return Err(Error);
            }
        }
        if rebuilt != *self {
            return Err(Error);
        }
        Ok(())
    }

    /// Return a proposed atomic state and receipt. An unchanged revision denotes
    /// an exact historical retry; it is not a fresh capability observation.
    /// # Errors
    /// Rejects invalid history, changed retries, stale revisions and conflicts.
    pub fn next(&self, message: &str, payload: &str) -> Result<(Self, Receipt), Error> {
        self.validate()?;
        let signed = SignedIntent::parse(message.as_bytes()).map_err(|_| Error)?;
        let intent = signed
            .verify(&self.startup.controller, payload.as_bytes())
            .map_err(|_| Error)?;
        if let Some(saved) = self
            .operations
            .iter()
            .find(|entry| entry.receipt.operation == intent.operation())
        {
            if saved.receipt.fingerprint != intent.fingerprint().map_err(|_| Error)?
                || (saved.receipt.state != State::Removed
                    && self
                        .members
                        .iter()
                        .any(|member| member.identity == saved.receipt.identity && member.state == State::Removed))
            {
                return Err(Error);
            }
            return Ok((self.clone(), saved.receipt.clone()));
        }
        let mut next = self.clone();
        let receipt = next.apply(message, payload)?;
        if serde_json::to_vec(&next).map_err(|_| Error)?.len() > MAX_MANIFEST_BYTES {
            return Err(Error);
        }
        Ok((next, receipt))
    }

    fn apply(&mut self, message: &str, payload: &str) -> Result<Receipt, Error> {
        let signed = SignedIntent::parse(message.as_bytes()).map_err(|_| Error)?;
        let intent = signed
            .verify(&self.startup.controller, payload.as_bytes())
            .map_err(|_| Error)?;
        let request: Request = serde_json::from_str(payload).map_err(|_| Error)?;
        let Target::Project { identity } = intent.target() else {
            return Err(Error);
        };
        if intent.action() != request.action()
            || intent.expected_revision() != self.revision
            || self.operations.len() >= MAX_OPERATIONS
            || self
                .operations
                .iter()
                .any(|entry| entry.receipt.operation == intent.operation())
        {
            return Err(Error);
        }
        let state = match request {
            Request::Reserve { capabilities, ports } => {
                self.reserve(identity, capabilities, ports)?;
                State::Attaching
            }
            Request::PrepareNamespace {} => {
                let member = self
                    .members
                    .iter_mut()
                    .find(|member| &member.identity == identity)
                    .ok_or(Error)?;
                if member.state != State::Attaching {
                    return Err(Error);
                }
                member.state = State::Preparing;
                State::Preparing
            }
            Request::ImportSource { descriptor } => {
                descriptor.validate()?;
                let member = self
                    .members
                    .iter_mut()
                    .find(|member| &member.identity == identity)
                    .ok_or(Error)?;
                if member.state != State::Preparing {
                    return Err(Error);
                }
                member.state = State::Importing;
                State::Importing
            }
            Request::ReserveSession { session } => {
                self.reserve_session(identity, session)?;
                State::Importing
            }
            Request::Cancel {} => {
                let member = self
                    .members
                    .iter_mut()
                    .find(|member| &member.identity == identity)
                    .ok_or(Error)?;
                if member.state == State::Removed {
                    return Err(Error);
                }
                member.state = State::Removed;
                State::Removed
            }
        };
        self.revision = self.revision.checked_add(1).ok_or(Error)?;
        let receipt = self.receipt(intent, identity, state)?;
        let mutation = Mutation {
            message: serde_json::to_string(&signed).map_err(|_| Error)?,
            payload: payload.into(),
            receipt: receipt.clone(),
        };
        if state == State::Removed && serde_json::to_vec(&mutation).map_err(|_| Error)?.len() + 1 > CANCELLATION_BYTES {
            return Err(Error);
        }
        self.operations.push(mutation);
        let live = self
            .members
            .iter()
            .filter(|member| member.state != State::Removed)
            .count();
        // Keep room for every live reservation's cancellation even at capacity.
        if self.operations.len() + live > MAX_OPERATIONS
            || serde_json::to_vec(self).map_err(|_| Error)?.len() + live * CANCELLATION_BYTES > MAX_MANIFEST_BYTES
        {
            return Err(Error);
        }
        Ok(receipt)
    }

    fn receipt(&self, intent: &Intent, identity: &ProjectIdentity, state: State) -> Result<Receipt, Error> {
        Ok(Receipt {
            version: 1,
            operation: intent.operation(),
            fingerprint: intent.fingerprint().map_err(|_| Error)?,
            revision: self.revision,
            identity: identity.clone(),
            state,
        })
    }

    fn reserve_session(&mut self, identity: &ProjectIdentity, session: Session) -> Result<(), Error> {
        if session.id.is_nil()
            || self.members.iter().any(|member| member.sessions.iter().any(|saved| saved.id == session.id))
            || !self.operations.iter().any(|entry| {
                &entry.receipt.identity == identity
                    && serde_json::from_str::<Request>(&entry.payload).is_ok_and(|request| {
                        matches!(request, Request::ImportSource { descriptor } if descriptor.revision == session.revision)
                    })
            })
        {
            return Err(Error);
        }
        let member = self
            .members
            .iter_mut()
            .find(|member| &member.identity == identity)
            .ok_or(Error)?;
        if member.state != State::Importing
            || member.sessions.len() >= MAX_SESSIONS
            || !member.capabilities.agents.contains(&session.agent)
        {
            return Err(Error);
        }
        member.sessions.push(session);
        Ok(())
    }

    fn reserve(
        &mut self,
        identity: &ProjectIdentity,
        capabilities: Capabilities,
        ports: BTreeSet<u16>,
    ) -> Result<(), Error> {
        capabilities.validate().map_err(|_| Error)?;
        if self.members.len() >= MAX_PROJECTS
            || (self.startup.sharing == SharingMode::Dedicated && !self.members.is_empty())
            || self.members.iter().any(|member| member.identity.project_id() == identity.project_id()
                || member.identity.cloud_id() == identity.cloud_id())
            || ports.len() > 16
            // SSH, X11, VNC and the worker control endpoint cannot be app grants.
            || ports.iter().any(|port| *port < 1024 || (5900..=6099).contains(port) || *port == 47280)
            || capabilities.browserstack.as_ref().is_some_and(|remote| !remote.local_ports.is_subset(&ports))
            || self.members.iter().filter(|member| member.state != State::Removed).any(|member|
                !member.ports.is_disjoint(&ports) || (member.capabilities.desktop && capabilities.desktop))
        {
            return Err(Error);
        }
        self.members.push(Member {
            identity: identity.clone(),
            namespace: format!("project-{}", identity.project_id()),
            capabilities,
            ports,
            state: State::Attaching,
            sessions: Vec::new(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AllocationId, ControllerId, ProjectId, signed::ControllerBinding};
    use ring::{
        rand::SystemRandom,
        signature::{Ed25519KeyPair, KeyPair},
    };

    #[test]
    fn cancellation_budget_covers_maximum_identity_and_signature_encoding() {
        let key =
            Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap().as_ref()).unwrap();
        let binding = ControllerBinding::new(
            AllocationId::generate(),
            ControllerId::generate(),
            key.public_key().as_ref().try_into().unwrap(),
        );
        let identity =
            ProjectIdentity::new(ProjectId::generate(), "s".repeat(100), "w".repeat(100), "c".repeat(100)).unwrap();
        let operation = OperationId::generate();
        let payload = serde_json::to_string(&Request::Cancel {}).unwrap();
        let intent = Intent::new(
            &binding,
            operation,
            u64::MAX,
            Target::Project {
                identity: identity.clone(),
            },
            Action::RemoveProject,
            payload.as_bytes(),
        )
        .unwrap();
        let mut wire = serde_json::to_value(SignedIntent::sign(intent, &binding, &key).unwrap()).unwrap();
        // Force maximum decimal byte lengths; only the size of the encoding is
        // under test here, not authorization of this intentionally invalid signature.
        wire["signature"] = serde_json::json!(vec![255; 64]);
        wire["intent"]["payload_hash"] = serde_json::json!(vec![255; 32]);
        let encoded = serde_json::to_vec(&wire).unwrap();
        let message = serde_json::to_string(&SignedIntent::parse(&encoded).unwrap()).unwrap();
        let mutation = Mutation {
            message,
            payload,
            receipt: Receipt {
                version: 1,
                operation,
                fingerprint: [255; 32],
                revision: u64::MAX,
                identity,
                state: State::Removed,
            },
        };
        assert!(serde_json::to_vec(&mutation).unwrap().len() + 1 < CANCELLATION_BYTES);
    }
}
