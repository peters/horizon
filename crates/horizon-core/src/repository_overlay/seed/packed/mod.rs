//! Bounded raw objects from an explicitly authorized, stable Linux object store.

mod process;
mod protocol;
mod view;

use super::{GitObjectSource, GitObjectStream, SeedError, SeedFailure, staging};
use git2::Oid;
use std::{
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};

/// Native decoding can materialize delta objects; these limits may reject valid packs.
/// CPU time accumulates over the complete session. Pipe deadlines exclude blocking
/// filesystem calls, process creation and kill/reap latency.
#[derive(Clone, Copy, Debug)]
pub struct PackedSourceLimits {
    pub address_space_bytes: u64,
    pub cpu_seconds: u32,
    pub object_timeout: Duration,
}

impl Default for PackedSourceLimits {
    fn default() -> Self {
        Self {
            address_space_bytes: 1024 * 1024 * 1024,
            cpu_seconds: 60,
            object_timeout: Duration::from_secs(30),
        }
    }
}

impl PackedSourceLimits {
    fn validate(self) -> Result<(), SeedError> {
        if !(64 * 1024 * 1024..=16 * 1024 * 1024 * 1024).contains(&self.address_space_bytes)
            || !(1..=3600).contains(&self.cpu_seconds)
            || self.object_timeout.is_zero()
            || self.object_timeout > Duration::from_secs(300)
        {
            return Err(SeedError::Limit);
        }
        Ok(())
    }
}

/// A fresh isolated metadata view and one owned, resource-limited Git child.
/// Only full object IDs are requested; no source config, refs, filters, hooks or
/// recursive alternates are inherited. The importer still verifies destination OIDs.
/// Drop terminates only this child and retains the private metadata directory.
pub struct PackedGitObjectSource<'a> {
    session: process::Session<'a>,
    metadata: PathBuf,
}

impl fmt::Debug for PackedGitObjectSource<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PackedGitObjectSource").finish_non_exhaustive()
    }
}

impl<'a> PackedGitObjectSource<'a> {
    /// The caller authorizes the object store and keeps its topology/content and
    /// ancestry stable, with the private scratch parent exclusively controlled.
    /// No-follow validation is not confinement against concurrent same-user changes.
    /// Requires trusted `/usr/bin/git` (batch-command support) and `/usr/bin/prlimit`.
    /// No installation, fallback, source mutation or export authority is provided.
    /// # Errors
    /// Reject unsafe topology, invalid bounds, missing tools, cancellation or storage
    /// failure. A failed private metadata directory is reported and never auto-deleted.
    pub fn new(
        parent: &Path,
        objects: &Path,
        limits: PackedSourceLimits,
        cancelled: impl Fn() -> bool + 'a,
    ) -> Result<Self, SeedFailure> {
        let preflight = limits.validate().and_then(|()| view::validate(objects, &cancelled));
        preflight.map_err(|reason| SeedFailure { reason, residue: None })?;
        let metadata = staging::reserve(parent, &cancelled).map_err(|reason| SeedFailure { reason, residue: None })?;
        let result = view::command(&metadata, objects, limits)
            .and_then(|command| process::Session::spawn(command, limits.object_timeout, Box::new(cancelled)));
        match result {
            Ok(session) => Ok(Self { session, metadata }),
            Err(reason) => Err(SeedFailure {
                reason,
                residue: Some(metadata),
            }),
        }
    }

    /// Retained scratch metadata, not a checkout or a cleanup authorization.
    #[must_use]
    pub fn metadata_path(&self) -> &Path {
        &self.metadata
    }
}

impl GitObjectSource for PackedGitObjectSource<'_> {
    fn open(&mut self, object: Oid) -> Result<GitObjectStream<'_>, SeedError> {
        let header = match self.session.info(object) {
            Ok(header) => header,
            Err(error) => {
                self.session.poison();
                return Err(error);
            }
        };
        Ok(GitObjectStream {
            kind: header.kind,
            bytes: header.bytes,
            reader: Box::new(protocol::ObjectReader::new(&mut self.session, header)),
        })
    }
}

#[cfg(test)]
mod tests;
