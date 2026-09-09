//! Explicit no-replace sibling publication; no task, transfer or cleanup authority.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod walk;

use super::PreparedPrivateCheckout;
use crate::cloud_run::ArtifactDigest;
use crate::repository_overlay::paths;
use git2::Oid;
use std::{
    fmt,
    path::{Path, PathBuf},
};

/// The rename succeeded. The result containing this receipt states whether the
/// subsequent synchronization was acknowledged. Dropping it never removes data.
#[derive(Debug)]
pub struct PublishedCheckout(PreparedPrivateCheckout);

impl PublishedCheckout {
    #[must_use]
    pub fn path(&self) -> &Path {
        self.0.path()
    }

    #[must_use]
    pub fn base_commit(&self) -> Oid {
        self.0.base_commit()
    }

    #[must_use]
    pub fn manifest_sha256(&self) -> &ArtifactDigest {
        self.0.manifest_sha256()
    }
}

/// Pre-rename, confirmed-rename and uncertain-rename failures are different states.
/// No receipt authorizes deletion; an uncertain rename requires inspecting both names.
#[derive(thiserror::Error)]
pub enum PublicationFailure {
    #[error("checkout remains unpublished: {reason}")]
    Unpublished {
        reason: PublicationError,
        checkout: PreparedPrivateCheckout,
    },
    #[error("checkout was published but synchronization is unconfirmed: {reason}")]
    PublishedUnsynchronized {
        reason: PublicationError,
        checkout: PublishedCheckout,
    },
    #[error("rename outcome is uncertain; retain and inspect both names")]
    RenameUnconfirmed {
        checkout: PreparedPrivateCheckout,
        destination: PathBuf,
    },
}

impl fmt::Debug for PublicationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PublicationError {
    #[error("checkout publication is unsupported on this platform or filesystem")]
    Unsupported,
    #[error("publication requires a fresh single-component sibling name")]
    InvalidName,
    #[error("publication destination already exists")]
    DestinationExists,
    #[error("checkout identity or node is unsafe or changed")]
    UnsafeNode,
    #[error("checkout exceeds a publication resource bound")]
    Limit,
    #[error("checkout storage synchronization or rename failed")]
    Storage,
    #[error("checkout publication was cancelled")]
    Cancelled,
}

/// Maximum UTF-8 bytes in one portable publication destination component.
pub const MAX_SIBLING_NAME_BYTES: usize = 255;

/// Validate one bounded portable destination component without accessing storage.
/// # Errors
/// Rejects unsafe or unsupported names. Success does not prove a fresh destination,
/// ownership, storage qualification or permission to publish.
pub fn validate_sibling_name(sibling: &str) -> Result<(), PublicationError> {
    if sibling.len() > MAX_SIBLING_NAME_BYTES || sibling.contains('/') || paths::validate(sibling).is_err() {
        Err(PublicationError::InvalidName)
    } else {
        Ok(())
    }
}

/// Synchronize a prepared tree and rename it to an explicitly named fresh sibling.
/// The caller must keep the tree unchanged, ancestry stable and private parent under
/// exclusive same-user control throughout preparation and publication. This is not
/// confinement against concurrent same-user/privileged mutation or an export grant.
/// Linux requires journaled ext4 with barriers, verified through bounded kernel
/// metadata. Trusted proc/sysfs and unchanged mount configuration are prerequisites.
/// No remote/overlay/nojournal filesystem, global sync or copy fallback is used.
/// A successful result acknowledges file/directory synchronization on healthy storage;
/// it cannot prove power-loss survival or exclude previously unobserved writeback errors.
/// Run off the UI thread: cancellation is checked between calls, not during fsync.
/// # Errors
/// Pre-rename failures retain the private receipt. Once rename succeeds, every later
/// failure/cancellation returns `PublishedUnsynchronized` with the destination receipt.
/// An uncertain rename error returns `RenameUnconfirmed`; inspect both names, never
/// assume the stage stayed in place or retry/clean up blindly.
/// Nothing is rolled back, overwritten, auto-deleted or started, including on Drop.
pub fn publish_sibling_checkout(
    checkout: PreparedPrivateCheckout,
    sibling: &str,
    cancelled: impl Fn() -> bool,
) -> Result<PublishedCheckout, PublicationFailure> {
    #[cfg(target_os = "linux")]
    {
        linux::publish(
            checkout,
            sibling,
            &cancelled,
            &mut |_, file| file.sync_all().map_err(|_| PublicationError::Storage),
            &mut linux::rename,
            &linux::supported_storage,
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (sibling, cancelled);
        Err(PublicationFailure::Unpublished {
            reason: PublicationError::Unsupported,
            checkout,
        })
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
