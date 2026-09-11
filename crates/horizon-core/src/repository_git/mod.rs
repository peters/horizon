//! One-shot ordinary Git preparation on an already trusted worker, not overlay setup.
//! Receipts attest initial preparation only; observation never resets user changes.

#[cfg(target_os = "linux")]
mod git;
#[cfg(target_os = "linux")]
mod linux;

use crate::{cloud_run::GitSource, remote_workspace::valid_local_id};
use serde::{Deserialize, Serialize};

pub const REQUEST_LIMIT: usize = 16 * 1024;
pub const RESPONSE_LIMIT: usize = 1024;
pub const CHECKOUT: &str = "/workspace/horizon/repository";

/// Explicit, credential-free preparation identity, not provider ownership evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitPreparation {
    pub version: u8,
    pub workspace_local_id: String,
    pub runtime_id: uuid::Uuid,
    pub source: GitSource,
    pub work_branch: String,
}

impl GitPreparation {
    /// Canonical task binding, independent of current checkout contents or existence.
    /// # Errors
    /// Rejects invalid preparation identities before any filesystem access.
    pub fn binding(&self) -> Result<crate::cloud_run::ArtifactDigest, GitPreparationError> {
        let mut bytes = b"horizon-ordinary-git-task-v1\0".to_vec();
        bytes.extend(self.encode()?);
        Ok(crate::cloud_run::ArtifactDigest::sha256(&bytes))
    }

    /// Inspect the completed checkout identity without Git, writes or task startup.
    /// Requires stable trusted worker ownership; callers must hold and recheck the
    /// returned directory identity before use. Dirty files are deliberately retained.
    /// # Errors
    /// Rejects incomplete/conflicting preparation, unsafe ancestry and replaced roots.
    pub fn inspect_checkout(&self) -> Result<GitCheckoutLocation, GitPreparationError> {
        self.encode()?;
        #[cfg(target_os = "linux")]
        {
            linux::inspect_checkout(std::path::Path::new("/workspace"), self)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(GitPreparationError::Unsupported)
        }
    }

    /// Decode bounded strict JSON before touching the worker.
    /// # Errors
    /// Rejects malformed identities, source, branches, unknown fields and trailing data.
    pub fn decode(bytes: &[u8]) -> Result<Self, GitPreparationError> {
        if bytes.len() > REQUEST_LIMIT {
            return Err(GitPreparationError::Invalid);
        }
        let request: Self = serde_json::from_slice(bytes).map_err(|_| GitPreparationError::Invalid)?;
        request.encode()?;
        Ok(request)
    }

    fn encode(&self) -> Result<Vec<u8>, GitPreparationError> {
        let mut work = self.source.clone();
        work.branch = Some(self.work_branch.clone());
        if self.source.repository.len() > 512
            || self.source.branch.as_ref().is_some_and(|branch| branch.len() > 1024)
            || self.work_branch.len() > 1024
            || self.work_branch == "HEAD"
            || self.version != 1
            || !valid_local_id(&self.workspace_local_id)
            || self.runtime_id.is_nil()
            || self.source.validate().is_err()
            || work.validate().is_err()
            || self.source.commit.as_str().bytes().all(|byte| byte == b'0')
        {
            return Err(GitPreparationError::Invalid);
        }
        serde_json::to_vec(self)
            .ok()
            .filter(|bytes| bytes.len() <= REQUEST_LIMIT)
            .ok_or(GitPreparationError::Invalid)
    }
}

/// Fixed worker checkout plus inode identity, not a caller-supplied path.
#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct GitCheckoutLocation {
    pub path: &'static str,
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitPreparationState {
    Absent,
    ClaimedUnknown,
    Complete,
    Error,
}

/// No raw Git output, credentials or caller-supplied paths enter this response.
#[derive(Debug, Serialize)]
pub struct GitPreparationResponse {
    pub version: u8,
    pub state: GitPreparationState,
    pub reason: Option<GitPreparationError>,
    pub checkout: Option<&'static str>,
}

impl GitPreparationResponse {
    #[must_use]
    pub fn failure(reason: GitPreparationError) -> Self {
        Self {
            version: 1,
            state: GitPreparationState::Error,
            reason: Some(reason),
            checkout: None,
        }
    }

    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match (self.state, self.reason) {
            (GitPreparationState::Complete | GitPreparationState::Absent, None) => 0,
            (_, Some(GitPreparationError::Invalid | GitPreparationError::Unsupported)) => 2,
            _ => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum GitPreparationError {
    #[error("invalid Git preparation request")]
    Invalid,
    #[error("Git preparation requires Linux confinement")]
    Unsupported,
    #[error("worker directory is unsafe or changed")]
    UnsafeRoot,
    #[error("Git preparation state conflicts or cannot be read safely")]
    Conflict,
    #[error("storage operation failed; retain all state")]
    Storage,
    #[error("Git operation failed; retain all state")]
    Git,
    #[error("Git preparation cancelled or exceeded its deadline; retain all state")]
    Interrupted,
    #[error("repository requires unsupported LFS, submodule or custom filter preparation")]
    UnsupportedRepository,
}

/// Prepare once, or observe the existing claim without invoking Git again.
/// Requires trusted stable worker ancestry and exclusive ownership throughout I/O.
/// No overlay filesystem qualification, task start, cleanup or retry is implied.
/// Cancellation/deadlines bound child I/O, not blocked filesystem operations/spawn/reap.
pub fn prepare(request: &GitPreparation, cancelled: impl Fn() -> bool) -> GitPreparationResponse {
    execute(request, false, cancelled)
}

/// Read the original completion receipt only, not current HEAD, cleanliness or readiness.
#[must_use]
pub fn observe(request: &GitPreparation) -> GitPreparationResponse {
    execute(request, true, || false)
}

fn execute(request: &GitPreparation, observe: bool, cancelled: impl Fn() -> bool) -> GitPreparationResponse {
    if let Err(reason) = request.encode() {
        return GitPreparationResponse::failure(reason);
    }
    #[cfg(target_os = "linux")]
    {
        linux::execute(
            std::path::Path::new("/workspace"),
            request,
            observe,
            &cancelled,
            &mut git::Git::new(),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (observe, cancelled);
        GitPreparationResponse::failure(GitPreparationError::Unsupported)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
