//! Bounded comparison facts carried only by the original held root control stream.
//! Decoding these values supplies neither channel custody nor owner approval.
use super::{EXPOSURE_LEASE_SECONDS, ExposureMode, ExposureRequest};
use crate::OperationId;
use serde::{Deserialize, Serialize};
mod members;
pub use super::Route;
pub use members::{MAX_MEMBERS, MemberFacts};
mod worker;
pub use worker::{WorkerIdentity, WorkerObservation};

pub const VERSION: u32 = 3;
pub const MAX_REQUEST_BYTES: usize = 20 * 1024;
pub const MAX_REPLY_BYTES: usize = 144 * 1024;
pub const MAX_SERVICES: usize = 8;
pub const MAX_MUTATIONS: usize = 4096;
pub const MAX_LIFETIME_MILLIS: u64 = EXPOSURE_LEASE_SECONDS * 1000;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub version: u32,
    pub allocation: String,
    pub epoch: OperationId,
    pub challenge: OperationId,
}

impl Session {
    /// # Errors
    /// Refuses another wire version or an invalid original allocation identifier.
    pub fn validate(&self) -> Result<(), Error> {
        if self.version != VERSION || !horizon_cloud::valid_id(&self.allocation) {
            return Err(Error::Session);
        }
        Ok(())
    }
}

/// Worker elapsed-time facts, never a desktop wall clock or an absolute deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lifetime {
    pub anchor_millis: u64,
    pub remaining_millis: u64,
}

impl Lifetime {
    /// # Errors
    /// Refuses an empty, excessive or overflowing interval. The keeper must also
    /// compare the anchor with its original observations and monotonic clock.
    pub fn deadline_millis(self) -> Result<u64, Error> {
        if self.remaining_millis == 0 || self.remaining_millis > MAX_LIFETIME_MILLIS {
            return Err(Error::Lifetime);
        }
        self.anchor_millis
            .checked_add(self.remaining_millis)
            .ok_or(Error::Lifetime)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Open {
        service: OperationId,
        policy: ExposureRequest,
        #[serde(default)]
        route: Route,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        members: Option<MemberFacts>,
        lifetime: Lifetime,
    },
    Renew {
        service: OperationId,
        lifetime: Lifetime,
    },
    Close {
        service: OperationId,
    },
    Read,
    Finish,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub session: Session,
    pub custody: OperationId,
    pub id: OperationId,
    pub command: Command,
}

impl Request {
    /// # Errors
    /// Refuses oversized or malformed frames, unsupported fields and invalid facts.
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(Error::Size);
        }
        let request: Self = serde_json::from_slice(bytes).map_err(|_| Error::Encoding)?;
        request.validate()?;
        Ok(request)
    }

    /// # Errors
    /// Refuses invalid session, policy or interval facts. A valid request cannot
    /// recreate a keeper or prove that an explicit owner decision occurred.
    pub fn validate(&self) -> Result<(), Error> {
        self.session.validate()?;
        match &self.command {
            Command::Open {
                policy,
                route,
                members,
                lifetime,
                ..
            } => {
                validate_policy(policy)?;
                route.validate_for(policy)?;
                match (policy.mode, route, members) {
                    (ExposureMode::TailnetOnly, Route::Gate {}, Some(members)) => members.validate()?,
                    (ExposureMode::TailnetOnly, _, _) | (_, _, Some(_)) => return Err(Error::Members),
                    _ => (),
                }
                lifetime.deadline_millis()?;
            }
            Command::Renew { lifetime, .. } => {
                lifetime.deadline_millis()?;
            }
            Command::Close { .. } | Command::Read | Command::Finish => {}
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Active,
    Expired,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub service: OperationId,
    /// Last service-specific mutation. Keeper Finish has its own Reply.id and
    /// must not replace all entries with that same command identity.
    pub operation: OperationId,
    pub policy: ExposureRequest,
    #[serde(default)]
    pub route: Route,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub ingress: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailnet_ingress: Option<u16>,
    pub remaining_millis: u64,
    pub phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ready,
    Applied,
    Closed,
    BindingRefused,
    PolicyRefused,
    Uncertain,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub session: Session,
    pub custody: OperationId,
    pub worker: WorkerObservation,
    pub id: Option<OperationId>,
    pub outcome: Outcome,
    pub elapsed_millis: u64,
    pub entries: Vec<Entry>,
}

impl Reply {
    /// # Errors
    /// Refuses oversized, malformed or inconsistent observations. Successful
    /// decoding is not a listener, child-reap or provider-release receipt.
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_REPLY_BYTES {
            return Err(Error::Size);
        }
        let reply: Self = serde_json::from_slice(bytes).map_err(|_| Error::Encoding)?;
        reply.validate()?;
        Ok(reply)
    }

    /// # Errors
    /// Refuses duplicate identities/ingress, invalid deadlines or inconsistent
    /// Ready/Closed outcomes. The live owner must still match its exact request.
    pub fn validate(&self) -> Result<(), Error> {
        self.session.validate()?;
        self.worker.validate()?;
        if matches!(self.outcome, Outcome::Ready | Outcome::Applied)
            && matches!(self.worker, WorkerObservation::Unverified { .. })
        {
            return Err(Error::Worker);
        }
        if (self.outcome == Outcome::Ready) != self.id.is_none()
            || (self.outcome == Outcome::Ready && !self.entries.is_empty())
            || self.entries.len() > MAX_SERVICES
        {
            return Err(Error::Entries);
        }
        for (index, entry) in self.entries.iter().enumerate() {
            validate_policy(&entry.policy)?;
            entry.route.validate_for(&entry.policy)?;
            let expected_url = match &self.worker {
                WorkerObservation::Running { identity } | WorkerObservation::Unverified { identity } => entry
                    .route
                    .observed_url(&identity.full_dns_name, entry.tailnet_ingress)?,
                WorkerObservation::Unavailable {} => match entry.route {
                    Route::Gate {} if entry.tailnet_ingress.is_none() => None,
                    Route::Gate {} | Route::FunnelHttps { .. } => return Err(Error::Worker),
                },
            };
            let topology = match entry.policy.mode {
                super::ExposureMode::TailnetOnly => entry.tailnet_ingress == Some(entry.ingress),
                super::ExposureMode::Restricted => entry.tailnet_ingress != Some(entry.ingress),
                super::ExposureMode::Off | super::ExposureMode::Public => entry.tailnet_ingress.is_none(),
            };
            if !topology
                || !entry.route.accepts_ingress(entry.ingress)
                || entry.url != expected_url
                || self.entries[..index].iter().any(|old| {
                    old.service == entry.service
                        || old.operation == entry.operation
                        || old.ingress == entry.ingress
                        || entry
                            .tailnet_ingress
                            .is_some_and(|port| old.tailnet_ingress == Some(port))
                })
                || (entry.phase == Phase::Active) != (entry.remaining_millis > 0)
                || entry.remaining_millis > MAX_LIFETIME_MILLIS
                || self.elapsed_millis.checked_add(entry.remaining_millis).is_none()
                || (self.outcome == Outcome::Closed && entry.phase != Phase::Closed)
            {
                return Err(Error::Entries);
            }
        }
        Ok(())
    }
}

fn validate_policy(policy: &ExposureRequest) -> Result<(), Error> {
    policy.validate()?;
    if policy.mode == ExposureMode::Off {
        return Err(Error::PolicyOff);
    }
    if serde_json::to_vec(policy).map_err(|_| Error::Encoding)?.len() > super::MAX_REQUEST_BYTES {
        return Err(Error::Size);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Exposure control frame exceeds its byte bound")]
    Size,
    #[error("Invalid exposure control JSON")]
    Encoding,
    #[error("Exposure control session does not match the supported contract")]
    Session,
    #[error("Invalid exposure control lifetime")]
    Lifetime,
    #[error("Invalid exposure control observations")]
    Entries,
    #[error("Invalid private workspace member comparison facts")]
    Members,
    #[error("Invalid exposure worker identity observation")]
    Worker,
    #[error("Off cannot open or describe an exposure")]
    PolicyOff,
    #[error(transparent)]
    Policy(#[from] super::Error),
}

#[cfg(test)]
mod tests;
