use horizon_core::{
    cloud_run::ArtifactDigest,
    repository_overlay::{materialize::MAX_REQUEST_PATH_BYTES, retained_setup::SetupIntent},
};
use serde::Deserialize;
use std::{io::Read, path::PathBuf};

// Three supported paths may each need six-byte JSON escaping, plus identity/name fields.
pub(super) const REQUEST_LIMIT: usize = (3 * 6 * MAX_REQUEST_PATH_BYTES + 8192).next_power_of_two();
pub(super) const RESPONSE_LIMIT: usize = (3 * 6 * (MAX_REQUEST_PATH_BYTES + 256) + 8193).next_power_of_two();

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireRequest {
    version: u32,
    retained_root: PathBuf,
    workspace_local_id: String,
    objects_directory: PathBuf,
    bundle_store: PathBuf,
    bundle_manifest: ArtifactDigest,
    destination: String,
}

pub(super) struct Request {
    pub(super) retained_root: PathBuf,
    pub(super) intent: SetupIntent,
}

pub(super) fn read(input: &mut impl Read) -> Result<Request, ()> {
    let mut bytes = Vec::new();
    input
        .take(REQUEST_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() > REQUEST_LIMIT {
        return Err(());
    }
    let wire: WireRequest = serde_json::from_slice(&bytes).map_err(|_| ())?;
    if wire.version != super::VERSION {
        return Err(());
    }
    let intent = SetupIntent::new(
        wire.workspace_local_id,
        wire.objects_directory,
        wire.bundle_store,
        wire.bundle_manifest,
        wire.destination,
    )
    .map_err(|_| ())?;
    Ok(Request {
        retained_root: wire.retained_root,
        intent,
    })
}
