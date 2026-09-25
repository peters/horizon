//! Read-only qualification of a retained allocation before future admission.
use super::{
    Cancellation, Connection, Duration, Error, Instant, OperationId, Owner, Phase, Record, Request, Result, RunPod,
    Snapshot, bindings, bootstrap_recovery, inspect_target, remaining, runner,
};
use horizon_cloud::Capabilities;
use horizon_cloud_protocol::{
    bootstrap::RecoveryRequest,
    inspection::{Receipt, Request as InspectionRequest},
    signed::{Action, Intent, Target},
};

/// Verify selected capabilities on an initialized empty allocation. The returned
/// observation grants no admission or deletion authority and is never cached as
/// such. Future admission must recheck compatibility under the worker lock.
/// The immutable image must be the prospective project's qualified image.
/// # Errors
/// Rejects changed ownership, image, account, pins, runtime or capabilities. This
/// performs provider reads and pinned SSH only; it never creates or deletes resources.
pub fn inspect(
    owner: &Owner,
    request: &Request,
    image_digest: &str,
    capabilities: &Capabilities,
    cancel: &Cancellation,
    timeout: Duration,
) -> Result<Receipt> {
    let deadline = Instant::now() + timeout.min(Duration::from_secs(180));
    remaining(deadline)?;
    let runner = runner(cancel);
    let (account, credential, identity) = bindings(request, &runner)?;
    let record = Record::load(owner)?.ok_or(Error::Invalid)?;
    record.verify(owner, request, &account, &identity)?;
    if record.phase != Phase::Completed || record.spec.image_digest != image_digest {
        return Err(Error::Invalid);
    }
    let target = inspect_target(&RunPod::new(credential), &record, cancel)?;
    inspect_with(
        owner,
        &target,
        capabilities,
        deadline,
        &mut |connection, bytes, timeout| {
            Ok(runner.private_exchange(
                &mut connection.pinned_command("horizon-cloud-worker inspect-allocation"),
                bytes,
                timeout,
            )?)
        },
    )
}

pub(super) fn inspect_with(
    owner: &Owner,
    target: &bootstrap_recovery::Target,
    capabilities: &Capabilities,
    deadline: Instant,
    exchange: &mut impl FnMut(&Connection, &[u8], Duration) -> Result<Vec<u8>>,
) -> Result<Receipt> {
    remaining(deadline)?;
    let snapshot = Snapshot::capture(target)?;
    if bootstrap_recovery::require_existing(owner, target)? != snapshot.binding {
        return Err(Error::Invalid);
    }
    let payload = serde_json::to_string(&InspectionRequest {
        capabilities: capabilities.clone(),
    })
    .map_err(|_| Error::Invalid)?;
    let intent = Intent::new(
        &target.startup.controller,
        OperationId::generate(),
        0,
        Target::Allocation {},
        Action::InspectAllocation,
        payload.as_bytes(),
    )
    .map_err(|_| Error::Invalid)?;
    let expected = Receipt {
        version: 1,
        startup: target.startup.clone(),
        worker_id: target.worker_id.clone(),
        operation: intent.operation(),
        fingerprint: intent.fingerprint().map_err(|_| Error::Invalid)?,
        revision: 0,
        capabilities: capabilities.clone(),
    };
    let message = serde_json::to_string(&owner.sign(intent)?).map_err(|_| Error::Invalid)?;
    let bytes = serde_json::to_vec(&RecoveryRequest { message, payload }).map_err(|_| Error::Invalid)?;
    if bytes.len() > 64 * 1024 {
        return Err(Error::Invalid);
    }
    let reply = exchange(&snapshot.connection, &bytes, remaining(deadline)?)?;
    if reply.len() > 64 * 1024 || serde_json::from_slice::<Receipt>(&reply).map_err(|_| Error::Invalid)? != expected {
        return Err(Error::Invalid);
    }
    owner.load()?;
    Ok(expected)
}
