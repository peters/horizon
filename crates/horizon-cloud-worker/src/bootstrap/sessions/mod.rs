//! Session preparation publishes writable data exactly once. Recovery consults
//! external ownership records, never mutable checkout or home configuration.
mod storage;
use super::{
    namespaces, source,
    store::{Store, invalid, same},
};
use horizon_cloud_protocol::{
    ProjectIdentity,
    membership::{Manifest, Receipt, Request, Source, State},
};
use std::{io, time::Instant};
pub(super) use storage::Boundary;
use storage::Tree;

pub(super) fn ensure(
    store: &Store,
    manifest: &Manifest,
    receipt: &Receipt,
    session_id: horizon_cloud_protocol::membership::SessionId,
    deadline: Instant,
    checkpoint: &mut impl FnMut(Boundary) -> io::Result<()>,
) -> io::Result<()> {
    let session = manifest
        .members
        .iter()
        .find(|member| member.identity == receipt.identity && member.state == State::Importing)
        .and_then(|member| member.sessions.iter().find(|session| session.id == session_id))
        .ok_or_else(invalid)?;
    if !entries(manifest)?
        .iter()
        .any(|(saved, id)| *saved == receipt && *id == session_id)
    {
        return Err(invalid());
    }
    let parent = namespaces::child(store, manifest, &receipt.identity, "worktrees")?;
    let source = source::published(store, manifest, &receipt.identity, deadline)?;
    let verify = || {
        source::remaining(deadline)?;
        same(
            &parent,
            &namespaces::child(store, manifest, &receipt.identity, "worktrees")?,
        )?;
        same(
            &source,
            &source::published(store, manifest, &receipt.identity, deadline)?,
        )
    };
    let mut tree = Tree::open(store, &parent, receipt, session_id, true, deadline)?.ok_or_else(invalid)?;
    tree.prepare(&source, &session.revision, checkpoint, &verify)?;
    verify()
}

fn entries(manifest: &Manifest) -> io::Result<Vec<(&Receipt, horizon_cloud_protocol::membership::SessionId)>> {
    manifest
        .operations
        .iter()
        .filter_map(|entry| match serde_json::from_str::<Request>(&entry.payload) {
            Ok(Request::PrepareSession { session_id }) => Some(Ok((&entry.receipt, session_id))),
            Ok(_) => None,
            Err(_) => Some(Err(invalid())),
        })
        .collect()
}

/// Startup permits absent attempts and anchored staging. Cancellation requires
/// settled publication; uncertain partial work remains retained and fenced.
pub(super) fn validate(store: &Store, manifest: &Manifest, cancelling: Option<&ProjectIdentity>) -> io::Result<()> {
    let deadline = Instant::now() + Source::WORKER_TIMEOUT;
    for (receipt, session_id) in entries(manifest)? {
        let settled = cancelling == Some(&receipt.identity)
            || manifest
                .members
                .iter()
                .any(|member| member.identity == receipt.identity && member.state == State::Removed);
        let parent = namespaces::child(store, manifest, &receipt.identity, "worktrees")?;
        match Tree::open(store, &parent, receipt, session_id, false, deadline)? {
            Some(tree) => tree.validate(settled)?,
            None if settled => return Err(invalid()),
            None => {}
        }
        same(
            &parent,
            &namespaces::child(store, manifest, &receipt.identity, "worktrees")?,
        )?;
    }
    Ok(())
}
