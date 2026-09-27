//! Requests the key, the workspace volume and the server, each behind a durable
//! record so a lost response never creates a second billed resource.
use super::{
    HOST_IMAGE, Journal, PROBE_DEVICE, Policy, admitted, allowed, first_fit, fit, holds, location, plan, supported,
    throwaway_public_key, worker,
};
use crate::{
    Cancellation, CloudError, CreateState, Progress, Worker, WorkerSpec,
    hetzner::{Hetzner, servers::ServerRequest, volumes::SIZE_GB},
    host,
};

/// The records a caller keeps for a cloud. Each call returns only once the
/// record is durable, because the provider request it guards follows at once.
pub trait Records {
    /// # Errors
    /// Reports a record that could not be made durable.
    fn journal(&mut self, journal: &Journal) -> Result<(), CloudError>;
    /// The fence for the cloud's server.
    /// # Errors
    /// Reports a record that could not be made durable.
    fn operation(&mut self, operation: &CreateState) -> Result<(), CloudError>;
}

/// What a worker is provisioned from.
pub struct Request<'a> {
    pub spec: &'a WorkerSpec,
    /// The policy in force now; see [`Policy`].
    pub policy: &'a Policy,
    /// The pull login for a private image. Callers load it only while the server
    /// fence is `Prepared`: a requested or bound server is only reconciled and
    /// its user data never sent again, so a rotated credential never blocks
    /// recovering it.
    pub login: Option<host::RegistryLogin>,
    /// Whether the caller holds nothing of a previous workspace, as after a
    /// redeploy. A deleted cloud starts afresh only then.
    pub fresh: bool,
}

/// Provisions the cloud's worker, or reconnects to the one it has, and returns
/// it once the server meets the profile and holds exactly the cloud's volume
/// where the policy allows. Everything is validated and loaded before the first
/// request that creates anything; every created resource is recorded before the
/// next step, so any failure leaves a state that a retry or a delete can resume from.
/// # Errors
/// Refuses invalid requests before any request, and reports provider,
/// persistence and verification failures.
pub fn provision(
    client: &Hetzner,
    request: Request<'_>,
    operation: &mut CreateState,
    journal: &mut Journal,
    records: &mut impl Records,
    cancel: &Cancellation,
    progress: impl FnMut(Progress),
) -> Result<Worker, CloudError> {
    let Request {
        spec,
        policy,
        login,
        fresh,
    } = request;
    let operation_id = spec.operation_id.as_str();
    // The worker's key, image digest and machine types are part of the host
    // configuration; a malformed one would boot a worker nobody can reach.
    spec.validate()?;
    supported(spec)?;
    if !SIZE_GB.contains(&u32::from(spec.profile.storage.volume_gb)) {
        return Err(CloudError::Invalid(
            "A Hetzner workspace volume must be between 10 and 10,240 GB",
        ));
    }
    // The whole host configuration is rendered once before the first request.
    // The volume's real device path is only known after it exists, and can only
    // be shorter than the probe's.
    let mut host = plan(spec, PROBE_DEVICE, login)?;
    host.cloud_config()?;
    reopen(journal, operation, fresh, records)?;
    // A volume fixes the location; until one exists, the first allowed location
    // with a fitting server type is chosen. A requested or bound server was placed
    // already, so reconnecting to it depends on the policy allowing its type, not
    // on the catalog still offering it.
    let (location, placements) = if journal.volume == CreateState::Prepared {
        first_fit(&client.catalog(cancel)?.offers, spec, policy)?
    } else {
        let location = location(journal.location.as_deref(), policy)?;
        let placements = if *operation == CreateState::Prepared {
            fit(&client.catalog(cancel)?.offers, spec, &policy.server_types, &location)?
        } else {
            allowed(&policy.server_types, &location)
        };
        (location, placements)
    };
    // Each journal change is saved before the caller's journal takes it, so a
    // failed save leaves the journal as it is on disk and a retry saves it again.
    if journal.key.is_none() {
        let next = Journal {
            key: Some(throwaway_public_key()?),
            ..journal.clone()
        };
        records.journal(&next)?;
        *journal = next;
    }
    let public_key = journal
        .key
        .clone()
        .ok_or(CloudError::Invalid("Missing Hetzner SSH key"))?;
    let key = client.ensure_ssh_key(operation_id, &public_key, cancel)?;
    // Recorded before the volume request it fixes; a location chosen earlier but
    // never used by a volume is replaced.
    if journal.location.as_deref() != Some(location.as_str()) {
        let next = Journal {
            location: Some(location.clone()),
            ..journal.clone()
        };
        records.journal(&next)?;
        *journal = next;
    }
    let mut fence = journal.volume.clone();
    let ensured = client.ensure_volume(
        operation_id,
        &location,
        u32::from(spec.profile.storage.volume_gb),
        &mut fence,
        cancel,
        |next| {
            let mut saved = journal.clone();
            saved.volume = next.clone();
            records.journal(&saved)
        },
    );
    // The fence moves only after each save succeeds, so it matches the saved
    // journal even when the request failed.
    journal.volume = fence;
    let volume = ensured?;
    host.workspace_device.clone_from(&volume.linux_device);
    let user_data = host.cloud_config()?;
    let server_request = ServerRequest {
        operation_id,
        placements: &placements,
        image: HOST_IMAGE,
        user_data: &user_data,
        volume: Some(&volume),
        ssh_key: Some(&key),
    };
    let server = client.ensure_server(
        &server_request,
        operation,
        cancel,
        |next| records.operation(next),
        progress,
    )?;
    // A worker only once it meets the profile and holds exactly this volume from
    // both sides where the policy allows, as readiness requires.
    let volume = client
        .inspect_volume(volume.id, cancel)?
        .ok_or(CloudError::WorkerLost)?;
    volume.verify(operation_id)?;
    if !holds(&server, &volume) || !admitted(&server, policy, Some(location.as_str())) {
        return Err(CloudError::Invalid(
            "The new server does not hold this cloud's workspace volume where the settings allow",
        ));
    }
    let described = worker(&server, spec, &volume)?;
    described.verify(spec)?;
    described.verify_resources(spec)?;
    Ok(described)
}

/// Refuses a server a stop released, and a cloud whose delete has not finished.
/// A finished delete of a cloud the caller holds nothing of starts afresh,
/// wherever the policy allows now.
fn reopen(
    journal: &mut Journal,
    operation: &CreateState,
    fresh: bool,
    records: &mut impl Records,
) -> Result<(), CloudError> {
    if matches!(operation, CreateState::Bound { worker_id } if journal.released.as_ref() == Some(worker_id)) {
        return Err(CloudError::Invalid(
            "This Hetzner cloud is stopping; stop it again to finish, then resume it",
        ));
    }
    if journal.deleting || matches!(journal.volume, CreateState::Terminated { .. }) {
        if journal.key.is_some()
            || matches!(journal.volume, CreateState::Requested | CreateState::Bound { .. })
            || *operation != CreateState::Prepared
            || !fresh
        {
            return Err(CloudError::Invalid(
                "Finish deleting this Hetzner cloud before deploying it again",
            ));
        }
        let next = Journal::default();
        records.journal(&next)?;
        *journal = next;
    }
    Ok(())
}
