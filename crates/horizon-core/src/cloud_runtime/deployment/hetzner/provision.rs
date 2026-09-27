//! Requests the key, the workspace volume and the server, each behind a durable
//! record so a lost response never creates a second billed resource.
use super::{Compute, Journal, throwaway_public_key, worker};
use crate::cloud_runtime::{
    Error, Event, Result,
    state::{Deployment, Store},
};
use horizon_cloud::{
    Cancellation, CloudError, WorkerSpec,
    hetzner::{
        catalog::Offer,
        servers::{Placement, ServerRequest},
    },
    host,
};
use std::collections::BTreeMap;

/// The Hetzner app image with Docker preinstalled that the host plan expects.
const HOST_IMAGE: &str = "docker-ce";
/// A device path in Hetzner's form, for checking a host plan before its volume exists.
const PROBE_DEVICE: &str = "/dev/disk/by-id/scsi-0HC_Volume_0";

pub(in crate::cloud_runtime::deployment) fn provision(
    compute: &Compute,
    store: &Store,
    state: &mut Deployment,
    spec: &WorkerSpec,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<()> {
    let operation = state.cloud_id.clone();
    supported(spec)?;
    // The whole host configuration, registry login included, is loaded once and
    // rendered before the first provider request, so an invalid one leaves
    // nothing behind. The volume's real device path is only known after it exists.
    let mut host = plan(spec, PROBE_DEVICE, compute.settings.registry_login()?)?;
    host.cloud_config()?;
    let root = store.root().to_path_buf();
    let mut journal = Journal::load(&root)?;
    let location = location(journal.location.as_deref(), spec)?;
    // Every read-only check comes before the first request that creates anything.
    let placements = fit(&compute.client.catalog(cancel)?.offers, spec, &location)?;
    if journal.key.is_none() {
        journal.key = Some(throwaway_public_key()?);
        journal.save(&root)?;
    }
    let public_key = journal.key.clone().ok_or(Error::Invalid("Missing Hetzner SSH key"))?;
    let key = compute.client.ensure_ssh_key(&operation, &public_key, cancel)?;
    if journal.location.is_none() {
        journal.location = Some(location.clone());
        journal.save(&root)?;
    }
    let mut fence = journal.volume.clone();
    let volume = compute.client.ensure_volume(
        &operation,
        &location,
        u32::from(spec.profile.storage.volume_gb),
        &mut fence,
        cancel,
        |next| {
            let mut saved = journal.clone();
            saved.volume = next.clone();
            saved.save(&root).map_err(|_| CloudError::Persistence)
        },
    )?;
    journal.volume = fence;
    host.workspace_device.clone_from(&volume.linux_device);
    let user_data = host.cloud_config()?;
    let request = ServerRequest {
        operation_id: &operation,
        placements: &placements,
        image: HOST_IMAGE,
        user_data: &user_data,
        volume: Some(&volume),
        ssh_key: Some(&key),
    };
    let mut fence = state.operation.clone();
    let server = compute.client.ensure_server(
        &request,
        &mut fence,
        cancel,
        |next| {
            state.operation = next.clone();
            store.save(state).map_err(|_| CloudError::Persistence)
        },
        |progress| emit(Event::Output(format!("{progress:?}"))),
    )?;
    state.worker = Some(worker(&server, spec, &volume)?);
    store.save(state)
}

/// Where the cloud's volume and server go. A recorded location is fixed by the
/// volume created there; until then the first allowed location is only a
/// candidate. Settings can change while a cloud has a volume but no server yet,
/// so a recorded location they no longer allow is refused rather than used.
pub(super) fn location(recorded: Option<&str>, spec: &WorkerSpec) -> Result<String> {
    match recorded {
        Some(location) if !spec.data_centers.iter().any(|allowed| allowed == location) => Err(Error::Invalid(
            "This cloud's workspace volume is in a location the Hetzner settings no longer allow",
        )),
        Some(location) => Ok(location.to_owned()),
        None => spec
            .data_centers
            .first()
            .cloned()
            .ok_or(Error::Invalid("Hetzner settings list no location")),
    }
}

/// A shared worker's startup data has no Hetzner path yet.
fn supported(spec: &WorkerSpec) -> Result<()> {
    if spec.startup_metadata.is_some() {
        return Err(Error::Invalid("Shared workers are not available on Hetzner yet"));
    }
    Ok(())
}

/// The configured server types, in order, whose CPU, memory and local disk fit the profile in the volume's
/// location. Hetzner's availability flag is advisory, so it is not used to skip a type.
pub(super) fn fit(offers: &[Offer], spec: &WorkerSpec, location: &str) -> Result<Vec<Placement>> {
    let fits = |server_type: &String| {
        offers.iter().any(|offer| {
            &offer.server_type == server_type
                && offer.location == location
                && offer.cores >= u32::from(spec.profile.cpu)
                && offer.memory_gb >= f64::from(spec.profile.memory_gb)
                && offer.disk_gb >= u32::from(spec.profile.storage.container_gb)
        })
    };
    let placements: Vec<Placement> = spec
        .cpu_flavors
        .iter()
        .filter(|server_type| fits(server_type))
        .map(|server_type| Placement {
            server_type: server_type.clone(),
            location: location.to_owned(),
        })
        .collect();
    if placements.is_empty() {
        return Err(Error::Invalid(
            "No configured Hetzner server type has the profile's CPU, memory and container disk in the workspace's location",
        ));
    }
    Ok(placements)
}

/// The host plan for this worker. The environment matches what the worker image
/// expects from any provider; nothing secret is in it.
pub(super) fn plan(spec: &WorkerSpec, device: &str, registry: Option<host::RegistryLogin>) -> Result<host::Plan> {
    let environment = BTreeMap::from([
        ("PUBLIC_KEY".to_owned(), spec.public_key.clone()),
        ("HORIZON_CLOUD_OPERATION".to_owned(), spec.operation_id.clone()),
        (
            "HORIZON_WORKER_CAPABILITIES".to_owned(),
            serde_json::to_string(&spec.profile.capabilities).map_err(|_| Error::Json)?,
        ),
    ]);
    Ok(host::Plan {
        image: spec.image_digest.clone(),
        environment,
        registry,
        workspace_device: device.to_owned(),
        shm_gb: shared_memory_gb(spec.profile.memory_gb),
    })
}

/// A quarter of the worker's memory for Chromium's shared memory, within the host's limits.
fn shared_memory_gb(memory_gb: u16) -> u8 {
    u8::try_from((memory_gb / 4).clamp(1, 16)).unwrap_or(1)
}
