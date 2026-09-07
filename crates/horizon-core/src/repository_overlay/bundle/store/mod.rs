//! Explicit private bundle persistence, not capture, export approval or scheduled remote backup.

#[cfg(target_os = "linux")]
mod linux;

use super::{RepositoryOverlayBundle, codec};
use crate::{cloud_run::ArtifactDigest, repository_overlay::reader::RepositoryReadError};
use std::{fmt, path::Path};

/// Immutable digest-named records in an explicitly selected, existing private directory.
/// The caller owns storage selection, authorization, capacity and retention policy.
/// No overwrite, deletion, directory creation, inventory or permission repair is exposed.
pub struct RepositoryBundleStore {
    #[cfg(target_os = "linux")]
    directory: linux::Directory,
}

impl RepositoryBundleStore {
    /// Pin an owned `0700` directory without following symlinked ancestors.
    /// Linux confinement and anonymous-file publication support are required for writes.
    /// # Errors
    /// Rejects unsupported platforms, inaccessible/unsafe roots and non-private ownership.
    pub fn open(root: &Path) -> Result<Self, BundleStoreError> {
        #[cfg(target_os = "linux")]
        {
            Ok(Self {
                directory: linux::Directory::open(root)?,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = root;
            Err(BundleStoreError::Unsupported)
        }
    }

    /// Synchronize and atomically publish complete bytes, never replacing an existing record.
    /// Identical retries verify and synchronize the existing file and directory again.
    /// A failure can leave a complete published record; retry explicitly with the same bundle.
    /// Run off the UI thread: bounded buffers do not bound filesystem latency. Encoding plus
    /// a retry comparison can hold two encoding buffers alongside the caller's bundle.
    /// # Errors
    /// Rejects unsupported storage, unsafe/conflicting existing records or synchronization failure.
    pub fn put(&self, bundle: &RepositoryOverlayBundle) -> Result<ArtifactDigest, BundleStoreError> {
        #[cfg(target_os = "linux")]
        {
            let bytes = codec::encode(bundle)?;
            self.directory.put(bundle.manifest_sha256(), &bytes)?;
            Ok(bundle.manifest_sha256().clone())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = bundle;
            Err(BundleStoreError::Unsupported)
        }
    }

    /// Read one private bounded record and revalidate its encoding, payloads and exact digest.
    /// Does not establish a signature, coherent capture, storage replication or apply authority.
    /// Run off the UI thread; decoding owns a separate bounded payload copy.
    /// # Errors
    /// Rejects missing, unsafe, oversized, corrupt or wrongly named records without modifying them.
    pub fn get(&self, digest: &ArtifactDigest) -> Result<RepositoryOverlayBundle, BundleStoreError> {
        #[cfg(target_os = "linux")]
        {
            let record = self.directory.read(digest)?.ok_or(BundleStoreError::Missing)?;
            let bundle = codec::decode(&record.bytes)?;
            if bundle.manifest_sha256() != digest {
                return Err(BundleStoreError::DigestMismatch);
            }
            Ok(bundle)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = digest;
            Err(BundleStoreError::Unsupported)
        }
    }
}

impl fmt::Debug for RepositoryBundleStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("RepositoryBundleStore").finish_non_exhaustive()
    }
}

/// Storage failures contain no paths, identities, digests or bundle contents.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum BundleStoreError {
    #[error("safe bundle storage is unsupported on this platform or filesystem")]
    Unsupported,
    #[error("bundle storage requires an existing private owned directory")]
    UnsafeDirectory,
    #[error("requested repository bundle is missing")]
    Missing,
    #[error("existing repository bundle conflicts with the requested contents")]
    Conflict,
    #[error("stored repository bundle does not match its requested digest")]
    DigestMismatch,
    #[error("repository bundle could not be durably stored")]
    WriteFailed,
    #[error(transparent)]
    Read(#[from] RepositoryReadError),
    #[error(transparent)]
    Codec(#[from] codec::OverlayCodecError),
}

#[cfg(test)]
mod tests;
