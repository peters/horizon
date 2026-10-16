//! Read-only durable deletion checks. No provider credential or fresh request.
use super::{CreateState, Deployment, Result, Store, kind};
use crate::cloud_runtime::{Stage, deployment::storage};
use horizon_cloud::{hetzner::cloud::Journal, provider::Kind};

pub(in crate::cloud_runtime) fn confirmed(store: &Store, state: &Deployment) -> Result<bool> {
    let Some(spec) = &state.spec else { return Ok(false) };
    if state.stage != Stage::Deleted
        || state.worker.is_some()
        || state.image_replacement.is_some()
        || state.session_restart.is_some()
        || spec.operation_id != state.cloud_id
        || spec.profile != state.profile
        || !matches!(state.operation, CreateState::Prepared | CreateState::Terminated { .. })
    {
        return Ok(false);
    }
    storage::resize::require_settled(store.root())?;
    storage::growth::require_settled(store.root())?;
    let provider = kind(&state.profile);
    for (file, allowed) in [
        ("hetzner.json", provider == Kind::Hetzner),
        ("docker-host.json", provider == Kind::DockerHost),
        ("workspace-volume.json", provider == Kind::RunPod),
        ("workspace-volume.required", provider == Kind::RunPod),
    ] {
        if !allowed && store.root().join(file).try_exists()? {
            return Ok(false);
        }
    }
    match provider {
        Kind::RunPod => storage::deletion_confirmed(store, state),
        Kind::Hetzner => {
            let Some(bytes) = read(store, "hetzner.json")? else {
                return Ok(false);
            };
            let journal: Journal = serde_json::from_slice(&bytes).map_err(|_| super::super::Error::Json)?;
            Ok(journal.deleting
                && matches!(&journal.volume, CreateState::Terminated { worker_id }
                    if worker_id.parse::<u64>().is_ok_and(|id| id != 0 && id.to_string() == *worker_id))
                && journal
                    .location
                    .as_deref()
                    .is_some_and(horizon_cloud::hetzner::cloud::valid_placement_name)
                && journal.key.is_none()
                && journal.released.is_none()
                && !journal.unused)
        }
        Kind::DockerHost => {
            let Some(bytes) = read(store, "docker-host.json")? else {
                return Ok(false);
            };
            // The existing SDK saves the empty default journal only after container
            // and volume absence. Reject extra fields rather than masking a future resource.
            let journal: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| super::super::Error::Json)?;
            Ok(matches!(state.operation, CreateState::Terminated { .. })
                && journal.as_object().is_some_and(serde_json::Map::is_empty))
        }
    }
}

fn read(store: &Store, file: &str) -> Result<Option<Vec<u8>>> {
    use std::io::Read;
    const LIMIT: usize = 1024 * 1024;
    let mut file = match std::fs::File::open(store.root().join(file)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.by_ref().take((LIMIT + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > LIMIT {
        return Err(super::super::Error::Invalid("Provider deletion journal is oversized"));
    }
    Ok(Some(bytes))
}
