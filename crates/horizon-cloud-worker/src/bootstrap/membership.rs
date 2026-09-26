//! Reservation mutations retain the allocation lock across authentication, probe
//! and durable publication. Namespace preparation creates only private directories.
use super::{
    inspection, namespaces,
    recovery::{BOOTSTRAP, Bootstrap, MANIFEST, Phase, ROOT, decode, read_request},
    runtime::Runtime,
    session_runtime, sessions, source,
    store::{Publication, Store, invalid},
};
use horizon_cloud::Capabilities;
use horizon_cloud_protocol::{
    bootstrap::RecoveryRequest,
    membership::{Manifest, Receipt, Request},
    signed::Action,
};
use std::{
    io::{self, Write},
    path::Path,
};

pub(super) fn run(action: Action) -> io::Result<()> {
    if std::env::args().len() != 2 {
        return Err(invalid());
    }
    let request = read_request(io::stdin().lock())?;
    let store = if matches!(action, Action::StartProjectSession | Action::StopProjectSession) {
        session_runtime::open()?
    } else {
        Store::open(Path::new(ROOT))?
    };
    let runtime = Runtime::captured()?;
    let receipt = mutate(&store, &runtime, &request, action, inspection::probe, &mut |_| Ok(()))?;
    serde_json::to_writer(io::stdout().lock(), &receipt)?;
    io::stdout().lock().write_all(b"\n")
}

pub(super) fn startup(store: &Store, bootstrap: &Bootstrap) -> io::Result<()> {
    if bootstrap.phase != Phase::Initialized {
        return bootstrap.empty(store);
    }
    let (_, manifest) = load(store, bootstrap)?;
    namespaces::validate(store, &manifest)?;
    source::validate(store, &manifest, false)?;
    sessions::validate(store, &manifest, None)?;
    session_runtime::validate(store, &manifest, None)
}

pub(super) fn load(store: &Store, bootstrap: &Bootstrap) -> io::Result<(Vec<u8>, Manifest)> {
    if bootstrap.version != 2 || bootstrap.phase != Phase::Initialized || bootstrap.recovery.is_none() {
        return Err(invalid());
    }
    let bytes = store.read(MANIFEST)?.ok_or_else(invalid)?;
    let manifest: Manifest = decode(&bytes)?;
    manifest.validate().map_err(|_| invalid())?;
    if manifest.startup != bootstrap.startup || manifest.worker_id != bootstrap.worker_id {
        return Err(invalid());
    }
    if manifest
        .operations
        .iter()
        .any(|entry| reserved_operation(bootstrap, entry.receipt.operation))
    {
        return Err(invalid());
    }
    Ok((bytes, manifest))
}

fn reserved_operation(bootstrap: &Bootstrap, operation: horizon_cloud_protocol::OperationId) -> bool {
    bootstrap
        .initialization
        .as_ref()
        .is_some_and(|receipt| receipt.operation == operation)
        || bootstrap
            .recovery
            .as_ref()
            .is_some_and(|receipt| receipt.operation == operation)
        || bootstrap
            .abandonment
            .as_ref()
            .is_some_and(|receipt| receipt.operation == operation)
        || bootstrap.startup.token == operation
}

pub(super) fn mutate(
    store: &Store,
    runtime: &Runtime,
    request: &RecoveryRequest,
    action: Action,
    probe: impl FnOnce(&Capabilities) -> io::Result<()>,
    checkpoint: &mut impl FnMut(Publication) -> io::Result<()>,
) -> io::Result<Receipt> {
    mutate_with(store, runtime, request, action, probe, checkpoint, &mut |_| Ok(()))
}

pub(super) fn mutate_with(
    store: &Store,
    runtime: &Runtime,
    request: &RecoveryRequest,
    action: Action,
    probe: impl FnOnce(&Capabilities) -> io::Result<()>,
    checkpoint: &mut impl FnMut(Publication) -> io::Result<()>,
    namespace_checkpoint: &mut impl FnMut(namespaces::Boundary) -> io::Result<()>,
) -> io::Result<Receipt> {
    let source_deadline = std::time::Instant::now() + horizon_cloud_protocol::membership::Source::WORKER_TIMEOUT;
    let encoded = store.read(BOOTSTRAP)?.ok_or_else(invalid)?;
    let bootstrap: Bootstrap = decode(&encoded)?;
    bootstrap.validate(store, runtime)?;
    let (original, manifest) = load(store, &bootstrap)?;
    let payload: Request = decode(request.payload.as_bytes())?;
    if payload.action() != action {
        return Err(invalid());
    }
    let (next, receipt) = manifest
        .next(&request.message, &request.payload)
        .map_err(|_| invalid())?;
    if reserved_operation(&bootstrap, receipt.operation) {
        return Err(invalid());
    }
    session_runtime::preflight(store, &manifest, &receipt, &payload)?;
    let prepared_source = if matches!(payload, Request::PrepareSession { .. }) {
        Some(source::published(store, &manifest, &receipt.identity, source_deadline)?)
    } else {
        None
    };
    let verify = || -> io::Result<()> {
        validate_cancellation(store, &manifest, &receipt.identity, &payload)?;
        if matches!(payload, Request::ReserveSession { .. }) {
            source::require_settled_until(store, &manifest, &receipt.identity, source_deadline)?;
        }
        if let Some(source) = &prepared_source {
            source.verify(store, &manifest, source_deadline)?;
        }
        bootstrap.validate(store, runtime)?;
        if store.read(BOOTSTRAP)?.as_deref() != Some(&encoded) {
            return Err(invalid());
        }
        Ok(())
    };
    verify()?;
    if matches!(
        payload,
        Request::ImportSource { .. } | Request::ReserveSession { .. } | Request::PrepareSession { .. }
    ) {
        namespaces::repository(store, &manifest, &receipt.identity)?;
        let member = manifest
            .members
            .iter()
            .find(|m| m.identity == receipt.identity)
            .ok_or_else(invalid)?;
        probe(&member.capabilities)?;
        verify()?;
    } else if let Request::Reserve { capabilities, .. } = &payload
        && next.revision != manifest.revision
    {
        probe(capabilities)?;
    }
    let next_bytes = if next.revision == manifest.revision {
        original.clone()
    } else {
        verify()?;
        let bytes = serde_json::to_vec(&next)?;
        store.write_with(MANIFEST, Some(&original), &bytes, &mut |boundary| {
            checkpoint(boundary)?;
            verify()
        })?;
        bytes
    };
    // A previous attempt may have renamed without syncing the directory. Exact
    // retries must establish durability before acknowledging the stored receipt.
    verify()?;
    store.sync(BOOTSTRAP)?;
    store.sync(MANIFEST)?;
    verify()?;
    if store.read(MANIFEST)?.as_deref() != Some(&next_bytes) {
        return Err(invalid());
    }
    if matches!(payload, Request::PrepareNamespace {}) {
        namespaces::ensure(store, &next, &receipt, &mut |boundary| {
            namespace_checkpoint(boundary)?;
            verify()?;
            if store.read(MANIFEST)?.as_deref() != Some(&next_bytes) {
                return Err(invalid());
            }
            Ok(())
        })?;
    }
    if let Request::PrepareSession { session_id } = payload {
        sessions::ensure(
            store,
            &next,
            &receipt,
            session_id,
            prepared_source.as_ref().ok_or_else(invalid)?,
            source_deadline,
            &mut |_| verify(),
        )?;
    }
    verify()?;
    if store.read(MANIFEST)?.as_deref() != Some(&next_bytes) {
        return Err(invalid());
    }
    session_runtime::commit(store, &next, &receipt, &payload, next.revision != manifest.revision)?;
    Ok(receipt)
}

fn validate_cancellation(
    store: &Store,
    manifest: &Manifest,
    identity: &horizon_cloud_protocol::ProjectIdentity,
    payload: &Request,
) -> io::Result<()> {
    if matches!(payload, Request::Cancel {}) {
        namespaces::require_settled(store, manifest, identity)?;
        source::require_settled(store, manifest, identity)?;
        sessions::validate(store, manifest, Some(identity))?;
        session_runtime::validate(store, manifest, Some(identity))?;
    }
    Ok(())
}
