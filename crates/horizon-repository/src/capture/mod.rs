//! Explicit retained-volume byte capture, not a coherent snapshot or recovery checkpoint.

#[cfg(target_os = "linux")]
use horizon_core::repository_overlay::{
    bundle::{
        RepositoryOverlayBundle, codec,
        store::{BundleStoreError, RepositoryBundleStore},
    },
    capture::{GitCaptureError, capture_selected},
    reader::RepositoryReadError,
};
use horizon_core::{
    cloud_run::ArtifactDigest,
    repository_git::GitPreparation,
    repository_overlay::{OverlayChange, OverlayContent},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    process::ExitCode,
};

const REQUEST_LIMIT: u64 = 128 * 1024;
const CAPACITY: u64 = 1024 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Enrollment {
    version: u8,
    preparation: GitPreparation,
    selected: Vec<String>,
    retained_volume_attested: bool,
}

impl Enrollment {
    fn binding(&self) -> Result<ArtifactDigest, Reason> {
        if self.version != 1 || !self.retained_volume_attested || self.selected.is_empty() || self.selected.len() > 128
        {
            return Err(Reason::Invalid);
        }
        self.preparation.binding().map_err(|_| Reason::Invalid)?;
        let mut unique = BTreeSet::new();
        for path in &self.selected {
            OverlayChange::new(path.clone(), OverlayContent::Remove).map_err(|_| Reason::Invalid)?;
            if !unique.insert(path) {
                return Err(Reason::Invalid);
            }
        }
        let mut bytes = b"horizon-retained-byte-capture-v1\0".to_vec();
        bytes.extend(serde_json::to_vec(self).map_err(|_| Reason::Invalid)?);
        Ok(ArtifactDigest::sha256(&bytes))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    enrollment: Enrollment,
    available_bytes: u64,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Reason {
    Invalid,
    #[cfg(not(target_os = "linux"))]
    Unsupported,
    #[cfg(target_os = "linux")]
    Identity,
    #[cfg(target_os = "linux")]
    Capture,
    #[cfg(target_os = "linux")]
    Changed,
    #[cfg(target_os = "linux")]
    Capacity,
    #[cfg(target_os = "linux")]
    Storage,
}

#[derive(Serialize)]
struct Response {
    version: u8,
    binding: Option<ArtifactDigest>,
    manifest: Option<ArtifactDigest>,
    record_sha256: Option<ArtifactDigest>,
    record_bytes: Option<u64>,
    reason: Option<Reason>,
}

pub(super) fn run(
    plan: bool,
    input: &mut impl Read,
    output: &mut impl Write,
    diagnostics: &mut impl Write,
) -> ExitCode {
    let mut response = Response {
        version: 1,
        binding: None,
        manifest: None,
        record_sha256: None,
        record_bytes: None,
        reason: None,
    };
    response.reason = (|| {
        let mut bytes = Vec::new();
        input
            .take(REQUEST_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Reason::Invalid)?;
        if bytes.len() as u64 > REQUEST_LIMIT {
            return Err(Reason::Invalid);
        }
        let request: Request = serde_json::from_slice(&bytes).map_err(|_| Reason::Invalid)?;
        let binding = request.enrollment.binding()?;
        if request.available_bytes > CAPACITY {
            return Err(Reason::Invalid);
        }
        execute(&request, &binding, plan, &mut response)?;
        response.binding = Some(binding);
        Ok(())
    })()
    .err();
    let code = match response.reason {
        None => 0,
        Some(Reason::Invalid) => 2,
        #[cfg(not(target_os = "linux"))]
        Some(Reason::Unsupported) => 2,
        #[cfg(target_os = "linux")]
        Some(_) => 1,
    };
    super::write_response(&response, code, 1024, output, diagnostics)
}

#[cfg(target_os = "linux")]
fn execute(request: &Request, binding: &ArtifactDigest, plan: bool, response: &mut Response) -> Result<(), Reason> {
    use std::{fs, os::unix::fs::MetadataExt};
    let preparation = &request.enrollment.preparation;
    let before = preparation.inspect_checkout().map_err(|_| Reason::Identity)?;
    if plan {
        return Ok(());
    }
    let root = std::path::PathBuf::from("/workspace/.horizon-worker/byte-captures")
        .join(binding.as_str())
        .join("bundles");
    // Storage and capture both reject symlinked ancestry. Stable trusted ownership
    // is required; these checks do not defend against a malicious worker owner.
    let store = RepositoryBundleStore::open_named(&root).map_err(|_| Reason::Storage)?;
    let destination = fs::symlink_metadata(&root).map_err(|_| Reason::Storage)?;
    if destination.dev() != before.device {
        return Err(Reason::Identity);
    }
    let selected: Vec<_> = request.enrollment.selected.iter().map(String::as_str).collect();
    let bundle = capture_selected(std::path::Path::new(before.path), preparation.source.clone(), &selected).map_err(
        |error| match error {
            GitCaptureError::Changed | GitCaptureError::Read(RepositoryReadError::Changed) => Reason::Changed,
            _ => Reason::Capture,
        },
    )?;
    if preparation.inspect_checkout().map_err(|_| Reason::Identity)? != before {
        return Err(Reason::Identity);
    }
    let encoded = codec::encode(&bundle).map_err(|_| Reason::Capture)?;
    let length = encoded.len() as u64;
    if needs_space(store.get(bundle.manifest_sha256()), &bundle)?
        && length > request.available_bytes.min(available(&root, destination.uid())?)
    {
        return Err(Reason::Capacity);
    }
    let record_sha256 = ArtifactDigest::sha256(&encoded);
    drop(encoded);
    let digest = store.put(&bundle).map_err(|_| Reason::Storage)?;
    let readback = store.get(&digest).map_err(|_| Reason::Storage)?;
    if readback != bundle || preparation.inspect_checkout().map_err(|_| Reason::Identity)? != before {
        return Err(Reason::Identity);
    }
    let after = fs::symlink_metadata(&root).map_err(|_| Reason::Storage)?;
    if (after.dev(), after.ino()) != (destination.dev(), destination.ino()) {
        return Err(Reason::Identity);
    }
    response.manifest = Some(digest);
    response.record_sha256 = Some(record_sha256);
    response.record_bytes = Some(length);
    Ok(())
}

#[cfg(target_os = "linux")]
fn needs_space(
    existing: Result<RepositoryOverlayBundle, BundleStoreError>,
    expected: &RepositoryOverlayBundle,
) -> Result<bool, Reason> {
    match existing {
        Ok(value) if value == *expected => Ok(false),
        Err(BundleStoreError::Missing) => Ok(true),
        _ => Err(Reason::Storage),
    }
}

#[cfg(target_os = "linux")]
fn available(root: &std::path::Path, owner: u32) -> Result<u64, Reason> {
    use std::os::unix::fs::MetadataExt;
    let device = std::fs::symlink_metadata(root).map_err(|_| Reason::Storage)?.dev();
    let mut total = 0u64;
    for (index, entry) in std::fs::read_dir(root).map_err(|_| Reason::Storage)?.enumerate() {
        if index >= 4095 {
            return Err(Reason::Capacity);
        }
        let entry = entry.map_err(|_| Reason::Storage)?;
        let name = entry.file_name();
        let stem = name.to_str().ok_or(Reason::Storage)?;
        if stem.len() != 64
            || !stem
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Reason::Storage);
        }
        let slot = std::fs::symlink_metadata(entry.path()).map_err(|_| Reason::Storage)?;
        if !slot.is_dir()
            || slot.uid() != owner
            || slot.mode() & 0o7777 != 0o700
            || slot.nlink() == 0
            || slot.dev() != device
        {
            return Err(Reason::Storage);
        }
        let mut contents = std::fs::read_dir(entry.path()).map_err(|_| Reason::Storage)?;
        let record = contents.next().ok_or(Reason::Storage)?.map_err(|_| Reason::Storage)?;
        if record.file_name() != "record.hzov" || contents.next().is_some() {
            return Err(Reason::Storage); // Partial claims are retained, never treated as reusable capacity.
        }
        let metadata = std::fs::symlink_metadata(record.path()).map_err(|_| Reason::Storage)?;
        if !metadata.is_file()
            || metadata.uid() != owner
            || metadata.mode() & 0o7777 != 0o600
            || metadata.nlink() != 1
            || metadata.dev() != device
            || metadata.len() == 0
            || metadata.len() > codec::MAX_ENCODED_BUNDLE_BYTES as u64
        {
            return Err(Reason::Storage);
        }
        total = total
            .checked_add(metadata.len())
            .filter(|total| *total <= CAPACITY)
            .ok_or(Reason::Capacity)?;
    }
    Ok(CAPACITY - total)
}

#[cfg(not(target_os = "linux"))]
fn execute(_: &Request, _: &ArtifactDigest, _: bool, _: &mut Response) -> Result<(), Reason> {
    Err(Reason::Unsupported)
}

#[cfg(test)]
mod tests;
