use super::{Error, Owner, Phase, Result, Store};
use horizon_cloud_protocol::OperationId;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::Path,
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    pub owner: Owner,
    pub id: OperationId,
    pub phase: Phase,
    #[serde(default)]
    pub released_workers: std::collections::BTreeSet<String>,
}

pub(super) fn load(root: &Path) -> Result<Option<Receipt>> {
    let file = match std::fs::File::open(root.join("companion-operation.json")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(16 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 {
        return Err(Error::Invalid("Companion target claim is too large"));
    }
    let record: Receipt = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
    if [
        &record.owner.cloud_id,
        &record.owner.scope.session_id,
        &record.owner.scope.workspace_id,
    ]
    .iter()
    .any(|id| !horizon_cloud::valid_id(id))
    {
        return Err(Error::Invalid("Invalid companion target owner"));
    }
    if record.released_workers.len() > 256 || record.released_workers.iter().any(|id| !horizon_cloud::valid_id(id)) {
        return Err(Error::Invalid("Invalid retired companion workers"));
    }
    Ok(Some(record))
}

pub(super) fn save(store: &Store, owner: &Owner, id: OperationId, phase: Phase) -> Result<()> {
    let record = Receipt {
        owner: owner.clone(),
        id,
        phase,
        released_workers: load(store.root())?.map_or_else(Default::default, |record| record.released_workers),
    };
    write(store, &record)
}

fn write(store: &Store, record: &Receipt) -> Result<()> {
    let bytes = serde_json::to_vec(record).map_err(|_| Error::Json)?;
    if bytes.len() > 16 * 1024 {
        return Err(Error::Invalid("Companion target claim is too large"));
    }
    let mut file = tempfile::NamedTempFile::new_in(store.root())?;
    file.write_all(&bytes)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(store.root().join("companion-operation.json"))
        .map_err(|error| error.error)?;
    #[cfg(unix)]
    std::fs::File::open(store.root())?.sync_all()?;
    Ok(())
}

pub(super) struct ExecutionLock(std::fs::File);

impl Drop for ExecutionLock {
    fn drop(&mut self) {
        // Closing alone leaves a briefly fork-inherited descriptor holding the lock.
        let _ = self.0.unlock();
    }
}

/// Held across provider work and SSH grant verification, which use operation.lock
/// separately. Re-entering a live executor must not masquerade as crash recovery.
pub(super) fn execution_lock(root: &Path) -> Result<ExecutionLock> {
    std::fs::create_dir_all(root)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("companion-execution.lock"))?;
    file.try_lock().map_err(|_| Error::Busy)?;
    Ok(ExecutionLock(file))
}

pub(super) fn released(
    store: &Store,
    reconciled: &crate::cloud_runtime::lifecycle::ReconciledDeployment,
) -> Result<()> {
    if reconciled.state.profile.provider != horizon_cloud::hetzner::PROVIDER || !reconciled.confirmed_stopped() {
        return Err(Error::Invalid("The previous Hetzner server is not confirmed released"));
    }
    let horizon_cloud::CreateState::Bound { worker_id } = &reconciled.state.operation else {
        return Err(Error::Invalid("Released server identity is missing"));
    };
    let mut record = load(store.root())?.ok_or(Error::Invalid("Missing companion target claim"))?;
    if record.released_workers.len() >= 256 && !record.released_workers.contains(worker_id) {
        return Err(Error::Invalid(
            "Companion server history is full; retain its recovery journal",
        ));
    }
    record.released_workers.insert(worker_id.clone());
    write(store, &record)
}

/// Nonsecret selection accepted by an outstanding operation; caller holds the target lock.
pub(in crate::cloud_runtime) fn requested_tailnet(
    root: &std::path::Path,
) -> Result<Option<horizon_cloud::tailnet::Selection>> {
    let claim = load(root)?;
    let terminal = claim.as_ref().is_some_and(|claim| {
        matches!(
            claim.phase,
            Phase::Ready | Phase::Stopped | Phase::Refused | Phase::RetryRequired
        )
    });
    let mut requested = if let Some(claim) = claim.as_ref().filter(|_| !terminal) {
        Some(read_tailnet_request(root, claim.id, true)?)
    } else {
        None
    };
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(id) = name
            .strip_prefix("tailnet-commit-")
            .and_then(|name| name.strip_suffix(".pending"))
        else {
            continue;
        };
        let operation = uuid::Uuid::parse_str(id).map_err(|_| Error::Json)?;
        let operation = OperationId::try_from(operation).map_err(|_| Error::Json)?;
        if !entry.file_type()?.is_file() {
            return Err(Error::Invalid("Pending tailnet marker is not a regular file"));
        }
        let mut marker = Vec::new();
        std::fs::File::open(entry.path())?.take(2).read_to_end(&mut marker)?;
        if marker != b"1" {
            return Err(Error::Invalid("Invalid pending tailnet marker"));
        }
        if terminal && claim.as_ref().is_some_and(|claim| claim.id == operation) {
            continue;
        }
        let selection = read_tailnet_request(root, operation, false)?;
        if requested.as_ref().is_some_and(|prior| prior != &selection) {
            return Err(Error::Invalid("Conflicting pending tailnet reservations"));
        }
        requested = Some(selection);
    }
    Ok(requested)
}

fn read_tailnet_request(
    root: &Path,
    operation: OperationId,
    legacy: bool,
) -> Result<horizon_cloud::tailnet::Selection> {
    let path = root.join(format!("tailnet-request-{operation}.json"));
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if legacy && error.kind() == std::io::ErrorKind::NotFound => {
            return horizon_cloud::tailnet::Selection::load(root).map_err(|_| Error::Json);
        }
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(1025).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 {
        return Err(Error::Invalid("Pending tailnet request is too large"));
    }
    let selection: horizon_cloud::tailnet::Selection = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
    if selection
        .tailnet
        .as_deref()
        .is_some_and(|id| !horizon_cloud::tailnet::valid_id(id))
    {
        return Err(Error::Invalid("Invalid pending tailnet selection"));
    }
    Ok(selection)
}
