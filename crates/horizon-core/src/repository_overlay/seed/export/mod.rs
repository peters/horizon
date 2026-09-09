//! Explicit local exact-base pack preparation, not remote transfer or publication.

pub(super) mod output;

use super::{
    MAX_BYTES, PreparedGitSeed, SeedError, SeedFailure,
    packed::{PackedSourceLimits, process::Session, view},
    staging,
};
use crate::cloud_run::ArtifactDigest;
use git2::Oid;
use std::{
    fmt,
    fs::OpenOptions,
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

/// Native child limits plus a separate packed-output ceiling. The source timeout
/// bounds the entire pipe operation, excluding blocking storage/spawn/reap calls.
#[derive(Clone, Copy, Debug)]
pub struct PackExportLimits {
    pub source: PackedSourceLimits,
    pub encoded_bytes: u64,
}

impl PackExportLimits {
    pub const DEFAULT_ENCODED_BYTES: u64 = super::DEFAULT_ENCODED_PACK_BYTES;
    pub const MAX_ENCODED_BYTES: u64 = MAX_BYTES * 2;

    fn validate(self) -> Result<(), SeedError> {
        self.source.validate()?;
        if !(32..=Self::MAX_ENCODED_BYTES).contains(&self.encoded_bytes) {
            return Err(SeedError::Limit);
        }
        Ok(())
    }
}

impl Default for PackExportLimits {
    fn default() -> Self {
        Self {
            source: PackedSourceLimits::default(),
            encoded_bytes: Self::DEFAULT_ENCODED_BYTES,
        }
    }
}

/// Finished trusted-producer output in retained private scratch, not an immutable
/// store record, synchronized publication or export authorization. Keep the file
/// and its ancestry stable/exclusive until a later consumer revalidates its digest.
pub struct PreparedGitPack {
    path: PathBuf,
    base_commit: Oid,
    sha256: ArtifactDigest,
    encoded_bytes: u64,
}

impl PreparedGitPack {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn base_commit(&self) -> Oid {
        self.base_commit
    }

    #[must_use]
    pub fn sha256(&self) -> &ArtifactDigest {
        &self.sha256
    }

    #[must_use]
    pub fn encoded_bytes(&self) -> u64 {
        self.encoded_bytes
    }
}

impl fmt::Debug for PreparedGitPack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedGitPack")
            .field("encoded_bytes", &self.encoded_bytes)
            .finish_non_exhaustive()
    }
}

/// Prepare one non-thin standard Git pack from an already verified private seed.
/// The caller explicitly authorizes its complete base closure, including removed
/// files and raw commit metadata, and keeps seed/ancestry stable and exclusively
/// controlled. No original repository configuration, refs, history, hooks, filters
/// or recursive alternates are inherited. Exact shallow metadata is synthesized.
/// Requires trusted `/usr/bin/git` and `/usr/bin/prlimit`; run off the UI thread.
/// No network, provider, setup/task start, source mutation, retry or cleanup occurs.
/// A failed partial pack remains in the reported scratch directory, unconfirmed.
/// Success checks framing/size, trusted child success and file flush, not an
/// independent pack decode or crash durability. The recipient must still validate
/// the expected closure, shallow boundary and digest before admitting setup.
/// # Errors
/// Rejects invalid limits, unsafe/overlapping source and output ancestry, cancellation,
/// child failure, oversized/truncated output and storage failure, retaining residue.
pub fn prepare_git_base_pack(
    parent: &Path,
    seed: &PreparedGitSeed,
    limits: PackExportLimits,
    cancelled: impl Fn() -> bool,
) -> Result<PreparedGitPack, SeedFailure> {
    let objects = seed.path().join(".git/objects");
    let preflight = limits.validate().and_then(|()| {
        // Reject output inside any seed node, not merely inside its object store.
        view::validate(seed.path(), parent, &cancelled)?;
        view::validate(&objects, parent, &cancelled)
    });
    preflight.map_err(|reason| SeedFailure { reason, residue: None })?;
    let metadata = staging::reserve(parent, &cancelled).map_err(|reason| SeedFailure { reason, residue: None })?;
    produce(&metadata, &objects, seed, limits, &cancelled).map_err(|reason| SeedFailure {
        reason,
        residue: Some(metadata),
    })
}

fn produce(
    metadata: &Path,
    objects: &Path,
    seed: &PreparedGitSeed,
    limits: PackExportLimits,
    cancelled: &impl Fn() -> bool,
) -> Result<PreparedGitPack, SeedError> {
    let command = view::command(
        metadata,
        objects,
        limits.source,
        view::Operation::Pack(seed.base_commit()),
    )?;
    let path = metadata.join("base.pack");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| SeedError::Storage)?;
    let mut session = Session::spawn(command, limits.source.object_timeout, Box::new(cancelled))?;
    session
        .begin_commit(seed.base_commit())
        .map_err(|error| source_error(error.kind()))?;
    let summary = output::copy(
        &mut |bytes| session.read(bytes),
        &mut file,
        limits.encoded_bytes,
        seed.imported_objects(),
    )?;
    session.finish().map_err(|error| source_error(error.kind()))?;
    file.flush().map_err(|_| SeedError::Storage)?;
    Ok(PreparedGitPack {
        path,
        base_commit: seed.base_commit(),
        sha256: summary.sha256,
        encoded_bytes: summary.bytes,
    })
}

pub(super) fn source_error(error: io::ErrorKind) -> SeedError {
    if error == io::ErrorKind::ConnectionAborted {
        SeedError::Cancelled
    } else {
        SeedError::Source
    }
}

#[cfg(test)]
mod tests;
