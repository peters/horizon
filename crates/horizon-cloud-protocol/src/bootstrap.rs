//! Bootstrap recovery wire data, not permission to initialize storage.
use crate::{OperationId, SharingMode, signed::ControllerBinding};
use serde::{Deserialize, Serialize};

/// Immutable startup context, supplied separately from management requests.
/// A matching context is not evidence that a mounted volume is fresh.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Startup {
    pub version: u32,
    pub controller: ControllerBinding,
    pub token: OperationId,
    pub sharing: SharingMode,
    pub worker_operation: String,
    pub volume_id: String,
    pub data_center_id: String,
}

impl Startup {
    /// # Errors
    /// Rejects unsupported versions and invalid provider identities.
    pub fn validate(&self) -> Result<(), crate::BindingError> {
        if self.version != 1
            || ![&self.worker_operation, &self.volume_id, &self.data_center_id]
                .into_iter()
                .all(|id| horizon_cloud::valid_id(id))
        {
            return Err(crate::BindingError::Encoding);
        }
        Ok(())
    }
}

/// Exact UTF-8 strings preserve the bytes authenticated by `SignedIntent`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRequest {
    pub message: String,
    pub payload: String,
}

/// There is deliberately no initialize action in this entry point.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryPayload {
    Recover { token: OperationId },
}

/// Receipt for recovery of the pre-admission, revision-zero manifest only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryReceipt {
    pub version: u32,
    pub startup: Startup,
    pub worker_id: String,
    pub operation: OperationId,
    pub fingerprint: [u8; 32],
}

/// Only the verified direct-create host flow may sign Initialize. Parsing this
/// payload or a retained provider receipt does not create that permission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum BootstrapPayload {
    Initialize {
        startup: Startup,
        worker_id: String,
        host_key: String,
    },
    Abandon {
        startup: Startup,
        worker_id: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapOutcome {
    Initializing,
    Abandoned,
}

/// First publication and terminal pre-admission fence receipts. Initialization
/// alone never reports admission readiness; the anchored Recover must complete.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapReceipt {
    pub version: u32,
    pub startup: Startup,
    pub worker_id: String,
    pub operation: OperationId,
    pub fingerprint: [u8; 32],
    pub outcome: BootstrapOutcome,
}
