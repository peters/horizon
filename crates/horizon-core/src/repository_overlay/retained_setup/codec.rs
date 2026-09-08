use super::{SetupClaimError as Error, SetupIntent};
use crate::cloud_run::ArtifactDigest;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub(super) const MAX_RECORD_BYTES: usize = 64 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    workspace_local_id: String,
    objects_directory: PathBuf,
    bundle_store: PathBuf,
    bundle_manifest: ArtifactDigest,
    destination: String,
}

pub(super) fn encode(intent: &SetupIntent) -> Result<Vec<u8>, Error> {
    let record = Record {
        version: 1,
        workspace_local_id: intent.workspace_local_id.clone(),
        objects_directory: intent.objects_directory.clone(),
        bundle_store: intent.bundle_store.clone(),
        bundle_manifest: intent.bundle_manifest.clone(),
        destination: intent.destination.clone(),
    };
    let bytes = serde_json::to_vec(&record).map_err(|_| Error::InvalidRecord)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(Error::InvalidRecord);
    }
    Ok(bytes)
}

pub(super) fn decode(bytes: &[u8]) -> Result<SetupIntent, Error> {
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(Error::InvalidRecord);
    }
    let record: Record = serde_json::from_slice(bytes).map_err(|_| Error::InvalidRecord)?;
    if record.version != 1 {
        return Err(Error::InvalidRecord);
    }
    let intent = SetupIntent::new(
        record.workspace_local_id,
        record.objects_directory,
        record.bundle_store,
        record.bundle_manifest,
        record.destination,
    )
    .map_err(|_| Error::InvalidRecord)?;
    if encode(&intent)? != bytes {
        return Err(Error::InvalidRecord);
    }
    Ok(intent)
}
