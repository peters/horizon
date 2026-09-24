use super::{Error, Result, transaction};
use crate::cloud_runtime::{deployment::storage, state::Deployment};
use std::{collections::BTreeMap, path::Path};

pub(super) fn capture(root: &Path, deployment: &Deployment) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    for name in ["workspace-volume.json", "workspace-volume.required"] {
        if let Some(bytes) = transaction::read_optional(&root.join(name))? {
            files.insert(name.into(), bytes);
        }
    }
    if !files.is_empty() {
        let worker = deployment
            .spec
            .as_ref()
            .ok_or(Error::Invalid("Storage journal has no worker specification"))?;
        storage::validate_migration(root, worker)?;
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_str().is_some_and(|name| name.starts_with("known-hosts-")) {
            let name = name
                .into_string()
                .map_err(|_| Error::Invalid("Invalid trust filename"))?;
            let bytes = transaction::read_optional(&entry.path())?.ok_or(Error::Invalid("Missing SSH trust file"))?;
            files.insert(name, bytes);
        }
    }
    Ok(files)
}

// Migration is serialized by the parent migration lock, then allocation/project
// locks. Never silently coalesce two legacy controllers with the same provider ID.
pub(super) fn reject_duplicate_workers(root: &Path, deployment: &Deployment) -> Result<()> {
    let owned = worker_ids(deployment);
    let parent = root.parent().ok_or(Error::Invalid("Missing cloud parent"))?;
    let own = super::read_intent(root)?.ok_or(Error::Invalid("Missing migration ownership"))?;
    for records in transaction::registered_records(parent)? {
        if records.allocation_id() != own.allocation && !owned.is_disjoint(&worker_ids(&records.deployment())) {
            return Err(Error::Invalid(
                "Another migrated allocation references the same provider worker",
            ));
        }
    }
    if owned.is_empty() {
        return Ok(());
    }
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        if entry.path() == root
            || !entry.file_name().to_str().is_some_and(horizon_cloud::valid_id)
            || !entry.file_type()?.is_dir()
        {
            continue;
        }
        let Some(bytes) = transaction::read_optional(&entry.path().join("deployment.json"))? else {
            if super::read_intent(&entry.path())?.is_some() {
                return Err(Error::Invalid("Neighboring migration has lost its project journal"));
            }
            continue;
        };
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
        let other: Deployment = if value.get("version").and_then(serde_json::Value::as_u64) == Some(1) {
            serde_json::from_value(value).map_err(|_| Error::Json)?
        } else {
            let intent =
                super::read_intent(&entry.path())?.ok_or(Error::Invalid("Unknown neighboring deployment ownership"))?;
            if intent.complete
                || transaction::read_optional(
                    &entry
                        .path()
                        .parent()
                        .ok_or(Error::Invalid("Missing cloud parent"))?
                        .join(".allocations")
                        .join(intent.allocation.to_string())
                        .join("transaction.json"),
                )?
                .is_some()
            {
                transaction::inspect(&entry.path(), intent.allocation, &intent.identity, intent.controller)?
                    .deployment()
            } else {
                super::records(&intent)?.deployment()
            }
        };
        if !owned.is_disjoint(&worker_ids(&other)) {
            return Err(Error::Invalid(
                "Another legacy project references the same provider worker",
            ));
        }
    }
    Ok(())
}

fn worker_ids(deployment: &Deployment) -> std::collections::BTreeSet<&str> {
    let mut ids = std::collections::BTreeSet::new();
    if let Some(worker) = &deployment.worker {
        ids.insert(worker.id.as_str());
    }
    match &deployment.operation {
        horizon_cloud::CreateState::Bound { worker_id } | horizon_cloud::CreateState::Terminated { worker_id } => {
            ids.insert(worker_id.as_str());
        }
        horizon_cloud::CreateState::Prepared | horizon_cloud::CreateState::Requested => {}
    }
    ids
}

pub(super) fn validate_update(root: &Path, previous: &Deployment, next: &Deployment) -> Result<()> {
    let owned = worker_ids(previous);
    if !owned.is_empty() && owned != worker_ids(next) {
        return Err(Error::Invalid(
            "A migrated allocation cannot change provider worker identity",
        ));
    }
    capture(root, next)?;
    reject_duplicate_workers(root, next)
}

pub(super) fn validate_retained(
    root: &Path,
    deployment: &Deployment,
    retained: &BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    let current = capture(root, deployment)?;
    if retained.iter().any(|(name, bytes)| {
        !current.contains_key(name) || (name.starts_with("known-hosts-") && current.get(name) != Some(bytes))
    }) {
        return Err(Error::Invalid(
            "A retained storage or SSH companion is missing or its trust changed",
        ));
    }
    Ok(())
}
