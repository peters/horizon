//! Local private SSH identities are independent of client views and session files.
//! They are not credentials for repository access and grant no provider authority.

#[cfg(target_os = "linux")]
mod command;
#[cfg(target_os = "linux")]
mod linux;

use crate::{
    HorizonHome,
    cloud_run::{CloudJobId, CloudWorkflowId},
};
use std::path::{Path, PathBuf};

/// A private filesystem location, never a serializable workspace snapshot.
pub struct RemoteSshIdentityStore {
    home: PathBuf,
}

/// Only the public key may be copied into the durable allocation request.
pub struct RemoteSshIdentity {
    private_key_path: PathBuf,
    public_key: String,
}

impl RemoteSshIdentity {
    #[must_use]
    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    /// Pass only as a local identity-file argument to the pinned SSH transport.
    #[must_use]
    pub fn private_key_path(&self) -> &Path {
        &self.private_key_path
    }
}

impl std::fmt::Debug for RemoteSshIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("RemoteSshIdentity").finish_non_exhaustive()
    }
}

impl RemoteSshIdentityStore {
    #[must_use]
    pub fn new(home: &HorizonHome) -> Self {
        Self {
            home: home.root().to_path_buf(),
        }
    }

    /// Read-only path checks before another store writes beneath this home.
    /// A missing home is allowed only below existing trusted ancestors. This does
    /// not bind an inode or protect against same-user concurrent path replacement.
    pub(crate) fn validate_home(&self) -> Result<(), RemoteSshIdentityError> {
        #[cfg(target_os = "linux")]
        {
            linux::validate_home(&self.home)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(RemoteSshIdentityError::UnsupportedPlatform)
        }
    }

    /// Retain a candidate for a new, unclaimed allocation before reserving its public key.
    /// Reuses an interrupted candidate for these exact IDs; never overwrites a key.
    /// This operation must not be used to recover an already reserved/claimed allocation.
    /// Run off the render thread. No key is removed on handle/store drop or client exit.
    /// # Errors
    /// Rejects insecure paths, unavailable key generation, storage failures and unsupported platforms.
    pub fn prepare_new(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
    ) -> Result<RemoteSshIdentity, RemoteSshIdentityError> {
        #[cfg(target_os = "linux")]
        {
            linux::prepare(&self.home, workflow_id, job_id)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (&self.home, workflow_id, job_id);
            Err(RemoteSshIdentityError::UnsupportedPlatform)
        }
    }

    /// Load only the existing private identity matching the allocation's saved public key.
    /// Missing, corrupt or mismatched keys fail closed; recovery never creates a replacement.
    /// It performs no provider operations and does not authorize attachment by itself.
    /// # Errors
    /// Rejects missing/mismatched keys, insecure paths, key inspection failures and unsupported platforms.
    pub fn recover(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        expected_public_key: &str,
    ) -> Result<RemoteSshIdentity, RemoteSshIdentityError> {
        #[cfg(target_os = "linux")]
        {
            linux::recover(&self.home, workflow_id, job_id, expected_public_key)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (&self.home, workflow_id, job_id, expected_public_key);
            Err(RemoteSshIdentityError::UnsupportedPlatform)
        }
    }
}

/// Errors deliberately omit key bytes, paths, subprocess output and command arguments.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum RemoteSshIdentityError {
    #[error("remote private SSH identity is missing; no replacement was generated")]
    Missing,
    #[error("remote private SSH identity does not match the saved public request")]
    Mismatch,
    #[error("remote SSH identity path is not a private regular file or directory")]
    InsecurePath,
    #[error("remote SSH identity storage operation failed")]
    Storage,
    #[error("remote SSH identity is invalid")]
    InvalidIdentity,
    #[error("OpenSSH key utility is unavailable")]
    KeyUtilityUnavailable,
    #[error("OpenSSH key operation failed")]
    KeyUtilityFailed,
    #[error("OpenSSH key operation exceeded its deadline")]
    Deadline,
    #[error("protected remote SSH identity storage is not yet supported on this platform")]
    UnsupportedPlatform,
}

#[cfg(test)]
mod tests;
