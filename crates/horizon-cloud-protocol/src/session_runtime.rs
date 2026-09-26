//! Fresh process observations are separate from historical mutation receipts.
use crate::{OperationId, ProjectIdentity, bootstrap::Startup, membership::SessionId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub session_id: SessionId,
    /// The signed request's expected revision is the base. A pending host
    /// mutation may already have committed remotely despite a lost reply.
    pub pending: Option<Pending>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    pub operation: OperationId,
    pub revision: u64,
    pub fingerprint: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Status {
    NotStarted,
    Launching,
    Running,
    Exited { code: Option<i32> },
    Stopping,
    Stopped,
    Uncertain,
}

// Empty struct variants keep the internally tagged decoder strict; Serde's
// unit variants otherwise accept and discard additional fields.
impl<'de> Deserialize<'de> for Status {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
        enum Strict {
            NotStarted {},
            Launching {},
            Running {},
            Exited { code: Option<i32> },
            Stopping {},
            Stopped {},
            Uncertain {},
        }
        Ok(match Strict::deserialize(deserializer)? {
            Strict::NotStarted {} => Self::NotStarted,
            Strict::Launching {} => Self::Launching,
            Strict::Running {} => Self::Running,
            Strict::Exited { code } => Self::Exited { code },
            Strict::Stopping {} => Self::Stopping,
            Strict::Stopped {} => Self::Stopped,
            Strict::Uncertain {} => Self::Uncertain,
        })
    }
}

/// The pinned SSH connection authenticates this bounded response. Running
/// describes a process, not successful agent authentication or project admission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub version: u32,
    pub startup: Startup,
    pub worker_id: String,
    pub project: ProjectIdentity,
    pub session_id: SessionId,
    pub operation: OperationId,
    pub fingerprint: [u8; 32],
    pub revision: u64,
    pub launch: Option<OperationId>,
    pub status: Status,
}

#[cfg(test)]
mod tests {
    use super::Status;
    #[test]
    fn status_rejects_unrecognized_fields_in_every_state() {
        for status in [
            Status::NotStarted,
            Status::Launching,
            Status::Running,
            Status::Exited { code: Some(17) },
            Status::Stopping,
            Status::Stopped,
            Status::Uncertain,
        ] {
            let mut value = serde_json::to_value(&status).unwrap();
            assert_eq!(serde_json::from_value::<Status>(value.clone()).unwrap(), status);
            value.as_object_mut().unwrap().insert("unexpected".into(), true.into());
            assert!(serde_json::from_value::<Status>(value).is_err());
        }
    }
}
