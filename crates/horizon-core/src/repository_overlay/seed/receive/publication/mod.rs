//! Explicit qualified no-replace pack publication, not setup or source approval.

mod linux;

use super::{PackReceiveLimits, ReceivedGitPack, SeedError};
use std::{
    fmt,
    path::{Path, PathBuf},
};

/// Rename succeeded; the containing result states whether synchronization and
/// final verification were acknowledged. Drop always retains the named data.
#[derive(Debug)]
pub struct PublishedGitPack(ReceivedGitPack);

impl PublishedGitPack {
    #[must_use]
    pub fn path(&self) -> &Path {
        self.0.path()
    }

    #[must_use]
    pub fn pack(&self) -> &ReceivedGitPack {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PackPublicationError {
    #[error("pack publication requires a fresh single-component sibling name")]
    InvalidName,
    #[error("pack publication destination already exists")]
    DestinationExists,
    #[error("pack publication storage is unsupported")]
    Unsupported,
    #[error("pack publication synchronization or binding verification failed")]
    Storage,
    #[error(transparent)]
    Verification(#[from] SeedError),
}

/// Pre-rename failure, acknowledged rename and uncertain rename are distinct.
/// No error, dropped receipt or lost response authorizes cleanup or replay.
#[derive(thiserror::Error)]
pub enum PackPublicationFailure {
    #[error("pack remains unpublished: {reason}")]
    Unpublished {
        reason: PackPublicationError,
        pack: ReceivedGitPack,
    },
    #[error("pack was renamed but final confirmation is incomplete: {reason}")]
    PublishedUnsynchronized {
        reason: PackPublicationError,
        pack: PublishedGitPack,
    },
    #[error("pack rename outcome is uncertain; retain and inspect both names")]
    RenameUnconfirmed {
        pack: Box<ReceivedGitPack>,
        destination: PathBuf,
    },
}

impl fmt::Debug for PackPublicationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

/// Reverify a received pack, synchronize its fixed private tree and publish to one
/// fresh sibling using no-replace rename. Requires stable exclusive ownership of
/// input/ancestry, unchanged mounts and trusted kernel metadata/Git/prlimit. Linux
/// journaled ext4 with barriers is required; no copy or filesystem fallback occurs.
/// Acknowledgement means bounded file/directory synchronization and binding checks
/// succeeded on healthy storage, not proof of power-loss survival or cloud durability.
/// The input is immutable by protocol, not protected against hostile same-user writes.
/// Run off the UI thread: cancellation cannot interrupt filesystem/native blocking
/// calls. Source approval, namespace/setup checks and task admission remain separate.
/// # Errors
/// Failures preserve the source, destination or both names according to the returned
/// variant. After successful rename, all errors retain the destination receipt; no
/// retry, repair, overwrite, rollback, deletion or task start occurs, including Drop.
pub fn publish_sibling_git_pack(
    pack: ReceivedGitPack,
    sibling: &str,
    limits: PackReceiveLimits,
    cancelled: impl Fn() -> bool,
) -> Result<PublishedGitPack, PackPublicationFailure> {
    linux::publish(
        pack,
        sibling,
        limits,
        &cancelled,
        &mut |_, file| file.sync_all().map_err(|_| PackPublicationError::Storage),
        &mut linux::rename,
        &linux::supported_storage,
    )
}

#[cfg(test)]
mod tests;
