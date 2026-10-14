//! Fixed private worker status projection and durable host enrollment ordering.
use super::receipt::{AllocationRequestId, CaptureAttempt, DeviceId, EnrollmentProvider, TailnetId, WorkerId};
use crate::cloud_runtime::{CreateState, Error, Result, command::Runner, ssh::Connection, state};
use horizon_cloud_protocol::OperationId;
use serde::Deserialize;
use std::time::Duration;

const PROJECTION: &str = include_str!("capture/projection.py");
const CAPTURE_ERROR: &str =
    "Tailnet device ID was not recorded; rebuild with the current worker and check its node manually before cleanup";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    version: u32,
    tailnet_id: TailnetId,
    device_id: DeviceId,
}

fn observed(bytes: &[u8], selected: &TailnetId) -> Result<DeviceId> {
    if bytes.len() > 1024 {
        return Err(Error::Invalid(CAPTURE_ERROR));
    }
    let report: Report = serde_json::from_slice(bytes).map_err(|_| Error::Invalid(CAPTURE_ERROR))?;
    if report.version != 1 || &report.tailnet_id != selected {
        return Err(Error::Invalid(CAPTURE_ERROR));
    }
    Ok(report.device_id)
}

pub(super) fn configure_and_record(
    connection: &Connection,
    store: &state::Store,
    state: &mut state::Deployment,
    contract: &crate::cloud_runtime::WorkerContract,
    runner: &Runner<'_>,
    configure: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let selected = super::Selection::load(store.root()).map_err(super::mapped)?;
    super::validate_worker_contract(&selected, contract)?;
    let Some(selected) = selected.tailnet else {
        return configure();
    };
    let worker = state
        .worker
        .as_ref()
        .ok_or(Error::Invalid("Missing bound worker for tailnet capture"))?;
    if !matches!(&state.operation, CreateState::Bound { worker_id } if worker_id == &worker.id) {
        return Err(Error::Invalid("Tailnet capture worker differs from its allocation"));
    }
    let spec = state
        .spec
        .as_ref()
        .ok_or(Error::Invalid("Missing allocation request for tailnet capture"))?;
    spec.validate()?;
    worker.verify(spec)?;
    let endpoint = worker
        .ssh_endpoint()
        .ok_or(Error::Invalid("Missing capture worker endpoint"))?;
    let proxy = worker
        .docker_host
        .as_ref()
        .map(|host| host.proxy_command())
        .transpose()?
        .flatten();
    validate_binding(
        connection,
        store.root(),
        &state.cloud_id,
        &worker.id,
        endpoint.host().as_str(),
        endpoint.port(),
        proxy.as_deref(),
    )?;
    let selected = TailnetId::new(&selected).map_err(|_| Error::Invalid(CAPTURE_ERROR))?;
    let attempt = CaptureAttempt {
        attempt_id: OperationId::generate(),
        tailnet_id: selected.clone(),
        provider: EnrollmentProvider::new(horizon_cloud::provider::Description::of(&spec.profile).kind),
        worker_id: WorkerId::new(&worker.id).map_err(|_| Error::Invalid(CAPTURE_ERROR))?,
        allocation_request_id: AllocationRequestId::new(&spec.operation_id)
            .map_err(|_| Error::Invalid(CAPTURE_ERROR))?,
    };
    capture(store, state, attempt, configure, || {
        let reply = runner
            .private_exchange(
                &mut connection.pinned_command(&format!("/usr/bin/python3 -I - {}", selected.as_str())),
                PROJECTION.as_bytes(),
                Duration::from_secs(15),
            )
            .map_err(|error| match error {
                Error::Provider(horizon_cloud::CloudError::Cancelled) => error,
                _ => Error::Invalid(CAPTURE_ERROR),
            })?;
        observed(&reply, &selected)
    })
}

fn validate_binding(
    connection: &Connection,
    root: &std::path::Path,
    cloud_id: &str,
    worker_id: &str,
    host: &str,
    port: u16,
    proxy: Option<&str>,
) -> Result<()> {
    if root.file_name().and_then(|name| name.to_str()) != Some(cloud_id)
        || connection.host_key_alias != format!("horizon-cloud-{worker_id}")
        || connection.known_hosts != root.join(format!("known-hosts-{worker_id}"))
        || connection.host != host
        || connection.port != port
        || connection.proxy_command.as_deref() != proxy
    {
        return Err(Error::Invalid(
            "Tailnet capture connection differs from its owning allocation root",
        ));
    }
    Ok(())
}

fn capture(
    store: &state::Store,
    state: &mut state::Deployment,
    attempt: CaptureAttempt,
    configure: impl FnOnce() -> Result<()>,
    observe: impl FnOnce() -> Result<DeviceId>,
) -> Result<()> {
    let id = attempt.attempt_id;
    state
        .tailnet_enrollment
        .arm(attempt)
        .map_err(|_| Error::Invalid(CAPTURE_ERROR))?;
    store.save(state)?;
    configure()?;
    let device = observe()?;
    let mut recorded = state.clone();
    recorded
        .tailnet_enrollment
        .record(id, device)
        .map_err(|_| Error::Invalid(CAPTURE_ERROR))?;
    recorded.version = recorded.record_version();
    store.save(&recorded)?;
    *state = recorded;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests;
