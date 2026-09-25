//! Reservation mutations retain the allocation lock across authentication, probe
//! and durable publication. They never create or remove project resources.
use super::{
    inspection,
    recovery::{BOOTSTRAP, Bootstrap, MANIFEST, Phase, ROOT, decode, read_request},
    runtime::Runtime,
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

pub(super) fn run(cancel: bool) -> io::Result<()> {
    if std::env::args().len() != 2 {
        return Err(invalid());
    }
    let request = read_request(io::stdin().lock())?;
    let store = Store::open(Path::new(ROOT))?;
    let runtime = Runtime::captured()?;
    let action = if cancel {
        Action::RemoveProject
    } else {
        Action::AttachProject
    };
    let receipt = mutate(&store, &runtime, &request, action, inspection::probe, &mut |_| Ok(()))?;
    serde_json::to_writer(io::stdout().lock(), &receipt)?;
    io::stdout().lock().write_all(b"\n")
}

pub(super) fn startup(store: &Store, bootstrap: &Bootstrap) -> io::Result<()> {
    if bootstrap.phase != Phase::Initialized {
        return bootstrap.empty(store);
    }
    load(store, bootstrap).map(|_| ())
}

fn load(store: &Store, bootstrap: &Bootstrap) -> io::Result<(Vec<u8>, Manifest)> {
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
    let verify = || -> io::Result<()> {
        bootstrap.validate(store, runtime)?;
        if store.read(BOOTSTRAP)?.as_deref() != Some(&encoded) {
            return Err(invalid());
        }
        Ok(())
    };
    let next_bytes = if next.revision == manifest.revision {
        original.clone()
    } else {
        if let Request::Reserve { capabilities, .. } = &payload {
            probe(capabilities)?;
        }
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
    Ok(receipt)
}
