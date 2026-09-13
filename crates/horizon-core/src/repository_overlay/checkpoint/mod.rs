//! One-shot base closure plus selected dirty layers, not a full workspace checkpoint.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod storage;

use crate::{
    cloud_run::{ArtifactDigest, GitCommitSha},
    repository_git::GitPreparation,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt, path::PathBuf};

pub const REQUEST_LIMIT: usize = 64 * 1024;
pub const CAPACITY: u64 = 512 * 1024 * 1024;
pub const ADMISSION_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointRequest {
    pub version: u8,
    pub preparation: GitPreparation,
    pub selected: Vec<String>,
    pub complete_base_closure_consent: bool,
    pub retained_volume_attested: bool,
    pub parent: PathBuf,
    pub attempt_name: String,
    pub max_retained_bytes: u64,
}

impl CheckpointRequest {
    /// Validate explicit coverage/retention consent and bounded request shape without I/O.
    /// # Errors
    /// Rejects unsupported versions, identities, paths, selections and budgets.
    pub fn validate(&self) -> Result<(), CheckpointError> {
        use super::{
            OverlayChange, OverlayContent, checkout::publication::validate_sibling_name, materialize::valid_path,
        };
        if self.version != 1
            || !self.complete_base_closure_consent
            || !self.retained_volume_attested
            || self.preparation.binding().is_err()
            || self.selected.is_empty()
            || self.selected.len() > 128
            || !(ADMISSION_BYTES..=CAPACITY).contains(&self.max_retained_bytes)
            || !valid_path(&self.parent)
            || validate_sibling_name(&self.attempt_name).is_err()
            || self.parent.join(&self.attempt_name).as_os_str().len() > super::seed::MAX_PACK_RECEIVE_PARENT_BYTES
        {
            return Err(CheckpointError::Invalid);
        }
        let mut unique = BTreeSet::new();
        for path in &self.selected {
            OverlayChange::new(path.clone(), OverlayContent::Remove).map_err(|_| CheckpointError::Invalid)?;
            if !unique.insert(path) {
                return Err(CheckpointError::Invalid);
            }
        }
        if serde_json::to_vec(self).map_err(|_| CheckpointError::Invalid)?.len() > REQUEST_LIMIT {
            return Err(CheckpointError::Invalid);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackIdentity {
    pub base_commit: GitCommitSha,
    pub sha256: ArtifactDigest,
    pub encoded_bytes: u64,
}

#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationManifest {
    pub version: u8,
    pub coverage: String,
    pub preparation: GitPreparation,
    pub complete_base_closure_consent: bool,
    pub retained_volume_attested: bool,
    pub selected: Vec<String>,
    pub pack: PackIdentity,
    pub overlay_manifest: ArtifactDigest,
    pub overlay_record: ArtifactDigest,
    pub overlay_bytes: u64,
    pub started_at_millis: u64,
    pub verified_at_millis: u64,
}

#[derive(Serialize)]
pub struct CheckpointGeneration {
    pub path: PathBuf,
    pub manifest_sha256: ArtifactDigest,
    pub manifest: GenerationManifest,
}
impl fmt::Debug for CheckpointGeneration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckpointGeneration").finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointError {
    #[error("invalid checkpoint request or consent")]
    Invalid,
    #[error("checkpoint capture is unsupported")]
    Unsupported,
    #[error("checkpoint identity changed or is unsafe")]
    Identity,
    #[error("checkpoint source changed during capture")]
    Changed,
    #[error("checkpoint exceeds a conservative capacity bound")]
    Capacity,
    #[error("checkpoint storage is unconfirmed; retain the attempt")]
    Storage,
    #[error("checkpoint capture was cancelled")]
    Cancelled,
}

#[derive(thiserror::Error)]
#[error("{reason}")]
pub struct CheckpointFailure {
    pub reason: CheckpointError,
    /// Attempted locator only; separately prove ownership before any future action.
    pub retained: Option<PathBuf>,
}
impl fmt::Debug for CheckpointFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(f)
    }
}

/// Capture one explicitly consented generation. Existing attempts are never adopted.
/// Stable exclusive ownership is required; blocking filesystem calls have no hard
/// deadline. No cloud, task, cleanup, retry, restore or remote watermark mutation.
/// # Errors
/// All post-claim failures retain an uncertain locator, not an ownership assertion.
pub fn checkpoint_once(
    request: &CheckpointRequest,
    cancelled: impl Fn() -> bool,
) -> Result<CheckpointGeneration, CheckpointFailure> {
    request
        .validate()
        .map_err(|reason| CheckpointFailure { reason, retained: None })?;
    #[cfg(target_os = "linux")]
    {
        let before = request.preparation.inspect_checkout().map_err(|_| CheckpointFailure {
            reason: CheckpointError::Identity,
            retained: None,
        })?;
        linux::run(
            request,
            std::path::Path::new(before.path),
            &|| {
                if request.preparation.inspect_checkout().ok().as_ref() == Some(&before) {
                    Ok(())
                } else {
                    Err(CheckpointError::Identity)
                }
            },
            &cancelled,
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cancelled;
        Err(CheckpointFailure {
            reason: CheckpointError::Unsupported,
            retained: None,
        })
    }
}

#[cfg(test)]
mod tests;
