//! Complete, hash-verified local payloads; not export approval, capture or filesystem safety.

mod fingerprint;

use super::{OverlayContent, RepositoryOverlayPlan, reader::MAX_READ_BYTES};
use crate::cloud_run::ArtifactDigest;
use std::{collections::BTreeMap, fmt};

/// Aggregate distinct regular-file payload bound for this in-memory boundary.
pub const MAX_BUNDLE_BYTES: usize = 128 * 1024 * 1024;

/// Locally hashed, bounded file bytes. Hash equality does not establish content secrecy or trust.
#[derive(Eq, PartialEq)]
pub struct VerifiedOverlayBlob {
    sha256: ArtifactDigest,
    bytes: Box<[u8]>,
}

impl VerifiedOverlayBlob {
    /// Hash an owned payload without cloning it, discarding spare allocation capacity.
    /// Run hashing off the UI thread.
    /// # Errors
    /// Rejects payloads exceeding the selected-reader per-file byte bound before hashing.
    pub fn new(bytes: Vec<u8>) -> Result<Self, OverlayBundleError> {
        if bytes.len() > MAX_READ_BYTES {
            return Err(OverlayBundleError::FileLimit);
        }
        Ok(Self {
            sha256: ArtifactDigest::sha256(&bytes),
            bytes: bytes.into_boxed_slice(),
        })
    }

    #[must_use]
    pub fn sha256(&self) -> &ArtifactDigest {
        &self.sha256
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl fmt::Debug for VerifiedOverlayBlob {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedOverlayBlob")
            .field("bytes", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// Owns exactly the regular-file bytes named by both plan layers, with a stable metadata digest.
/// This is not an atomic repository snapshot, signature, approval, archive or transfer operation.
#[derive(Eq, PartialEq)]
pub struct RepositoryOverlayBundle {
    plan: RepositoryOverlayPlan,
    blobs: BTreeMap<String, VerifiedOverlayBlob>,
    file_bytes: usize,
    manifest_sha256: ArtifactDigest,
}

impl RepositoryOverlayBundle {
    /// Verify completeness and exact lengths before publishing an immutable local bundle.
    /// Duplicate path contents share one supplied blob; extra payloads are never retained.
    /// # Errors
    /// Rejects inconsistent/oversized plans, missing/extra/duplicate blobs or wrong file lengths.
    pub fn new(
        plan: RepositoryOverlayPlan,
        blobs: impl IntoIterator<Item = VerifiedOverlayBlob>,
    ) -> Result<Self, OverlayBundleError> {
        let mut required = RequiredBlobs::new(&plan)?;
        let file_bytes = required.bytes;
        let mut verified = BTreeMap::new();
        for blob in blobs {
            let digest = blob.sha256.as_str();
            if verified.contains_key(digest) {
                return Err(OverlayBundleError::DuplicateBlob);
            }
            let bytes = required
                .sizes
                .remove(digest)
                .ok_or(OverlayBundleError::UnexpectedBlob)?;
            if bytes != blob.bytes.len() {
                return Err(OverlayBundleError::SizeMismatch);
            }
            verified.insert(digest.to_owned(), blob);
        }
        if !required.sizes.is_empty() {
            return Err(OverlayBundleError::MissingBlob);
        }
        drop(required);
        let manifest_sha256 = fingerprint::digest(&plan)?;
        Ok(Self {
            plan,
            blobs: verified,
            file_bytes,
            manifest_sha256,
        })
    }

    #[must_use]
    pub fn plan(&self) -> &RepositoryOverlayPlan {
        &self.plan
    }

    /// Explicit access to a verified payload; no unplanned payload can be returned.
    #[must_use]
    pub fn blob(&self, digest: &ArtifactDigest) -> Option<&[u8]> {
        self.blobs.get(digest.as_str()).map(|blob| blob.bytes.as_ref())
    }

    #[must_use]
    pub fn blobs(&self) -> impl ExactSizeIterator<Item = &VerifiedOverlayBlob> {
        self.blobs.values()
    }

    /// Distinct regular-file bytes, unlike the plan's conservative per-reference payload count.
    #[must_use]
    pub fn file_bytes(&self) -> usize {
        self.file_bytes
    }

    /// Version 1 canonical metadata fingerprint, including both layers and their file hashes.
    /// Equality is not an authorization decision or proof of coherent capture or safe application.
    #[must_use]
    pub fn manifest_sha256(&self) -> &ArtifactDigest {
        &self.manifest_sha256
    }
}

impl fmt::Debug for RepositoryOverlayBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RepositoryOverlayBundle")
            .field("blobs", &self.blobs.len())
            .field("file_bytes", &self.file_bytes)
            .finish_non_exhaustive()
    }
}

struct RequiredBlobs<'a> {
    sizes: BTreeMap<&'a str, usize>,
    bytes: usize,
}

impl<'a> RequiredBlobs<'a> {
    fn new(plan: &'a RepositoryOverlayPlan) -> Result<Self, OverlayBundleError> {
        let mut required = Self {
            sizes: BTreeMap::new(),
            bytes: 0,
        };
        for change in plan.index().iter().chain(plan.working_tree()) {
            if let OverlayContent::File { sha256, bytes, .. } = change.content() {
                let bytes = usize::try_from(*bytes)
                    .ok()
                    .filter(|bytes| *bytes <= MAX_READ_BYTES)
                    .ok_or(OverlayBundleError::FileLimit)?;
                if let Some(previous) = required.sizes.insert(sha256.as_str(), bytes) {
                    if previous != bytes {
                        return Err(OverlayBundleError::SizeMismatch);
                    }
                } else {
                    required.bytes = required
                        .bytes
                        .checked_add(bytes)
                        .filter(|bytes| *bytes <= MAX_BUNDLE_BYTES)
                        .ok_or(OverlayBundleError::BundleLimit)?;
                }
            }
        }
        Ok(required)
    }
}

/// Redacted diagnostics never include file content, source, path or digest values.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum OverlayBundleError {
    #[error("overlay file exceeds the in-memory payload limit")]
    FileLimit,
    #[error("overlay bundle exceeds the in-memory payload budget")]
    BundleLimit,
    #[error("overlay file lengths do not agree with verified content")]
    SizeMismatch,
    #[error("overlay bundle is missing a required file payload")]
    MissingBlob,
    #[error("overlay bundle contains an unplanned file payload")]
    UnexpectedBlob,
    #[error("overlay bundle contains duplicate file payloads")]
    DuplicateBlob,
    #[error("overlay bundle metadata could not be fingerprinted")]
    Encoding,
}

#[cfg(test)]
mod tests;
