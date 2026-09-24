//! Signed first publication and irreversible pre-admission abandonment.
use super::{
    keys,
    recovery::{BOOTSTRAP, Bootstrap, Phase, ROOT, decode, read_request},
    runtime::{RUN_ROOT, Runtime, Source},
    store::{Store, invalid},
};
use horizon_cloud_protocol::{
    bootstrap::{BootstrapOutcome, BootstrapPayload, BootstrapReceipt, RecoveryRequest},
    signed::{Action, SignedIntent, Target},
};
use std::{
    io::{self, Write},
    path::Path,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Boundary {
    Root,
    Key,
    Marker,
    Abandoned,
}

pub(super) fn run(abandon: bool) -> io::Result<()> {
    if std::env::args().len() != 2 {
        return Err(invalid());
    }
    let runtime = Runtime::captured()?;
    let request = read_request(io::stdin().lock())?;
    let receipt = if abandon {
        abandon_with(&Store::open(Path::new(ROOT))?, &runtime, &request, &mut |_| Ok(()))?
    } else {
        let key = keys::captured(Path::new(RUN_ROOT))?;
        initialize(Path::new(ROOT), &runtime, &request, &key, &mut |_| Ok(()))?
    };
    serde_json::to_writer(io::stdout().lock(), &receipt)?;
    io::stdout().lock().write_all(b"\n")
}

fn verified(
    runtime: &Runtime,
    request: &RecoveryRequest,
    expected: &BootstrapPayload,
    outcome: BootstrapOutcome,
) -> io::Result<BootstrapReceipt> {
    if runtime.source != Source::StartupCapture {
        return Err(invalid());
    }
    runtime.validate_binding()?;
    let signed = SignedIntent::parse(request.message.as_bytes()).map_err(|_| invalid())?;
    let intent = signed
        .verify(&runtime.startup.controller, request.payload.as_bytes())
        .map_err(|_| invalid())?;
    if intent.action() != Action::Bootstrap
        || intent.expected_revision() != 0
        || *intent.target() != (Target::Allocation {})
        || decode::<BootstrapPayload>(request.payload.as_bytes())? != *expected
    {
        return Err(invalid());
    }
    Ok(BootstrapReceipt {
        version: 1,
        startup: runtime.startup.clone(),
        worker_id: runtime.worker_id.clone(),
        operation: intent.operation(),
        fingerprint: intent.fingerprint().map_err(|_| invalid())?,
        outcome,
    })
}

fn initialize(
    root: &Path,
    runtime: &Runtime,
    request: &RecoveryRequest,
    key: &[u8],
    checkpoint: &mut impl FnMut(Boundary) -> io::Result<()>,
) -> io::Result<BootstrapReceipt> {
    let host_key = keys::public(key)?;
    let expected = BootstrapPayload::Initialize {
        startup: runtime.startup.clone(),
        worker_id: runtime.worker_id.clone(),
        host_key: host_key.clone(),
    };
    let receipt = verified(runtime, request, &expected, BootstrapOutcome::Initializing)?;
    match std::fs::symlink_metadata(root) {
        Ok(_) => {
            let store = Store::open(root)?;
            let record: Bootstrap = decode(&store.read(BOOTSTRAP)?.ok_or_else(invalid)?)?;
            record.validate(&store, runtime)?;
            if record.version != 2
                || record.phase == Phase::Abandoned
                || record.initialization.as_ref() != Some(&receipt)
                || record.host_key.as_ref() != Some(&host_key)
            {
                return Err(invalid());
            }
            record.empty(&store)?;
            store.sync("ssh-host-key")?;
            store.sync(BOOTSTRAP)?;
            return Ok(receipt);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let store = Store::create(root)?;
    checkpoint(Boundary::Root)?;
    store.pristine_workspace()?;
    store.create_host_key(key)?;
    checkpoint(Boundary::Key)?;
    let record = Bootstrap {
        version: 2,
        startup: runtime.startup.clone(),
        worker_id: runtime.worker_id.clone(),
        phase: Phase::Initializing,
        recovery: None,
        initialization: Some(receipt.clone()),
        host_key: Some(host_key),
        key_hash: Some(keys::hash(key)),
        abandonment: None,
    };
    record.validate(&store, runtime)?;
    store.pristine_workspace()?;
    let bytes = serde_json::to_vec(&record)?;
    store.write(BOOTSTRAP, None, &bytes)?;
    checkpoint(Boundary::Marker)?;
    record.validate(&store, runtime)?;
    store.pristine_workspace()?;
    store.sync("ssh-host-key")?;
    store.sync(BOOTSTRAP)?;
    if store.read(BOOTSTRAP)?.as_deref() != Some(&bytes) {
        return Err(invalid());
    }
    Ok(receipt)
}

fn abandon_with(
    store: &Store,
    runtime: &Runtime,
    request: &RecoveryRequest,
    checkpoint: &mut impl FnMut(Boundary) -> io::Result<()>,
) -> io::Result<BootstrapReceipt> {
    let expected = BootstrapPayload::Abandon {
        startup: runtime.startup.clone(),
        worker_id: runtime.worker_id.clone(),
    };
    let receipt = verified(runtime, request, &expected, BootstrapOutcome::Abandoned)?;
    let bytes = store.read(BOOTSTRAP)?.ok_or_else(invalid)?;
    let mut record: Bootstrap = decode(&bytes)?;
    record.validate(store, runtime)?;
    if record.version != 2 || record.abandonment.as_ref().is_some_and(|saved| saved != &receipt) {
        return Err(invalid());
    }
    record.empty(store)?;
    record.phase = Phase::Abandoned;
    record.abandonment = Some(receipt.clone());
    let next = serde_json::to_vec(&record)?;
    if bytes != next {
        store.write(BOOTSTRAP, Some(&bytes), &next)?;
    }
    checkpoint(Boundary::Abandoned)?;
    record.validate(store, runtime)?;
    record.empty(store)?;
    store.sync(BOOTSTRAP)?;
    if store.read(BOOTSTRAP)?.as_deref() != Some(&next) {
        return Err(invalid());
    }
    Ok(receipt)
}

#[cfg(test)]
mod tests;
