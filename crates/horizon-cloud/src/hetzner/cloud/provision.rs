//! Requests the key, the workspace volume and the server, each behind a durable
//! record so a lost response never creates a second billed resource.
use super::{
    HOST_IMAGE, Journal, PROBE_DEVICE, Policy, admitted, allowed, check_login, first_fit, fit, holds, location, plan,
    supported, throwaway_public_key, worker,
};
use crate::{
    Cancellation, CloudError, CreateState, Progress, Worker, WorkerSpec,
    hetzner::{
        Hetzner,
        keys::SshKey,
        servers::{Placement, Server, ServerRequest},
        volumes::{SIZE_GB, Volume},
    },
    host,
};

/// The records a caller keeps for a cloud. Each call returns only once the
/// record is durable, because the provider request it guards follows at once.
pub trait Records {
    /// Called at a durable request boundary, before a provider mutation.
    /// # Errors
    /// A failure prevents the mutation; earlier mutations remain unresolved.
    fn before_mutation(&mut self) -> Result<(), CloudError> {
        Ok(())
    }

    /// Called when the request after the last boundary was refused outright, so
    /// it changed nothing.
    fn mutation_refused(&mut self) {}

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
    mut progress: impl FnMut(Progress),
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
    // A login for another registry would only fail the host's pull after the server
    // is billed. Whether the image needs one is the caller's to decide.
    check_login(login.as_ref(), false, &spec.image_digest)?;
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
    // A volume fixes the location. Until one exists, every allowed location with a
    // fitting server type is a candidate, in the policy's order. A requested or
    // bound server was placed already, so reconnecting to it depends on the policy
    // allowing its type, not on the catalog still offering it.
    // An empty volume an earlier attempt created (Hetzner answered that it did)
    // and no server held, left sold out or interrupted, is deleted first, so the
    // cloud is placed afresh. One whose create answer was lost is reconciled by
    // label and kept: it may be an older workspace, so the cloud stays in its
    // location rather than risk deleting it.
    if journal.unused && *operation == CreateState::Prepared && matches!(journal.volume, CreateState::Bound { .. }) {
        release_empty_volume(client, operation_id, journal, records, cancel)?;
    }
    // Checked after that cleanup, so a refused login never keeps an empty volume billed.
    check_pull_login(client, &host, operation, spec, cancel)?;
    // The cloud may move only while it has no volume, neither recorded nor found
    // by label; a found one is adopted and holds whatever the workspace held.
    let found = if journal.volume == CreateState::Prepared {
        client.find_volumes(operation_id, cancel)?
    } else {
        Vec::new()
    };
    let movable = journal.volume == CreateState::Prepared && found.is_empty();
    let candidates = if movable {
        let offers = client.catalog(cancel)?.offers;
        let fitting: Vec<(String, Vec<Placement>)> = policy
            .locations
            .iter()
            .filter_map(|location| {
                fit(&offers, spec, &policy.server_types, location)
                    .ok()
                    .map(|placements| (location.clone(), placements))
            })
            .collect();
        if fitting.is_empty() {
            // The same refusal as before, naming what does not fit.
            first_fit(&offers, spec, policy)?;
        }
        fitting
    } else {
        // A volume found by label fixes the location when the journal lost it.
        let recorded = journal
            .location
            .clone()
            .or_else(|| found.first().map(|volume| volume.location.name.clone()));
        let location = location(recorded.as_deref(), policy)?;
        let placements = if *operation == CreateState::Prepared {
            fit(&client.catalog(cancel)?.offers, spec, &policy.server_types, &location)?
        } else {
            allowed(&policy.server_types, &location)
        };
        vec![(location, placements)]
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
    let key = client.ensure_ssh_key_observed(operation_id, &public_key, cancel, || records.before_mutation())?;
    let mut sold_out = None;
    for (location, placements) in candidates {
        let placed = place(
            client,
            &mut Place {
                spec,
                movable,
                location: &location,
                placements: &placements,
                key: &key,
                host: &mut host,
            },
            operation,
            journal,
            records,
            cancel,
            &mut progress,
        );
        match placed {
            Ok((server, volume)) => return settle(client, spec, policy, &location, &server, &volume, cancel),
            // Every type is sold out here and no server ever held the volume, so it
            // holds nothing: delete it and move on.
            Err(CloudError::Capacity(reason)) if journal.unused && *operation == CreateState::Prepared => {
                release_empty_volume(client, operation_id, journal, records, cancel)?;
                sold_out = Some(reason);
            }
            Err(error) => return Err(error),
        }
    }
    Err(CloudError::Capacity(sold_out.unwrap_or_default()))
}

/// A server is about to be created, and a host that cannot pull its image never
/// becomes ready: an expired or revoked pull login is refused before it is paid
/// for. A requested or bound server is only reconciled, so it is never checked.
fn check_pull_login(
    client: &Hetzner,
    host: &host::Plan,
    operation: &CreateState,
    spec: &WorkerSpec,
    cancel: &Cancellation,
) -> Result<(), CloudError> {
    match &host.registry {
        Some(login) if *operation == CreateState::Prepared => client.verify_pull(login, &spec.image_digest, cancel),
        _ => Ok(()),
    }
}

/// Where one placement attempt goes.
struct Place<'a> {
    spec: &'a WorkerSpec,
    /// Whether this attempt places the cloud's first volume, so one it creates is empty.
    movable: bool,
    location: &'a str,
    placements: &'a [Placement],
    key: &'a SshKey,
    host: &'a mut host::Plan,
}

/// Records the location, ensures the volume there and asks for a server
/// holding it, trying the placements in order.
fn place(
    client: &Hetzner,
    at: &mut Place<'_>,
    operation: &mut CreateState,
    journal: &mut Journal,
    records: &mut impl Records,
    cancel: &Cancellation,
    progress: &mut impl FnMut(Progress),
) -> Result<(Server, Volume), CloudError> {
    let operation_id = at.spec.operation_id.as_str();
    // Recorded before the volume request it fixes; a location chosen earlier but
    // never used by a volume is replaced.
    if journal.location.as_deref() != Some(at.location) {
        let next = Journal {
            location: Some(at.location.to_owned()),
            ..journal.clone()
        };
        records.journal(&next)?;
        *journal = next;
    }
    let mut fence = journal.volume.clone();
    // Only a volume Hetzner answers that this call created is known to be empty,
    // and it is saved as such in the same record that binds it. One found by
    // label, adopted after a name clash or reconciled after a lost answer may be
    // older and hold a workspace, so it never is; a lost answer only means the
    // cloud stays in this location.
    let unused = std::cell::Cell::new(journal.unused);
    let ensured = client.ensure_volume_traced(
        operation_id,
        at.location,
        u32::from(at.spec.profile.storage.volume_gb),
        &mut fence,
        cancel,
        |next, created| {
            let mut saved = journal.clone();
            saved.volume = next.clone();
            if matches!(next, CreateState::Bound { .. }) {
                saved.unused = at.movable && created;
            }
            records.journal(&saved)?;
            if *next == CreateState::Requested {
                records.before_mutation()?;
            }
            unused.set(saved.unused);
            Ok(())
        },
    );
    // The fence moves only after each save succeeds, so it matches the saved
    // journal even when the request failed.
    journal.volume = fence;
    journal.unused = unused.get();
    let volume = ensured?;
    at.host.workspace_device.clone_from(&volume.linux_device);
    let user_data = at.host.cloud_config()?;
    // A new server joins Horizon's private network for its zone, so companion
    // connections to peers there stay off the public internet.
    let network = if *operation == CreateState::Prepared {
        let zone = client.network_zone(at.location, cancel)?;
        Some(
            client
                .ensure_network_observed(&zone, cancel, || records.before_mutation())?
                .id,
        )
    } else {
        None
    };
    let server_request = ServerRequest {
        operation_id,
        placements: at.placements,
        image: HOST_IMAGE,
        user_data: &user_data,
        volume: Some(&volume),
        ssh_key: Some(at.key),
        network,
    };
    let server = client.ensure_server(
        &server_request,
        operation,
        cancel,
        |next| {
            // A server holds the volume from now on, so it is no longer known empty.
            if journal.unused && matches!(next, CreateState::Bound { .. }) {
                let saved = Journal {
                    unused: false,
                    ..journal.clone()
                };
                records.journal(&saved)?;
                *journal = saved;
            }
            records.operation(next)?;
            if *next == CreateState::Requested {
                records.before_mutation()?;
            }
            Ok(())
        },
        &mut *progress,
    )?;
    Ok((server, volume))
}

/// Deletes the empty workspace volume an earlier attempt left, proves it gone and
/// returns the journal to no volume and no location, as if it had never been
/// requested. Only a volume the journal marks `unused` is ever passed here.
fn release_empty_volume(
    client: &Hetzner,
    operation_id: &str,
    journal: &mut Journal,
    records: &mut impl Records,
    cancel: &Cancellation,
) -> Result<(), CloudError> {
    debug_assert!(journal.unused, "only an unused volume is released");
    let mut fence = journal.volume.clone();
    let snapshot = journal.clone();
    let records = std::cell::RefCell::new(records);
    client.delete_volume_observed(
        operation_id,
        &mut fence,
        cancel,
        |next| {
            // Deleted and proven absent: nothing was requested any more.
            let saved = match next {
                CreateState::Terminated { .. } => Journal {
                    volume: CreateState::Prepared,
                    location: None,
                    unused: false,
                    ..snapshot.clone()
                },
                other => Journal {
                    volume: other.clone(),
                    ..snapshot.clone()
                },
            };
            records.borrow_mut().journal(&saved)
        },
        |_| {},
        || records.borrow_mut().before_mutation(),
    )?;
    *journal = Journal {
        volume: CreateState::Prepared,
        location: None,
        unused: false,
        ..journal.clone()
    };
    Ok(())
}

/// The placed server as a worker, once it meets the profile and holds exactly
/// this volume from both sides where the policy allows, as readiness requires.
fn settle(
    client: &Hetzner,
    spec: &WorkerSpec,
    policy: &Policy,
    location: &str,
    server: &Server,
    volume: &Volume,
    cancel: &Cancellation,
) -> Result<Worker, CloudError> {
    let operation_id = spec.operation_id.as_str();
    let server = server.clone();
    let volume = client
        .inspect_volume(volume.id, cancel)?
        .ok_or(CloudError::WorkerLost)?;
    volume.verify(operation_id)?;
    if !holds(&server, &volume) || !admitted(&server, policy, Some(location)) {
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
