use super::{Error, OperationId, Result, Store};
use crate::cloud_runtime::{CreateState, tailnet};
use horizon_cloud::tailnet::Selection;
use std::{
    io::{Read, Write},
    path::Path,
};

pub(super) struct Choice {
    selection: Selection,
    id: OperationId,
}

impl Choice {
    pub(super) fn commit(self, target: &Store) -> Result<()> {
        self.selection
            .commit(target.root())
            .map_err(|_| Error::Invalid("Could not save tailnet selection"))?;
        std::fs::remove_file(target.root().join(format!("tailnet-commit-{}.pending", self.id)))?;
        sync(target.root())?;
        Ok(())
    }
}

pub(super) fn prepare(root: &Path, target: &Store, id: OperationId, tailnet: Option<&str>) -> Result<Choice> {
    let prior = horizon_cloud::tailnet::Selection::load(target.root()).map_err(|_| Error::Json)?;
    let selected = match tailnet {
        Some("none") => None,
        Some(id) => Some(id),
        None => prior.tailnet.as_deref(),
    };
    let catalog = tailnet
        .map(|_| tailnet::store(root).load())
        .transpose()
        .map_err(|_| Error::Invalid("Tailnet settings are unavailable"))?;
    if catalog
        .as_ref()
        .is_some_and(|catalog| selected.is_some_and(|id| !catalog.tailnets.iter().any(|t| t.id == id)))
    {
        return Err(Error::Invalid("Choose a saved tailnet ID from cloud_companions"));
    }
    if target
        .load()?
        .is_some_and(|s| s.spec.is_some() || s.worker.is_some() || s.operation != CreateState::Prepared)
        && prior.tailnet.as_deref() != selected
    {
        return Err(Error::Invalid("Tailnet is chosen only when provisioning a cloud"));
    }
    let path = target.root().join(format!("tailnet-request-{id}.json"));
    let requested = horizon_cloud::tailnet::Selection {
        tailnet: selected.map(str::to_owned),
    };
    if path.exists() {
        let bytes = std::fs::read(&path)?;
        let prior: horizon_cloud::tailnet::Selection = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
        if prior != requested {
            return Err(Error::Invalid("A retried operation cannot change its tailnet"));
        }
    } else {
        let mut pending = tempfile::NamedTempFile::new_in(target.root())?;
        pending.write_all(&serde_json::to_vec(&requested).map_err(|_| Error::Json)?)?;
        pending.as_file().sync_all()?;
        pending.persist(path).map_err(|e| Error::Io(e.error))?;
    }
    let mut pending = tempfile::NamedTempFile::new_in(target.root())?;
    pending.write_all(b"1")?;
    pending.as_file().sync_all()?;
    pending
        .persist(target.root().join(format!("tailnet-commit-{id}.pending")))
        .map_err(|e| Error::Io(e.error))?;
    sync(target.root())?;
    Ok(Choice {
        selection: requested,
        id,
    })
}

fn sync(root: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::File::open(root)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = root;
    Ok(())
}

pub(super) fn recover(target: &Store, id: OperationId) -> Result<()> {
    if !target
        .root()
        .join(format!("tailnet-commit-{id}.pending"))
        .try_exists()?
    {
        return Ok(());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(target.root().join(format!("tailnet-request-{id}.json")))?
        .take(1025)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 {
        return Err(Error::Invalid("Pending tailnet request is too large"));
    }
    let selection: Selection = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
    if target
        .load()?
        .is_some_and(|s| s.spec.is_some() || s.worker.is_some() || s.operation != CreateState::Prepared)
        && Selection::load(target.root()).map_err(|_| Error::Json)? != selection
    {
        return Err(Error::Invalid("Cannot recover a changed tailnet on an allocated cloud"));
    }
    Choice { selection, id }.commit(target)
}

pub(super) fn validate_pending(target: &Store, intent: &super::Intent, claim: &super::receipt::Receipt) -> Result<()> {
    if intent.action == super::Action::EnsureReady
        && intent.state == super::State::Submitted
        && claim.phase == super::Phase::Submitted
    {
        recover(target, intent.operation_id)?;
    }
    tailnet::validate_pending(target.root())
}
