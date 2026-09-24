//! Read-only pre-admission observations, never permission to attach or delete.
use crate::{OperationId, bootstrap::Startup};
use horizon_cloud::Capabilities;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub capabilities: Capabilities,
}

/// Authenticated by the retained SSH host key. The operation and fingerprint
/// bind this observation to one signed request; admission must inspect again
/// under its own allocation lock before reserving any project resources.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub version: u32,
    pub startup: Startup,
    pub worker_id: String,
    pub operation: OperationId,
    pub fingerprint: [u8; 32],
    pub revision: u64,
    pub capabilities: Capabilities,
}
