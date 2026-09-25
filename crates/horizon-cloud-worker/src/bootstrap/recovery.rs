use super::{
    keys,
    runtime::{Runtime, Source},
    store::{LIMIT, Store, invalid},
};
use horizon_cloud_protocol::{
    bootstrap::{BootstrapOutcome, BootstrapReceipt, RecoveryPayload, RecoveryReceipt, RecoveryRequest, Startup},
    membership::Manifest,
    signed::{Action, SignedIntent, Target},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    path::Path,
};

pub(super) const ROOT: &str = "/workspace/.horizon-allocation";
pub(super) const BOOTSTRAP: &str = "bootstrap.json";
pub(super) const MANIFEST: &str = "membership.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Phase {
    Initializing,
    Initialized,
    Abandoned,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Bootstrap {
    pub version: u32,
    pub startup: Startup,
    pub worker_id: String,
    pub phase: Phase,
    pub recovery: Option<RecoveryReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initialization: Option<BootstrapReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_hash: Option<[u8; 32]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abandonment: Option<BootstrapReceipt>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Boundary {
    Receipt,
    Manifest,
    Initialized,
}

pub(super) fn run() -> io::Result<()> {
    if std::env::args().len() != 2 {
        return Err(invalid());
    }
    let request = read_request(io::stdin().lock())?;
    let store = Store::open(Path::new(ROOT))?;
    let bootstrap: Bootstrap = decode(&store.read(BOOTSTRAP)?.ok_or_else(invalid)?)?;
    let runtime = Runtime::load(bootstrap.version)?;
    let receipt = recover(&store, &runtime, &request, &mut |_| Ok(()))?;
    serde_json::to_writer(io::stdout().lock(), &receipt)?;
    io::stdout().lock().write_all(b"\n")
}

pub(super) fn read_request(reader: impl Read) -> io::Result<RecoveryRequest> {
    let mut input = Vec::new();
    reader.take(LIMIT + 1).read_to_end(&mut input)?;
    if input.len() as u64 > LIMIT {
        return Err(invalid());
    }
    decode(&input)
}

pub(super) fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes).map_err(|_| invalid())
}

pub(super) fn recover(
    store: &Store,
    runtime: &Runtime,
    request: &RecoveryRequest,
    checkpoint: &mut impl FnMut(Boundary) -> io::Result<()>,
) -> io::Result<RecoveryReceipt> {
    let mut encoded = store.read(BOOTSTRAP)?.ok_or_else(invalid)?;
    let mut bootstrap: Bootstrap = decode(&encoded)?;
    bootstrap.validate(store, runtime)?;
    if bootstrap.phase == Phase::Abandoned {
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
    let expected = Manifest::empty(bootstrap.startup.clone(), bootstrap.worker_id.clone());
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
    bootstrap.validate(store, runtime)?;
    if bootstrap.phase == Phase::Initializing {
        if store.read(BOOTSTRAP)?.as_deref() != Some(&encoded) {
            return Err(invalid());
        }
        if manifest.is_none() {
            store.write(MANIFEST, None, &serde_json::to_vec(&expected)?)?;
        }
        checkpoint(Boundary::Manifest)?;
        bootstrap.validate(store, runtime)?;
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
    bootstrap.validate(store, runtime)?;
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

impl Bootstrap {
    pub fn validate(&self, store: &Store, runtime: &Runtime) -> io::Result<()> {
        self.startup.validate().map_err(|_| invalid())?;
        if self.startup != runtime.startup
            || self.worker_id != runtime.worker_id
            || !horizon_cloud::valid_id(&self.worker_id)
            || self.startup.volume_id != runtime.volume_id
            || self.startup.data_center_id != runtime.data_center_id
            || self.startup.worker_operation != runtime.worker_operation
        {
            return Err(invalid());
        }
        if let Some(receipt) = &self.recovery
            && (receipt.version != 1 || receipt.startup != self.startup || receipt.worker_id != self.worker_id)
        {
            return Err(invalid());
        }
        match self.version {
            1 if runtime.source == Source::LegacyEnvironment
                && self.initialization.is_none()
                && self.host_key.is_none()
                && self.key_hash.is_none()
                && self.abandonment.is_none()
                && self.phase != Phase::Abandoned =>
            {
                Ok(())
            }
            2 if runtime.source == Source::StartupCapture => {
                let receipt = self.initialization.as_ref().ok_or_else(invalid)?;
                self.validate_receipt(receipt, BootstrapOutcome::Initializing)?;
                let key = store.host_key()?;
                if self.key_hash != Some(keys::hash(&key)) {
                    return Err(invalid());
                }
                if self.host_key.as_deref() != Some(keys::public(&key)?.as_str()) {
                    return Err(invalid());
                }
                match (&self.phase, &self.abandonment) {
                    (Phase::Abandoned, Some(receipt)) => self.validate_receipt(receipt, BootstrapOutcome::Abandoned),
                    (Phase::Abandoned, None) | (_, Some(_)) => Err(invalid()),
                    _ => Ok(()),
                }
            }
            _ => Err(invalid()),
        }
    }

    fn validate_receipt(&self, receipt: &BootstrapReceipt, outcome: BootstrapOutcome) -> io::Result<()> {
        if receipt.version != 1
            || receipt.startup != self.startup
            || receipt.worker_id != self.worker_id
            || receipt.outcome != outcome
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub fn empty(&self, store: &Store) -> io::Result<()> {
        let expected = Manifest::empty(self.startup.clone(), self.worker_id.clone());
        match store.read(MANIFEST)? {
            Some(bytes) if self.recovery.is_some() && decode::<Manifest>(&bytes)? == expected => Ok(()),
            None if self.phase == Phase::Initializing || self.phase == Phase::Abandoned => Ok(()),
            _ => Err(invalid()),
        }
    }
}

#[cfg(test)]
mod tests;
