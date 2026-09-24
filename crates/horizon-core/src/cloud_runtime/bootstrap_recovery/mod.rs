//! Anchored host recovery of an existing worker bootstrap, never initialization.
mod connection;

use super::{Cancellation, command::Runner, owner::Owner, ssh::Connection};
use connection::{Binding, Snapshot};
use horizon_cloud_protocol::{
    OperationId,
    bootstrap::{RecoveryPayload, RecoveryReceipt, RecoveryRequest, Startup},
    signed::{Action, Intent, SignedIntent, Target as IntentTarget},
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const KEY: &str = "bootstrap_recovery";
const LIMIT: usize = 64 * 1024;
type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Bootstrap recovery context, SSH pin or receipt is invalid or changed")]
    Invalid,
    #[error(transparent)]
    Owner(#[from] super::owner::Error),
    #[error(transparent)]
    Transport(#[from] super::Error),
    #[error("Bootstrap recovery identity file is unavailable")]
    Io(#[from] std::io::Error),
}

/// Identifies retained state; it grants no permission to initialize absent state.
/// The caller must validate local allocation ownership before requesting recovery.
pub struct Target {
    pub startup: Startup,
    pub worker_id: String,
    pub connection: Connection,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    binding: Binding,
    request: String,
    receipt: RecoveryReceipt,
    completed: bool,
}

/// Reuse one anchored signed request and verify the worker's complete receipt.
/// The Owner retains its canonical lock throughout transport and completion.
/// Even a completed retry contacts the worker; cached receipts do not establish
/// that remote membership still exists. No provider operations are performed.
///
/// # Errors
/// Missing or changed SSH pins, uncertain journals and absent worker state block
/// recovery. A lost reply or completion-save failure retains the same operation.
/// Requires OpenSSH `ssh`/`ssh-keygen` and a valid unencrypted private identity;
/// encrypted or agent-only identities are rejected before anchoring the request.
pub fn recover(
    owner: &mut Owner,
    target: &Target,
    cancel: &Cancellation,
    timeout: Duration,
) -> Result<RecoveryReceipt> {
    cancel.check().map_err(super::Error::from)?;
    if timeout.is_zero() {
        return Err(Error::Invalid);
    }
    let runner = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    recover_with(owner, target, &mut |connection, request| {
        Ok(runner.private_exchange(
            &mut connection.pinned_command("horizon-cloud-worker recover-allocation"),
            request,
            timeout.min(Duration::from_secs(60)),
        )?)
    })
}

fn recover_with(
    owner: &mut Owner,
    target: &Target,
    exchange: &mut impl FnMut(&Connection, &[u8]) -> Result<Vec<u8>>,
) -> Result<RecoveryReceipt> {
    let binding = Binding::capture(target)?;
    if owner.binding()? != target.startup.controller {
        return Err(Error::Invalid);
    }
    let mut payload = owner.load()?;
    let object = payload.as_object_mut().ok_or(Error::Invalid)?;
    let mut record = if let Some(saved) = object.get(KEY) {
        serde_json::from_value::<Record>(saved.clone()).map_err(|_| Error::Invalid)?
    } else {
        let record = Record::new(owner, binding.clone())?;
        object.insert(KEY.into(), serde_json::to_value(&record).map_err(|_| Error::Invalid)?);
        owner.save(payload.clone())?;
        record
    };
    record.verify(&binding)?;
    // Recheck both local authorities immediately before sending the anchored bytes.
    let snapshot = Snapshot::capture(target)?;
    if owner.binding()? != target.startup.controller || snapshot.binding != binding {
        return Err(Error::Invalid);
    }
    let bytes = exchange(&snapshot.connection, record.request.as_bytes())?;
    if bytes.len() > LIMIT {
        return Err(Error::Invalid);
    }
    let receipt: RecoveryReceipt = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
    if receipt != record.receipt {
        return Err(Error::Invalid);
    }
    if record.completed {
        // A previously completed receipt never bypasses native anchor validation.
        owner.load()?;
    } else {
        record.completed = true;
        payload
            .as_object_mut()
            .ok_or(Error::Invalid)?
            .insert(KEY.into(), serde_json::to_value(&record).map_err(|_| Error::Invalid)?);
        owner.save(payload)?;
    }
    Ok(receipt)
}

impl Record {
    fn new(owner: &Owner, binding: Binding) -> Result<Self> {
        let payload = serde_json::to_string(&RecoveryPayload::Recover {
            token: binding.startup.token,
        })
        .map_err(|_| Error::Invalid)?;
        let intent = Intent::new(
            &binding.startup.controller,
            OperationId::generate(),
            0,
            IntentTarget::Allocation {},
            Action::Bootstrap,
            payload.as_bytes(),
        )
        .map_err(|_| Error::Invalid)?;
        let receipt = RecoveryReceipt {
            version: 1,
            startup: binding.startup.clone(),
            worker_id: binding.worker_id.clone(),
            operation: intent.operation(),
            fingerprint: intent.fingerprint().map_err(|_| Error::Invalid)?,
        };
        let message = serde_json::to_string(&owner.sign(intent)?).map_err(|_| Error::Invalid)?;
        let request = serde_json::to_string(&RecoveryRequest { message, payload }).map_err(|_| Error::Invalid)?;
        Ok(Self {
            version: 1,
            binding,
            request,
            receipt,
            completed: false,
        })
    }

    fn verify(&self, binding: &Binding) -> Result<()> {
        if self.version != 1 || self.binding != *binding || self.request.len() > LIMIT {
            return Err(Error::Invalid);
        }
        let request: RecoveryRequest = serde_json::from_str(&self.request).map_err(|_| Error::Invalid)?;
        let signed = SignedIntent::parse(request.message.as_bytes()).map_err(|_| Error::Invalid)?;
        let intent = signed
            .verify(&binding.startup.controller, request.payload.as_bytes())
            .map_err(|_| Error::Invalid)?;
        let payload: RecoveryPayload = serde_json::from_str(&request.payload).map_err(|_| Error::Invalid)?;
        if intent.action() != Action::Bootstrap
            || intent.expected_revision() != 0
            || *intent.target() != (IntentTarget::Allocation {})
            || payload
                != (RecoveryPayload::Recover {
                    token: binding.startup.token,
                })
            || self.receipt
                != (RecoveryReceipt {
                    version: 1,
                    startup: binding.startup.clone(),
                    worker_id: binding.worker_id.clone(),
                    operation: intent.operation(),
                    fingerprint: intent.fingerprint().map_err(|_| Error::Invalid)?,
                })
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests;
