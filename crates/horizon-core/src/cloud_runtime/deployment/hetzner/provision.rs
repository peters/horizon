//! Requests the key, the workspace volume and the server, each behind a durable
//! record so a lost response never creates a second billed resource.
use super::{Allowed, Compute, Journal, throwaway_public_key, worker};
use crate::cloud_runtime::{
    Error, Event, Result,
    state::{Deployment, Store},
};
use horizon_cloud::{
    Cancellation, CloudError, CreateState, WorkerSpec,
    hetzner::{
        catalog::Offer,
        servers::{Placement, ServerRequest},
    },
    host,
};
use std::collections::BTreeMap;

/// The Hetzner app image with Docker preinstalled that the host plan expects.
const HOST_IMAGE: &str = "docker-ce";
/// A device path in Hetzner's form with the longest possible volume ID, for
/// checking a host plan before its volume exists: the real path can only be
/// shorter, so the user data cannot outgrow its limit after allocation.
const PROBE_DEVICE: &str = "/dev/disk/by-id/scsi-0HC_Volume_18446744073709551615";

pub(in crate::cloud_runtime::deployment) fn provision(
    compute: &Compute,
    store: &Store,
    state: &mut Deployment,
    spec: &WorkerSpec,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<()> {
    let operation = state.cloud_id.clone();
    // The worker's key, image digest and machine types are part of the host
    // configuration; a malformed one would boot a worker nobody can reach.
    spec.validate()?;
    supported(spec)?;
    if !horizon_cloud::hetzner::volumes::SIZE_GB.contains(&u32::from(spec.profile.storage.volume_gb)) {
        return Err(Error::Invalid(
            "A Hetzner workspace volume must be between 10 and 10,240 GB",
        ));
    }
    // The whole host configuration, registry login included, is loaded once and
    // rendered before the first provider request, so an invalid one leaves
    // nothing behind. The volume's real device path is only known after it exists.
    let mut host = plan(spec, PROBE_DEVICE, compute.settings.registry_login()?)?;
    host.cloud_config()?;
    let root = store.root().to_path_buf();
    let mut journal = Journal::load(&root)?;
    // Every read-only check comes before the first request that creates anything.
    // A volume fixes the location; until one exists, the first allowed location
    // with a fitting server type is chosen. A requested or bound server was placed
    // already, so reconnecting to it depends on the settings allowing its type,
    // not on the catalog still offering it.
    let volume_exists = journal.volume != CreateState::Prepared;
    let (location, placements) = if volume_exists {
        let location = location(journal.location.as_deref(), &compute.allowed)?;
        let placements = if state.operation == CreateState::Prepared {
            fit(
                &compute.client.catalog(cancel)?.offers,
                spec,
                &compute.allowed.server_types,
                &location,
            )?
        } else {
            allowed(&compute.allowed.server_types, &location)
        };
        (location, placements)
    } else {
        first_fit(&compute.client.catalog(cancel)?.offers, spec, &compute.allowed)?
    };
    if journal.key.is_none() {
        journal.key = Some(throwaway_public_key()?);
        journal.save(&root)?;
    }
    let public_key = journal.key.clone().ok_or(Error::Invalid("Missing Hetzner SSH key"))?;
    let key = compute.client.ensure_ssh_key(&operation, &public_key, cancel)?;
    // Recorded before the volume request it fixes; a location chosen earlier but
    // never used by a volume is replaced.
    if journal.location.as_deref() != Some(location.as_str()) {
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
    // Recorded only once it meets the profile, as readiness requires.
    let described = worker(&server, spec, &volume)?;
    described.verify(spec)?;
    described.verify_resources(spec)?;
    state.worker = Some(described);
    store.save(state)
}

/// The location of an existing workspace volume. Settings can change while a
/// cloud has a volume, so a location they no longer allow is refused rather than used.
pub(super) fn location(recorded: Option<&str>, allowed: &Allowed) -> Result<String> {
    match recorded {
        Some(location) if allowed.locations.iter().any(|name| name == location) => Ok(location.to_owned()),
        Some(_) => Err(Error::Invalid(
            "This cloud's workspace volume is in a location the Hetzner settings no longer allow",
        )),
        None => Err(Error::Invalid("This cloud's workspace volume has no recorded location")),
    }
}

/// For a cloud without a volume: the first allowed location, in order, where an
/// allowed server type fits, with those types. Nothing is recorded until a
/// volume is requested there.
pub(super) fn first_fit(offers: &[Offer], spec: &WorkerSpec, allowed: &Allowed) -> Result<(String, Vec<Placement>)> {
    if allowed.locations.is_empty() {
        return Err(Error::Invalid("Hetzner settings list no location"));
    }
    allowed
        .locations
        .iter()
        .find_map(|location| {
            fit(offers, spec, &allowed.server_types, location)
                .ok()
                .map(|placements| (location.clone(), placements))
        })
        .ok_or(Error::Invalid(
            "No configured Hetzner server type has the profile's CPU, memory and container disk in any allowed location",
        ))
}

/// Every allowed server type in the location, in order, for reconciling a
/// server that was already requested.
pub(super) fn allowed(server_types: &[String], location: &str) -> Vec<Placement> {
    server_types
        .iter()
        .map(|server_type| Placement {
            server_type: server_type.clone(),
            location: location.to_owned(),
        })
        .collect()
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
pub(super) fn fit(
    offers: &[Offer],
    spec: &WorkerSpec,
    server_types: &[String],
    location: &str,
) -> Result<Vec<Placement>> {
    let fits = |server_type: &String| {
        offers.iter().any(|offer| {
            &offer.server_type == server_type
                && offer.location == location
                && offer.cores >= u32::from(spec.profile.cpu)
                && offer.memory_gb >= f64::from(spec.profile.memory_gb)
                && offer.disk_gb >= u32::from(spec.profile.storage.container_gb)
        })
    };
    let placements: Vec<Placement> = server_types
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
