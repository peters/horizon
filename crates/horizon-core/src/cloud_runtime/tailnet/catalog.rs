//! Saved catalog removal retains cloud ownership through credential deletion.
use super::{Catalog, Error, Result, Selection, mapped, state, store};
use horizon_cloud::tailnet::CatalogOwnership;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Boundary {
    Enumerated,
    Owned,
}

/// Remove a saved network only when no allocation or pending request needs it.
/// # Errors
/// Busy cloud ownership, changed directory membership, corrupt state or retained resources.
pub fn remove_saved(root: &Path, id: &str) -> Result<Catalog> {
    remove_with(root, id, |_| Ok(()), |ownership| ownership.delete(id).map_err(mapped))
}

fn remove_with(
    root: &Path,
    id: &str,
    mut checkpoint: impl FnMut(Boundary) -> Result<()>,
    commit: impl FnOnce(&CatalogOwnership) -> Result<Catalog>,
) -> Result<Catalog> {
    if !horizon_cloud::tailnet::valid_id(id) {
        return Err(Error::Invalid("Invalid saved tailnet identity"));
    }
    let paths = cloud_paths(root)?;
    checkpoint(Boundary::Enumerated)?;
    // Every writer takes its cloud lock before the existing catalog mutation lock.
    // Keep this complete, sorted set until catalog and credential deletion settle.
    let clouds = paths
        .iter()
        .map(|path| state::Store::lock(path))
        .collect::<Result<Vec<_>>>()?;
    let ownership = store(root).own_catalog().map_err(mapped)?;
    if cloud_paths(root)? != paths {
        return Err(Error::Busy);
    }
    let catalog = ownership.load().map_err(mapped)?;
    if !catalog.tailnets.iter().any(|network| network.id == id) {
        return Err(Error::Invalid("The saved tailnet no longer exists"));
    }
    checkpoint(Boundary::Owned)?;
    for cloud in &clouds {
        let selected = Selection::load(cloud.root()).map_err(mapped)?;
        let requested = super::super::companions::lifecycle::requested_tailnet(cloud.root())?;
        let deployment = cloud.load()?;
        if requested.is_some_and(|selection| selection.tailnet.as_deref() == Some(id)) {
            return Err(Error::Invalid(
                "Cancel the pending cloud request before removing its tailnet",
            ));
        }
        if selected.tailnet.as_deref() != Some(id) {
            continue;
        }
        if let Some(deployment) = deployment {
            if (deployment.stage != super::super::Stage::Deleted
                && (deployment.spec.is_some()
                    || deployment.worker.is_some()
                    || deployment.operation != super::super::CreateState::Prepared))
                || !super::super::lifecycle::can_remove(cloud, &deployment)?
            {
                return Err(Error::Invalid(
                    "Delete clouds assigned to this tailnet before removing its credentials",
                ));
            }
        } else if provider_record_exists(cloud.root())? {
            return Err(Error::Invalid(
                "Provider state exists without a deployment; preserve the tailnet",
            ));
        }
    }
    commit(&ownership)
}

fn cloud_paths(root: &Path) -> Result<BTreeSet<PathBuf>> {
    let mut clouds = BTreeSet::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if !kind.is_dir() && !kind.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        // Legacy allocation journals are not additional cloud identities.
        if name == ".allocations" && kind.is_dir() {
            continue;
        }
        if kind.is_symlink() || !name.to_str().is_some_and(horizon_cloud::valid_id) {
            return Err(Error::Invalid("Cloud catalog contains an unknown directory"));
        }
        clouds.insert(entry.path());
    }
    Ok(clouds)
}

fn provider_record_exists(cloud: &Path) -> Result<bool> {
    for name in ["hetzner.json", "workspace-volume.json", "workspace-volume.required"] {
        match std::fs::symlink_metadata(cloud.join(name)) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(false)
}

/// Caller retains cloud ownership through any later provider allocation.
pub(super) fn validate_selection(cloud: &Path) -> Result<()> {
    if Selection::load(cloud).map_err(mapped)?.tailnet.is_none() {
        return Ok(());
    }
    let root = cloud.parent().ok_or(Error::Invalid("Missing cloud catalog root"))?;
    let ownership = store(root).own_catalog().map_err(mapped)?;
    let catalog = ownership.load().map_err(mapped)?;
    let selection = Selection::load(cloud).map_err(mapped)?;
    if selection
        .tailnet
        .as_deref()
        .is_some_and(|id| !catalog.tailnets.iter().any(|network| network.id == id))
    {
        return Err(Error::Invalid(
            "The selected tailnet is no longer saved; refuse allocation",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
