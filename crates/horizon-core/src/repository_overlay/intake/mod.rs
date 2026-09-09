//! Explicit retained repository intake, never export approval, setup or task authority.

#[cfg(target_os = "linux")]
mod linux;

use super::bundle::codec;
use crate::cloud_run::interactive_worker::valid_single_token;
use crate::{
    cloud_run::{ArtifactDigest, CloudJobId, CloudWorkflowId, GitSource},
    remote_workspace::valid_local_id,
};
use serde::{Deserialize, Serialize};
use std::{fmt, io::Read, path::PathBuf};

pub const REQUEST_LIMIT: usize = 32 * 1024;
pub const RESPONSE_LIMIT: usize = 128 * 1024;
pub const PACK_LIMIT: u64 = super::seed::DEFAULT_ENCODED_PACK_BYTES;

/// Bounded immutable recovery identity. Constructing/deserializing it grants no writes.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntakeRequest {
    pub version: u8,
    pub workspace_local_id: String,
    pub workflow_id: CloudWorkflowId,
    pub job_id: CloudJobId,
    pub runtime_generation: u64,
    pub worker_resource_id: String,
    pub client_key_sha256: ArtifactDigest,
    pub source: GitSource,
    pub pack: EncodedIdentity,
    pub overlay: EncodedIdentity,
}

/// Pack digest or the existing complete two-layer bundle manifest, with wire length.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncodedIdentity {
    pub sha256: ArtifactDigest,
    pub encoded_bytes: u64,
}

impl IntakeRequest {
    /// Canonical bounded bytes suitable for retaining BEFORE an explicitly approved send.
    /// # Errors
    /// Rejects malformed identities, source or limits without filesystem access.
    pub fn encode(&self) -> Result<Vec<u8>, IntakeError> {
        let valid = self.version == 1
            && valid_local_id(&self.workspace_local_id)
            && self.workflow_id.to_string() != uuid::Uuid::nil().to_string()
            && self.job_id.to_string() != uuid::Uuid::nil().to_string()
            && self.runtime_generation > 0
            && valid_single_token(&self.worker_resource_id, 512)
            && self.source.validate().is_ok()
            && self.source.commit.as_str().bytes().any(|byte| byte != b'0')
            && (32..=PACK_LIMIT).contains(&self.pack.encoded_bytes)
            && (1..=codec::MAX_ENCODED_BUNDLE_BYTES as u64).contains(&self.overlay.encoded_bytes);
        if !valid {
            return Err(IntakeError::Invalid);
        }
        serde_json::to_vec(self)
            .ok()
            .filter(|bytes| bytes.len() <= REQUEST_LIMIT)
            .ok_or(IntakeError::Invalid)
    }

    /// Decode a strict request, not an approval or permission to recreate missing state.
    /// # Errors
    /// Rejects invalid JSON/identities, duplicate/unknown fields and excessive length.
    pub fn decode(bytes: &[u8]) -> Result<Self, IntakeError> {
        if bytes.len() > REQUEST_LIMIT {
            return Err(IntakeError::Invalid);
        }
        let request: Self = serde_json::from_slice(bytes).map_err(|_| IntakeError::Invalid)?;
        request.encode()?;
        Ok(request)
    }
}

impl fmt::Debug for IntakeRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntakeRequest").finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntakeState {
    Acknowledged,
    Observed,
    ClaimedUnknown,
    Rejected,
    Unconfirmed,
    Error,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackState {
    Acknowledged,
    Observed,
    ReceiveUnconfirmed,
    Unpublished,
    PublishedUnsynchronized,
    RenameUnconfirmed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleState {
    Acknowledged,
    Observed,
    WriteUnconfirmed,
}

#[derive(Clone, Serialize)]
pub struct IntakeRoots {
    pub packs: PathBuf,
    pub bundles: PathBuf,
    pub setup: PathBuf,
}

#[derive(Serialize)]
pub struct PackProgress {
    pub state: PackState,
    pub source: Option<PathBuf>,
    pub destination: Option<PathBuf>,
    pub objects: Option<u32>,
}

/// Known progress survives failure. These paths are historical candidates, not cleanup grants.
#[derive(Serialize)]
pub struct IntakeResponse {
    pub version: u8,
    pub state: IntakeState,
    pub intent_sha256: Option<ArtifactDigest>,
    pub roots: Option<IntakeRoots>,
    pub pack: Option<PackProgress>,
    pub bundle: Option<BundleState>,
    pub reason: Option<IntakeError>,
}

impl IntakeResponse {
    #[must_use]
    pub const fn failure(reason: IntakeError) -> Self {
        Self {
            version: 1,
            state: match reason {
                IntakeError::Invalid => IntakeState::Rejected,
                IntakeError::Unsupported => IntakeState::Unsupported,
                _ => IntakeState::Error,
            },
            intent_sha256: None,
            roots: None,
            pack: None,
            bundle: None,
            reason: Some(reason),
        }
    }

    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        match self.state {
            IntakeState::Acknowledged | IntakeState::Observed => 0,
            IntakeState::Rejected => 2,
            IntakeState::ClaimedUnknown => 4,
            _ => 1,
        }
    }
}

impl fmt::Debug for IntakeResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntakeResponse")
            .field("state", &self.state)
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum IntakeError {
    #[error("invalid repository intake identity or framing")]
    Invalid,
    #[error("repository intake requires supported qualified storage")]
    Unsupported,
    #[error("repository intake storage is unsafe, changed or unavailable")]
    Storage,
    #[error("repository intake claim conflicts with this request")]
    Conflict,
    #[error("repository intake was cancelled; retain state and do not replay")]
    Cancelled,
    #[error("repository intake input could not be verified")]
    Input,
}

/// Explicit worker mutation at its fixed existing retained parent. The caller separately
/// owns export, storage/capacity and pinned transport authority. No setup/task is started.
/// Existing claims only observe, without reading payload or repairing missing children.
/// Cancellation/Drop never removes state; blocking input/storage has no hard deadline.
#[must_use]
pub fn receive(request: &IntakeRequest, input: &mut impl Read, cancelled: impl Fn() -> bool) -> IntakeResponse {
    execute(request, Some(input), &cancelled)
}

/// Read-only observation of the same claim and inputs. Never synchronizes or repairs.
#[must_use]
pub fn observe(request: &IntakeRequest, cancelled: impl Fn() -> bool) -> IntakeResponse {
    execute(request, None, &cancelled)
}

fn execute(request: &IntakeRequest, input: Option<&mut dyn Read>, cancelled: &dyn Fn() -> bool) -> IntakeResponse {
    if request.encode().is_err() {
        return IntakeResponse::failure(IntakeError::Invalid);
    }
    #[cfg(target_os = "linux")]
    {
        linux::execute(
            std::path::Path::new("/workspace/.horizon-worker"),
            request,
            input,
            cancelled,
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (input, cancelled);
        IntakeResponse::failure(IntakeError::Unsupported)
    }
}

#[cfg(test)]
mod tests;
