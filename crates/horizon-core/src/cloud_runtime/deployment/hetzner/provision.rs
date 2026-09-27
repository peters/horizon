//! Requests the key, the workspace volume and the server, each behind a durable
//! record so a lost response never creates a second billed resource.
use super::{Compute, Journal, JournalFile as _, throwaway_public_key, worker};
use crate::cloud_runtime::{
    Error, Event, Result,
    state::{Deployment, Store},
};
use horizon_cloud::{
    Cancellation, CloudError, CreateState, WorkerSpec,
    hetzner::{
        cloud::{HOST_IMAGE, PROBE_DEVICE, allowed, first_fit, fit, location, plan, supported},
        servers::ServerRequest,
    },
};

pub(in crate::cloud_runtime::deployment) fn provision(
    compute: &Compute,
    store: &Store,
    state: &mut Deployment,
    spec: &WorkerSpec,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<()> {
    let operation = state.cloud_id.clone();
    // Resources are named after the deployment, and user data and verification
    // follow the spec, so the two must describe the same cloud before any request.
    if spec.operation_id != operation || spec.profile != state.profile {
        return Err(Error::Invalid("Deployment and worker identities differ"));
    }
    // The worker's key, image digest and machine types are part of the host
    // configuration; a malformed one would boot a worker nobody can reach.
    spec.validate()?;
    supported(spec)?;
    if !horizon_cloud::hetzner::volumes::SIZE_GB.contains(&u32::from(spec.profile.storage.volume_gb)) {
        return Err(Error::Invalid(
            "A Hetzner workspace volume must be between 10 and 10,240 GB",
        ));
    }
    // The whole host configuration is loaded once and rendered before the first
    // provider request, so an invalid one leaves nothing behind. The registry
    // login is part of it only while a server can still be created: a requested
    // or bound server is only reconciled, so a rotated pull credential never
    // blocks recovering it. The volume's real device path is only known after it exists.
    let login = if state.operation == CreateState::Prepared {
        super::pull_login(&compute.settings, compute.registries.as_ref(), &spec.profile.image)?
    } else {
        None
    };
    let mut host = plan(spec, PROBE_DEVICE, login)?;
    host.cloud_config()?;
    let root = store.root().to_path_buf();
    let mut journal = journal_for(&root, state)?;
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
    // Recorded only once it meets the profile and holds exactly this volume from
    // both sides, as readiness requires.
    let volume = compute
        .client
        .inspect_volume(volume.id, cancel)?
        .ok_or(CloudError::WorkerLost)?;
    volume.verify(&operation)?;
    if !horizon_cloud::hetzner::cloud::holds(&server, &volume)
        || !horizon_cloud::hetzner::cloud::admitted(&server, &compute.allowed, Some(location.as_str()))
    {
        return Err(Error::Invalid(
            "The new server does not hold this cloud's workspace volume where the settings allow",
        ));
    }
    let described = worker(&server, spec, &volume)?;
    described.verify(spec)?;
    described.verify_resources(spec)?;
    state.worker = Some(described);
    store.save(state)
}

/// The journal provisioning goes on from. A server a stop released is never
/// reconnected to. A cloud whose delete started is refused until the delete has
/// finished (no key, volume or server left) and the deployment no longer claims
/// the old workspace's source, which a redeploy resets; it then starts afresh
/// wherever the settings allow now.
fn journal_for(root: &std::path::Path, state: &Deployment) -> Result<Journal> {
    let operation = &state.operation;
    let mut journal = Journal::load(root)?;
    if matches!(operation, CreateState::Bound { worker_id } if journal.released.as_ref() == Some(worker_id)) {
        return Err(Error::Invalid(
            "This Hetzner cloud is stopping; stop it again to finish, then resume it",
        ));
    }
    if journal.deleting || matches!(journal.volume, CreateState::Terminated { .. }) {
        if journal.key.is_some()
            || matches!(journal.volume, CreateState::Requested | CreateState::Bound { .. })
            || *operation != CreateState::Prepared
            || state.source_ready
        {
            return Err(Error::Invalid(
                "Finish deleting this Hetzner cloud before deploying it again",
            ));
        }
        journal = Journal {
            location: None,
            volume: CreateState::Prepared,
            key: None,
            released: None,
            deleting: false,
        };
        journal.save(root)?;
    }
    Ok(journal)
}
