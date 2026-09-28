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
    /// The operation whose cloud creation the owner confirmed on the card. Only that
    /// operation may allocate the target's first worker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed: Option<OperationId>,
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
    let previous = load(store.root())?;
    let confirmed = previous
        .as_ref()
        .and_then(|record| record.confirmed)
        .filter(|confirmed| *confirmed == id);
    let record = Receipt {
        owner: owner.clone(),
        id,
        phase,
        released_workers: previous.map_or_else(Default::default, |record| record.released_workers),
        confirmed,
    };
    write(store, &record)
}

/// Records the owner's confirmation that `id` may create the target's first worker.
pub(super) fn confirm(store: &Store, owner: &Owner, id: OperationId) -> Result<()> {
    let record = Receipt {
        owner: owner.clone(),
        id,
        phase: Phase::Submitted,
        released_workers: load(store.root())?.map_or_else(Default::default, |record| record.released_workers),
        confirmed: Some(id),
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
