//! Private project trees are acknowledged only after durable publication. They
//! contain no source, credentials or processes, and cancellation retains them.
mod storage;
use super::store::{Store, invalid, same};
use horizon_cloud_protocol::{
    bootstrap::Startup,
    membership::{Manifest, Receipt, State},
};
use serde::{Deserialize, Serialize};
use std::{fs::File, io, os::unix::fs::MetadataExt};
use storage::{Directories, Location, directory};

const CHILDREN: &[&str] = &["repository", "worktrees", "homes", "runtime", "logs", "tools"];
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    version: u32,
    startup: Startup,
    preparation: Option<Receipt>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Boundary {
    Created,
    Anchored,
    Child,
    Populated,
    Published,
    Synced,
}

fn container() -> Location {
    Location {
        record: "namespaces.json".into(),
        staging: ".namespaces.next".into(),
        destination: "projects".into(),
    }
}
fn project(receipt: &Receipt) -> Location {
    let id = receipt.identity.project_id();
    Location {
        record: format!("namespace-{id}.json"),
        staging: format!(".namespace-{}.next", receipt.operation),
        destination: id.to_string(),
    }
}
fn owner(manifest: &Manifest, preparation: Option<Receipt>) -> Owner {
    Owner {
        version: 1,
        startup: manifest.startup.clone(),
        preparation,
    }
}
fn verify(store: &Store, workspace: &File, allocation: &File) -> io::Result<()> {
    let (current, root) = store.namespace_anchors()?;
    let old = workspace.metadata()?;
    let new = current.metadata()?;
    if (old.dev(), old.ino()) != (new.dev(), new.ino()) {
        return Err(invalid());
    }
    same(allocation, &root)
}
fn preparation<'a>(manifest: &'a Manifest, identity: &horizon_cloud_protocol::ProjectIdentity) -> Option<&'a Receipt> {
    manifest
        .operations
        .iter()
        .find(|entry| entry.receipt.identity == *identity && entry.receipt.state == State::Preparing)
        .map(|entry| &entry.receipt)
}

pub(super) fn ensure(
    store: &Store,
    manifest: &Manifest,
    receipt: &Receipt,
    checkpoint: &mut impl FnMut(Boundary) -> io::Result<()>,
) -> io::Result<()> {
    if receipt.state != State::Preparing
        || preparation(manifest, &receipt.identity) != Some(receipt)
        || !manifest.members.iter().any(|member| {
            member.identity == receipt.identity && matches!(member.state, State::Preparing | State::Importing)
        })
    {
        return Err(invalid());
    }
    if manifest
        .members
        .iter()
        .any(|member| member.identity == receipt.identity && member.state == State::Importing)
    {
        return require_settled(store, manifest, &receipt.identity);
    }
    let (workspace, allocation) = store.namespace_anchors()?;
    let projects = Directories {
        store,
        allocation: &allocation,
        parent: &workspace,
    }
    .ensure(&container(), &owner(manifest, None), &[], &mut |boundary| {
        checkpoint(boundary)?;
        verify(store, &workspace, &allocation)
    })?;
    Directories {
        store,
        allocation: &allocation,
        parent: &projects,
    }
    .ensure(
        &project(receipt),
        &owner(manifest, Some(receipt.clone())),
        CHILDREN,
        &mut |boundary| {
            checkpoint(boundary)?;
            verify(store, &workspace, &allocation)?;
            same(&projects, &directory(&workspace, "projects")?)
        },
    )?;
    verify(store, &workspace, &allocation)?;
    same(&projects, &directory(&workspace, "projects")?)
}

/// Partial anchored staging is allowed at startup for explicit reconciliation;
/// missing ownership evidence or changed published trees are never adopted.
pub(super) fn validate(store: &Store, manifest: &Manifest) -> io::Result<()> {
    validate_with(store, manifest, || Ok(()))
}

pub(super) fn validate_with(
    store: &Store,
    manifest: &Manifest,
    checkpoint: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let prepared: Vec<_> = manifest
        .operations
        .iter()
        .filter(|entry| entry.receipt.state == State::Preparing)
        .collect();
    if prepared.is_empty() {
        return Ok(());
    }
    let (workspace, allocation) = store.namespace_anchors()?;
    let projects = Directories {
        store,
        allocation: &allocation,
        parent: &workspace,
    }
    .validate(&container(), &owner(manifest, None), &[], false)?;
    for entry in prepared {
        let location = project(&entry.receipt);
        let settled = manifest
            .members
            .iter()
            .any(|member| member.identity == entry.receipt.identity && member.state == State::Removed);
        match &projects {
            Some(projects) => {
                Directories {
                    store,
                    allocation: &allocation,
                    parent: projects,
                }
                .validate(
                    &location,
                    &owner(manifest, Some(entry.receipt.clone())),
                    CHILDREN,
                    settled,
                )?;
            }
            None if settled || store.read(&location.record)?.is_some() => return Err(invalid()),
            None => {}
        }
    }
    checkpoint()?;
    let current = Directories {
        store,
        allocation: &allocation,
        parent: &workspace,
    }
    .validate(&container(), &owner(manifest, None), &[], false)?;
    match (&projects, current) {
        (Some(saved), Some(current)) => same(saved, &current)?,
        (None, None) => {}
        _ => return Err(invalid()),
    }
    verify(store, &workspace, &allocation)
}

pub(super) fn require_settled(
    store: &Store,
    manifest: &Manifest,
    identity: &horizon_cloud_protocol::ProjectIdentity,
) -> io::Result<()> {
    let Some(receipt) = preparation(manifest, identity) else {
        return Ok(());
    };
    let (workspace, allocation) = store.namespace_anchors()?;
    let projects = Directories {
        store,
        allocation: &allocation,
        parent: &workspace,
    }
    .validate(&container(), &owner(manifest, None), &[], true)?
    .ok_or_else(invalid)?;
    Directories {
        store,
        allocation: &allocation,
        parent: &projects,
    }
    .validate(
        &project(receipt),
        &owner(manifest, Some(receipt.clone())),
        CHILDREN,
        true,
    )?;
    verify(store, &workspace, &allocation)?;
    same(&projects, &directory(&workspace, "projects")?)
}

/// Open only a published project's repository parent. Callers retain this handle
/// and re-open through this function before acknowledging filesystem effects.
pub(super) fn repository(
    store: &Store,
    manifest: &Manifest,
    identity: &horizon_cloud_protocol::ProjectIdentity,
) -> io::Result<File> {
    require_settled(store, manifest, identity)?;
    let receipt = preparation(manifest, identity).ok_or_else(invalid)?;
    let (workspace, allocation) = store.namespace_anchors()?;
    let projects = directory(&workspace, "projects")?;
    let project = Directories {
        store,
        allocation: &allocation,
        parent: &projects,
    }
    .validate(
        &project(receipt),
        &owner(manifest, Some(receipt.clone())),
        CHILDREN,
        true,
    )?
    .ok_or_else(invalid)?;
    let repository = directory(&project, "repository")?;
    require_settled(store, manifest, identity)?;
    same(&projects, &directory(&workspace, "projects")?)?;
    same(&project, &directory(&projects, &identity.project_id().to_string())?)?;
    Ok(repository)
}
