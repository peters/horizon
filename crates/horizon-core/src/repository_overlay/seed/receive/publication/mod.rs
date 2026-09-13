//! Explicit pack publication policies, not setup or source approval.

mod linux;
mod named;

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

/// Acknowledged named publication, or a complete existing pack reverified and
/// synchronized without renaming. An unused incoming pack is never removed.
#[derive(Debug)]
pub enum NamedPackPublication {
    Published(PublishedGitPack),
    Existing {
        pack: ReceivedGitPack,
        unused_incoming: Option<ReceivedGitPack>,
    },
}

/// Named slots are permanent claims, not reusable locks. Every variant retains
/// its data; no failure, lost reply or dropped receipt grants cleanup or replay.
#[derive(thiserror::Error)]
pub enum NamedPackPublicationFailure {
    #[error("named pack input retained without publication acknowledgement: {reason}")]
    Retained {
        reason: PackPublicationError,
        pack: Box<ReceivedGitPack>,
        /// None only when the requested destination exceeded the path bound.
        destination: Option<PathBuf>,
    },
    #[error("named pack claim outcome is uncertain; no write ownership was acquired")]
    ClaimUnconfirmed {
        pack: Box<ReceivedGitPack>,
        destination: PathBuf,
    },
    #[error("named pack rename outcome is uncertain; retain and inspect both names")]
    RenameUnconfirmed {
        pack: Box<ReceivedGitPack>,
        destination: PathBuf,
    },
    #[error("named pack was renamed but final confirmation is incomplete: {reason}")]
    PublishedUnsynchronized {
        reason: PackPublicationError,
        pack: Box<PublishedGitPack>,
    },
}

impl fmt::Debug for NamedPackPublicationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

/// Publish to `<root>/<encoded-pack-sha256>/pack` using one exclusive permanent
/// directory claim and ordinary rename only inside that fresh claim. Root must
/// already be private and owned. Input must be its direct child, or exactly the
/// complete named destination for explicit reacknowledgement. Existing slots
/// never authorize renaming; only identical complete packs can be resynchronized.
///
/// Requires Linux confinement, stable exclusive input/ancestry, unchanged mounts,
/// trusted Git/prlimit and filesystem mkdir/rename/fsync support. This explicit
/// policy does not relax the qualified sibling publisher or receiver. Success
/// acknowledges bounded verification/synchronization, not power-loss survival,
/// cloud storage qualification, source approval or a complete checkpoint. No
/// protection from hostile same-user mutation or filesystem latency is claimed.
/// Run off the UI thread; no copy fallback, repair, task or provider action occurs.
/// # Errors
/// Invalid, changed, partial or foreign state, cancellation and storage failures
/// retain the precise input/destination phase. Drop never deletes any data.
pub fn publish_named_git_pack(
    root: &Path,
    pack: ReceivedGitPack,
    limits: PackReceiveLimits,
    cancelled: impl Fn() -> bool,
) -> Result<NamedPackPublication, NamedPackPublicationFailure> {
    named::publish(
        root,
        pack,
        limits,
        &cancelled,
        &mut |_, file| file.sync_all().map_err(|_| PackPublicationError::Storage),
        &mut named::claim,
        &mut named::rename,
    )
}

#[cfg(test)]
mod tests;
