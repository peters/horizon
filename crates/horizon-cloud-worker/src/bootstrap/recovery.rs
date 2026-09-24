use super::store::{LIMIT, Store, invalid};
use horizon_cloud_protocol::{
    bootstrap::{RecoveryPayload, RecoveryReceipt, RecoveryRequest, Startup},
    signed::{Action, SignedIntent, Target},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    path::Path,
};

const ROOT: &str = "/workspace/.horizon-allocation";
const BOOTSTRAP: &str = "bootstrap.json";
const MANIFEST: &str = "membership.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Initializing,
    Initialized,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bootstrap {
    version: u32,
    startup: Startup,
    worker_id: String,
    phase: Phase,
    recovery: Option<RecoveryReceipt>,
}

/// This entry point only understands the pre-admission manifest. Later membership
/// must add full validation before this recovery path can accept admitted projects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    startup: Startup,
    worker_id: String,
    revision: u64,
    members: Vec<serde_json::Value>,
}

#[derive(Clone)]
struct Runtime {
    startup: Startup,
    worker_id: String,
    volume_id: String,
    data_center_id: String,
    worker_operation: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Boundary {
    Receipt,
    Manifest,
    Initialized,
}

pub(super) fn run() -> io::Result<()> {
    if std::env::args().len() != 2 {
        return Err(invalid());
    }
    // Presence of a real workspace mount is necessary, never freshness proof.
    let mounts = std::fs::read_to_string("/proc/self/mountinfo")?;
    if !mounts
        .lines()
        .any(|line| line.split_whitespace().nth(4) == Some("/workspace"))
    {
        return Err(invalid());
    }
    let metadata = variable("HORIZON_WORKER_STARTUP")?;
    if metadata.len() > 8192 {
        return Err(invalid());
    }
    let runtime = Runtime {
        startup: decode(metadata.as_bytes())?,
        worker_id: variable("RUNPOD_POD_ID")?,
        volume_id: variable("RUNPOD_VOLUME_ID")?,
        data_center_id: variable("RUNPOD_DC_ID")?,
        worker_operation: variable("HORIZON_CLOUD_OPERATION")?,
    };
    let request = read_request(io::stdin().lock())?;
    let store = Store::open(Path::new(ROOT))?;
    let receipt = recover(&store, &runtime, &request, &mut |_| Ok(()))?;
    serde_json::to_writer(io::stdout().lock(), &receipt)?;
    io::stdout().lock().write_all(b"\n")
}

fn read_request(reader: impl Read) -> io::Result<RecoveryRequest> {
    let mut input = Vec::new();
    reader.take(LIMIT + 1).read_to_end(&mut input)?;
    if input.len() as u64 > LIMIT {
        return Err(invalid());
    }
    decode(&input)
}

fn variable(name: &str) -> io::Result<String> {
    std::env::var(name).map_err(|_| invalid())
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes).map_err(|_| invalid())
}

fn recover(
    store: &Store,
    runtime: &Runtime,
    request: &RecoveryRequest,
    checkpoint: &mut impl FnMut(Boundary) -> io::Result<()>,
) -> io::Result<RecoveryReceipt> {
    let mut encoded = store.read(BOOTSTRAP)?.ok_or_else(invalid)?;
    let mut bootstrap: Bootstrap = decode(&encoded)?;
    bootstrap.startup.validate().map_err(|_| invalid())?;
    if bootstrap.version != 1
        || bootstrap.startup != runtime.startup
        || !horizon_cloud::valid_id(&bootstrap.worker_id)
        || bootstrap.worker_id != runtime.worker_id
        || bootstrap.startup.volume_id != runtime.volume_id
        || bootstrap.startup.data_center_id != runtime.data_center_id
        || bootstrap.startup.worker_operation != runtime.worker_operation
    {
        return Err(invalid());
    }
    let message = SignedIntent::parse(request.message.as_bytes()).map_err(|_| invalid())?;
    let intent = message
        .verify(&bootstrap.startup.controller, request.payload.as_bytes())
        .map_err(|_| invalid())?;
    if intent.action() != Action::Bootstrap
        || intent.expected_revision() != 0
        || *intent.target() != (Target::Allocation {})
    {
        return Err(invalid());
    }
    let RecoveryPayload::Recover { token } = decode(request.payload.as_bytes())?;
    if token != bootstrap.startup.token {
        return Err(invalid());
    }
    let receipt = RecoveryReceipt {
        version: 1,
        startup: bootstrap.startup.clone(),
        worker_id: bootstrap.worker_id.clone(),
        operation: intent.operation(),
        fingerprint: intent.fingerprint().map_err(|_| invalid())?,
    };
    if bootstrap.recovery.as_ref().is_some_and(|saved| saved != &receipt) {
        return Err(invalid());
    }
    let expected = Manifest {
        version: 1,
        startup: bootstrap.startup.clone(),
        worker_id: bootstrap.worker_id.clone(),
        revision: 0,
        members: Vec::new(),
    };
    let manifest = store.read(MANIFEST)?;
    if bootstrap.recovery.is_none() && (bootstrap.phase != Phase::Initializing || manifest.is_some()) {
        return Err(invalid());
    }
    match manifest.as_deref() {
        Some(bytes) if decode::<Manifest>(bytes)? != expected => return Err(invalid()),
        None if bootstrap.phase == Phase::Initialized => return Err(invalid()),
        _ => {}
    }
    // Bind the durable recovery operation before its first write to membership.
    // The host must retain this operation and exact payload after any uncertainty.
    if bootstrap.recovery.is_none() {
        bootstrap.recovery = Some(receipt.clone());
        let next = serde_json::to_vec(&bootstrap)?;
        store.write(BOOTSTRAP, Some(&encoded), &next)?;
        encoded = next;
    }
    checkpoint(Boundary::Receipt)?;
    store.sync(BOOTSTRAP)?;
    if store.read(BOOTSTRAP)?.as_deref() != Some(&encoded) {
        return Err(invalid());
    }
    if bootstrap.phase == Phase::Initializing {
        if store.read(BOOTSTRAP)?.as_deref() != Some(&encoded) {
            return Err(invalid());
        }
        if manifest.is_none() {
            store.write(MANIFEST, None, &serde_json::to_vec(&expected)?)?;
        }
        checkpoint(Boundary::Manifest)?;
        // Never commit initialized after a concurrent or injected artifact change.
        verify_manifest(store, &expected)?;
        store.sync(MANIFEST)?;
        verify_manifest(store, &expected)?;
        bootstrap.phase = Phase::Initialized;
        let next = serde_json::to_vec(&bootstrap)?;
        store.write(BOOTSTRAP, Some(&encoded), &next)?;
        encoded = next;
    }
    checkpoint(Boundary::Initialized)?;
    verify_manifest(store, &expected)?;
    store.sync(MANIFEST)?;
    store.sync(BOOTSTRAP)?;
    verify_manifest(store, &expected)?;
    if store.read(BOOTSTRAP)?.as_deref() != Some(&encoded) {
        return Err(invalid());
    }
    Ok(receipt)
}

fn verify_manifest(store: &Store, expected: &Manifest) -> io::Result<()> {
    let bytes = store.read(MANIFEST)?.ok_or_else(invalid)?;
    if decode::<Manifest>(&bytes)? != *expected {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
