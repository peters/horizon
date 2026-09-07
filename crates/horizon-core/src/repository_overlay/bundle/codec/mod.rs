//! Bounded portable bytes, not export permission, authenticated transport or filesystem application.

mod metadata;

use super::super::{MAX_CHANGES, MAX_METADATA_BYTES, OverlayPlanError};
use super::{OverlayBundleError, RepositoryOverlayBundle, RequiredBlobs, VerifiedOverlayBlob, fingerprint};

const MAGIC: &[u8; 8] = b"HZOVLY\0\x01";
const RECORD_HEADER_BYTES: usize = 64 + 8;
// Allowed strings require at most doubled quote escaping; reserve fixed field overhead per change.
const MAX_ENCODED_METADATA_BYTES: usize = 2 * MAX_METADATA_BYTES + 256 * MAX_CHANGES + 1024;

/// Maximum complete version 1 encoding, including bounded metadata and per-blob framing.
pub const MAX_ENCODED_BUNDLE_BYTES: usize =
    super::MAX_BUNDLE_BYTES + MAX_ENCODED_METADATA_BYTES + MAX_CHANGES * RECORD_HEADER_BYTES + 16;

/// Encode a verified bundle without I/O. This creates a separate owned copy of its payloads.
/// Run off the UI thread; the caller remains responsible for any export authorization.
/// # Errors
/// Rejects encoding/size failures or inability to reserve the bounded output allocation.
pub fn encode(bundle: &RepositoryOverlayBundle) -> Result<Box<[u8]>, OverlayCodecError> {
    let metadata = fingerprint::encode(bundle.plan())?;
    let count = bundle.blobs().len();
    let length = encoded_length(metadata.len(), count, bundle.file_bytes())?;
    let mut encoded = Vec::new();
    encoded
        .try_reserve_exact(length)
        .map_err(|_| OverlayCodecError::Allocation)?;
    encoded.extend_from_slice(MAGIC);
    encoded.extend_from_slice(
        &u32::try_from(metadata.len())
            .map_err(|_| OverlayCodecError::Limit)?
            .to_le_bytes(),
    );
    encoded.extend_from_slice(&metadata);
    encoded.extend_from_slice(
        &u32::try_from(count)
            .map_err(|_| OverlayCodecError::Limit)?
            .to_le_bytes(),
    );
    for blob in bundle.blobs() {
        encoded.extend_from_slice(blob.sha256().as_str().as_bytes());
        encoded.extend_from_slice(
            &u64::try_from(blob.bytes().len())
                .map_err(|_| OverlayCodecError::Limit)?
                .to_le_bytes(),
        );
        encoded.extend_from_slice(blob.bytes());
    }
    Ok(encoded.into_boxed_slice())
}

fn encoded_length(metadata: usize, count: usize, payloads: usize) -> Result<usize, OverlayCodecError> {
    if metadata > MAX_ENCODED_METADATA_BYTES || count > MAX_CHANGES || payloads > super::MAX_BUNDLE_BYTES {
        return Err(OverlayCodecError::Limit);
    }
    count
        .checked_mul(RECORD_HEADER_BYTES)
        .and_then(|bytes| bytes.checked_add(16))
        .and_then(|bytes| bytes.checked_add(metadata))
        .and_then(|bytes| bytes.checked_add(payloads))
        .filter(|bytes| *bytes <= MAX_ENCODED_BUNDLE_BYTES)
        .ok_or(OverlayCodecError::Limit)
}

/// Decode explicitly supplied bytes, rechecking metadata, framing and every payload hash.
/// This owns a separate bounded payload copy; it does not validate real filesystem topology.
/// Run off the UI thread. Successful decoding grants no permission to apply or export the result.
/// # Errors
/// Rejects unsupported versions, noncanonical/malformed encodings, limit excess or invalid bundles.
pub fn decode(encoded: &[u8]) -> Result<RepositoryOverlayBundle, OverlayCodecError> {
    if encoded.len() > MAX_ENCODED_BUNDLE_BYTES {
        return Err(OverlayCodecError::Limit);
    }
    let mut cursor = Cursor { remaining: encoded };
    if cursor.take(MAGIC.len())? != MAGIC {
        return Err(OverlayCodecError::Unsupported);
    }
    let metadata_bytes = usize::try_from(cursor.u32()?).map_err(|_| OverlayCodecError::Limit)?;
    if metadata_bytes > MAX_ENCODED_METADATA_BYTES {
        return Err(OverlayCodecError::Limit);
    }
    let plan = metadata::decode(cursor.take(metadata_bytes)?)?;
    let mut required = RequiredBlobs::new(&plan)?;
    let count = usize::try_from(cursor.u32()?).map_err(|_| OverlayCodecError::Limit)?;
    if count > MAX_CHANGES {
        return Err(OverlayCodecError::Limit);
    }
    if count != required.sizes.len() {
        return Err(OverlayCodecError::Malformed);
    }
    let mut blobs = Vec::new();
    blobs
        .try_reserve_exact(count)
        .map_err(|_| OverlayCodecError::Allocation)?;
    let mut previous: Option<&str> = None;
    for _ in 0..count {
        let digest = std::str::from_utf8(cursor.take(64)?).map_err(|_| OverlayCodecError::Malformed)?;
        if previous.is_some_and(|previous| previous >= digest) {
            return Err(OverlayCodecError::NonCanonical);
        }
        let size = usize::try_from(cursor.u64()?).map_err(|_| OverlayCodecError::Limit)?;
        let expected = required.sizes.remove(digest).ok_or(OverlayCodecError::Malformed)?;
        if size != expected {
            return Err(OverlayCodecError::Malformed);
        }
        blobs.push(verified_blob(cursor.take(size)?, digest)?);
        previous = Some(digest);
    }
    if !cursor.remaining.is_empty() {
        return Err(OverlayCodecError::NonCanonical);
    }
    drop(required);
    Ok(RepositoryOverlayBundle::new(plan, blobs)?)
}

fn verified_blob(bytes: &[u8], digest: &str) -> Result<VerifiedOverlayBlob, OverlayCodecError> {
    let mut owned = Vec::new();
    owned
        .try_reserve_exact(bytes.len())
        .map_err(|_| OverlayCodecError::Allocation)?;
    owned.extend_from_slice(bytes);
    let blob = VerifiedOverlayBlob::new(owned)?;
    if blob.sha256().as_str() != digest {
        return Err(OverlayCodecError::Malformed);
    }
    Ok(blob)
}

struct Cursor<'a> {
    remaining: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], OverlayCodecError> {
        let (bytes, remaining) = self
            .remaining
            .split_at_checked(length)
            .ok_or(OverlayCodecError::Malformed)?;
        self.remaining = remaining;
        Ok(bytes)
    }

    fn u32(&mut self) -> Result<u32, OverlayCodecError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| OverlayCodecError::Malformed)?,
        ))
    }

    fn u64(&mut self) -> Result<u64, OverlayCodecError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| OverlayCodecError::Malformed)?,
        ))
    }
}

/// Redacted format errors do not include input bytes, paths, identities or payload hashes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum OverlayCodecError {
    #[error("repository bundle format is unsupported")]
    Unsupported,
    #[error("repository bundle encoding is malformed")]
    Malformed,
    #[error("repository bundle encoding is not canonical")]
    NonCanonical,
    #[error("repository bundle encoding exceeds a size limit")]
    Limit,
    #[error("repository bundle allocation could not be reserved")]
    Allocation,
    #[error(transparent)]
    Plan(#[from] OverlayPlanError),
    #[error(transparent)]
    Bundle(#[from] OverlayBundleError),
}

#[cfg(test)]
mod tests;
