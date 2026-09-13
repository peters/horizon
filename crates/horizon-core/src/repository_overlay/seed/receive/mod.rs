//! Bounded private pack receipt, independent of source approval and setup admission.

mod named;
mod native;
mod observe;
pub mod publication;

pub use named::receive_named_git_base_pack;
pub use observe::observe_git_base_pack;

use super::{
    MAX_OBJECTS, SeedError, SeedFailure,
    export::{PackExportLimits, PreparedGitPack, output, source_error},
    packed::{PackedSourceLimits, view},
    staging,
};
use crate::cloud_run::ArtifactDigest;
use git2::Oid;
use std::{
    fmt,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

/// Expected identity, not source-export approval or a trust assertion about input bytes.
#[derive(Clone, Copy)]
pub struct ExpectedGitPack<'a> {
    pub base_commit: Oid,
    pub sha256: &'a ArtifactDigest,
    pub encoded_bytes: u64,
}

impl<'a> From<&'a PreparedGitPack> for ExpectedGitPack<'a> {
    fn from(pack: &'a PreparedGitPack) -> Self {
        Self {
            base_commit: pack.base_commit(),
            sha256: pack.sha256(),
            encoded_bytes: pack.encoded_bytes(),
        }
    }
}

/// Encoded input ceiling and per-child decoding/enumeration limits. Blocking reader
/// and filesystem calls remain the caller's latency responsibility.
#[derive(Clone, Copy, Debug)]
pub struct PackReceiveLimits {
    pub source: PackedSourceLimits,
    pub encoded_bytes: u64,
}

impl Default for PackReceiveLimits {
    fn default() -> Self {
        Self {
            source: PackedSourceLimits::default(),
            encoded_bytes: PackExportLimits::DEFAULT_ENCODED_BYTES,
        }
    }
}

impl PackReceiveLimits {
    fn validate(self, expected: ExpectedGitPack<'_>) -> Result<(), SeedError> {
        self.source.validate()?;
        if !(32..=PackExportLimits::MAX_ENCODED_BYTES).contains(&self.encoded_bytes)
            || !(32..=self.encoded_bytes).contains(&expected.encoded_bytes)
        {
            return Err(SeedError::Limit);
        }
        if expected.base_commit.as_bytes().len() != 20 || expected.base_commit.is_zero() {
            return Err(SeedError::Object);
        }
        Ok(())
    }
}

/// Strictly decoded private exact-base pack, not a policy-verified namespace, durable
/// publication or ready checkout. Keep its path and ancestry stable/exclusive until
/// existing namespace/seed/setup verification consumes it. Drop never removes data.
pub struct ReceivedGitPack {
    path: PathBuf,
    objects_directory: PathBuf,
    base_commit: Oid,
    sha256: ArtifactDigest,
    encoded_bytes: u64,
    objects: u32,
}

impl ReceivedGitPack {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    #[must_use]
    pub fn objects_directory(&self) -> &Path {
        &self.objects_directory
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
    #[must_use]
    pub fn objects(&self) -> u32 {
        self.objects
    }
}

impl fmt::Debug for ReceivedGitPack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReceivedGitPack")
            .field("encoded_bytes", &self.encoded_bytes)
            .field("objects", &self.objects)
            .finish_non_exhaustive()
    }
}

impl<'a> From<&'a ReceivedGitPack> for ExpectedGitPack<'a> {
    fn from(pack: &'a ReceivedGitPack) -> Self {
        Self {
            base_commit: pack.base_commit(),
            sha256: pack.sha256(),
            encoded_bytes: pack.encoded_bytes(),
        }
    }
}

/// Receive one explicit standard SHA-1 pack in fresh private scratch. Verify exact
/// length/EOF/SHA-256 before trusted resource-limited Git decodes it. An isolated
/// shallow traversal must account for every packed object from the expected base;
/// extra history/unreachable objects and incomplete/thin packs are rejected.
/// Source selection, namespace policy, seed preparation and setup admission remain
/// separate. No network/configuration inheritance, publication, sync acknowledgement,
/// task start, retry or cleanup occurs. Requires stable exclusive scratch ancestry
/// and trusted `/usr/bin/git` and `/usr/bin/prlimit`; run off the UI thread.
/// Cancellation is checked between reader chunks and native operations. Blocking
/// reader/storage calls, spawn and kill/reap are outside native pipe deadlines.
/// # Errors
/// Invalid metadata/limits and unsafe parents fail before reservation. All later
/// failures retain the whole private reservation, including partial files, as
/// unconfirmed. No failure or lost receipt authorizes replay or deletion.
pub fn receive_git_base_pack(
    parent: &Path,
    expected: ExpectedGitPack<'_>,
    input: &mut impl Read,
    limits: PackReceiveLimits,
    cancelled: impl Fn() -> bool,
) -> Result<ReceivedGitPack, SeedFailure> {
    limits
        .validate(expected)
        .and_then(|()| staging::check_cancel(&cancelled))
        .map_err(|reason| SeedFailure { reason, residue: None })?;
    let path = staging::reserve(parent, &cancelled).map_err(|reason| SeedFailure { reason, residue: None })?;
    receive(&path, expected, input, limits, &cancelled, &mut native::relocate).map_err(|reason| SeedFailure {
        reason,
        residue: Some(path),
    })
}

fn receive(
    path: &Path,
    expected: ExpectedGitPack<'_>,
    input: &mut impl Read,
    limits: PackReceiveLimits,
    cancelled: &impl Fn() -> bool,
    relocate: &mut impl FnMut(&Path, &str) -> Result<(), SeedError>,
) -> Result<ReceivedGitPack, SeedError> {
    let decoded = path.join("decoded");
    let selection = path.join("selection");
    for directory in [&decoded, &selection] {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(directory)
            .map_err(|_| SeedError::Storage)?;
    }
    let command = view::index_command(&decoded, expected.base_commit, limits.source, expected.encoded_bytes)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(decoded.join("objects/pack/received.pack"))
        .map_err(|_| SeedError::Storage)?;
    let summary = output::copy(
        &mut |bytes| {
            if cancelled() {
                return Err(io::Error::from(io::ErrorKind::ConnectionAborted));
            }
            input.read(bytes)
        },
        &mut file,
        expected.encoded_bytes,
        MAX_OBJECTS,
    )?;
    if summary.bytes != expected.encoded_bytes || &summary.sha256 != expected.sha256 {
        return Err(SeedError::Object);
    }
    file.flush().map_err(|_| SeedError::Storage)?;
    let hash = native::index(command, &decoded, summary.objects, limits.source, cancelled)?;
    relocate(&decoded, &hash)?;
    let objects_directory = decoded.join("objects");
    native::closure(
        &selection,
        &objects_directory,
        expected.base_commit,
        summary.objects,
        limits.source,
        cancelled,
    )?;
    Ok(ReceivedGitPack {
        path: path.to_path_buf(),
        objects_directory,
        base_commit: expected.base_commit,
        sha256: summary.sha256,
        encoded_bytes: summary.bytes,
        objects: summary.objects,
    })
}

#[cfg(test)]
mod tests;
