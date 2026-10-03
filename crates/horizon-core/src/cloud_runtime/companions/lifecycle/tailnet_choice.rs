use super::{Error, OperationId, Result, Store};
use crate::cloud_runtime::{CreateState, tailnet};
use horizon_cloud::tailnet::{Catalog, Selection};
use std::{io::Write, path::Path};

pub(super) struct Choice {
    selection: Selection,
    catalog: Option<Catalog>,
}

impl Choice {
    pub(super) fn commit(self, target: &Store) -> Result<()> {
        if let Some(catalog) = self.catalog {
            Selection::save(target.root(), self.selection.tailnet.as_deref(), &catalog)
                .map_err(|_| Error::Invalid("Could not save tailnet selection"))?;
        }
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
    #[cfg(unix)]
    std::fs::File::open(target.root())?.sync_all()?;
    Ok(Choice {
        selection: requested,
        catalog,
    })
}
