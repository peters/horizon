//! Check, stop, resume and delete of an Azure cloud through
//! `horizon_cloud::azure::lifecycle`, with the VM fence and the stop in the
//! deployment record and the group in the cloud's `azure.json`. A stop deallocates
//! the VM and Resume starts the same one (`StoppedCost::WorkerKept`), as on `RunPod`;
//! the reconnect after a resume waits for its readiness.
use super::{load, provision::Saved};
use crate::cloud_runtime::{
    Error, Result, Stage,
    lifecycle::{request_resume, request_stop},
    mutation::{self, State as Mutation},
    settings::Settings,
    state::{Deployment, Store},
};
use horizon_cloud::{
    Cancellation, CloudError, CreateState, Worker,
    azure::{
        Azure, Found,
        lifecycle::{self as cloud, Power},
    },
    runpod::recovery::{Outcome, Reconciliation},
};

/// The ARM client for the cloud at `store`. The settings must name the subscription
/// that the cloud's group was requested or made in, because a delete proves the group
/// absent only in the client's subscription.
/// # Errors
/// Refuses missing settings, an unreadable token, another subscription, and a
/// requested group whose journal does not record its subscription.
pub(in crate::cloud_runtime) fn client(settings: &Settings, store: &Store) -> Result<Azure> {
    let azure = settings.azure_settings()?;
    let client = Azure::new(azure.credential()?, &azure.subscription_id)?;
    let journal = load(store.root())?;
    let subscription = match &journal.group {
        CreateState::Prepared => return Ok(client),
        CreateState::Requested => journal.subscription.as_deref().ok_or(Error::Invalid(
            "Horizon did not record which Azure subscription the cloud's resource group was requested in; \
             find the group horizon-ws-<cloud id> in the Azure portal and delete it there",
        ))?,
        CreateState::Bound { worker_id } | CreateState::Terminated { worker_id } => {
            worker_id.split('/').nth(2).unwrap_or_default()
        }
    };
    if !subscription.eq_ignore_ascii_case(client.subscription_id()) {
        return Err(Error::Invalid(
            "The cloud's Azure resource group is in another subscription than the settings name",
        ));
    }
    Ok(client)
}

/// Checks the recorded VM without creating, starting or deleting anything. A VM
/// found for a requested worker is bound; the size and region policy does not
/// apply, so a changed setting never blocks finishing a stop.
pub(in crate::cloud_runtime) fn check(
    client: &Azure,
    store: &Store,
    state: &mut Deployment,
    worker_hint: Option<&str>,
    cancel: &Cancellation,
) -> Result<Reconciliation> {
    // A rebuild's release deletes the VM; only the rebuild settles it.
    state.refuse_unsettled_replacement()?;
    let spec = state.spec.clone().ok_or(Error::Invalid("No worker was requested"))?;
    let hinted = |id: &str| worker_hint.is_none_or(|hint| hint == id);
    let mut report = Reconciliation {
        operation_id: state.cloud_id.clone(),
        outcome: Outcome::Prepared,
        worker: None,
    };
    let bound = match state.operation.clone() {
        CreateState::Prepared if worker_hint.is_some() => {
            return Err(Error::Invalid("No uncertain creation request to reconcile"));
        }
        CreateState::Prepared => None,
        CreateState::Terminated { worker_id } | CreateState::Bound { worker_id } if !hinted(&worker_id) => {
            return Err(CloudError::IdentityMismatch.into());
        }
        CreateState::Terminated { worker_id } => {
            report.outcome = Outcome::Terminated { worker_id };
            None
        }
        CreateState::Bound { worker_id } => {
            report.outcome = Outcome::Missing {
                worker_id: worker_id.clone(),
            };
            cloud::inspect_bound(client, &spec, &worker_id, cancel)?
        }
        CreateState::Requested => {
            report.outcome = Outcome::Unresolved;
            let Found::Created { vm, .. } = client.reconcile(&state.cloud_id, cancel)? else {
                return Ok(report);
            };
            let instance = vm.instance_id.ok_or(CloudError::InvalidResponse)?;
            if !hinted(&instance) {
                return Err(CloudError::IdentityMismatch.into());
            }
            let bound = cloud::inspect_bound(client, &spec, &instance, cancel)?;
            if bound.is_some() {
                state.operation = CreateState::Bound { worker_id: instance };
                store.save(state)?;
            }
            bound
        }
    };
    if let Some(bound) = bound {
        report.outcome = outcome(&bound.worker);
        report.worker = Some(bound.worker);
    }
    Ok(report)
}

fn outcome(worker: &Worker) -> Outcome {
    let worker_id = worker.id.clone();
    if worker.is_starting_or_running() {
        Outcome::Found { worker_id }
    } else {
        Outcome::Inactive { worker_id }
    }
}

/// Deallocates the bound VM and keeps its disks and address. The stop is confirmed
/// only once ARM reports the VM deallocated.
pub(in crate::cloud_runtime) fn stop(
    client: &Azure,
    store: &Store,
    state: &mut Deployment,
    cancel: &Cancellation,
    observe: mutation::Observer<'_>,
) -> Result<()> {
    let CreateState::Bound { worker_id } = state.operation.clone() else {
        return Err(Error::Invalid("Reconcile a bound worker before stopping it"));
    };
    let spec = state
        .spec
        .clone()
        .ok_or(Error::Invalid("Missing worker specification"))?;
    let bound = cloud::inspect_bound(client, &spec, &worker_id, cancel)?.ok_or(CloudError::WorkerLost)?;
    request_stop(
        store,
        state,
        !bound.deallocated(),
        |announce| cloud::power(client, &spec, &worker_id, Power::Deallocate, cancel, announce).map(drop),
        observe,
    )?;
    let bound = cloud::inspect_bound(client, &spec, &worker_id, cancel)?.ok_or(CloudError::WorkerLost)?;
    let deallocated = bound.deallocated();
    state.worker = Some(bound.worker);
    if !deallocated {
        store.save(state)?;
        return Err(Error::Invalid(
            "Stop requested but not confirmed. Reconcile the stop before resuming.",
        ));
    }
    state.stage = Stage::Stopped;
    store.save(state)?;
    observe(Mutation::Settled)?;
    Ok(())
}

/// Starts the same VM again; the reconnect that follows waits for its readiness.
pub(in crate::cloud_runtime) fn resume(
    client: &Azure,
    store: &Store,
    state: &mut Deployment,
    cancel: &Cancellation,
    observe: mutation::Observer<'_>,
) -> Result<()> {
    let CreateState::Bound { worker_id } = state.operation.clone() else {
        return Err(Error::Invalid("No existing worker to resume"));
    };
    let spec = state
        .spec
        .clone()
        .ok_or(Error::Invalid("Missing worker specification"))?;
    if load(store.root())?.deleting {
        return Err(Error::Invalid(
            "The cloud is being deleted; finish deleting it before resuming",
        ));
    }
    let bound = cloud::inspect_bound(client, &spec, &worker_id, cancel)?.ok_or(CloudError::WorkerLost)?;
    request_resume(
        store,
        state,
        !bound.running(),
        |announce| cloud::power(client, &spec, &worker_id, Power::Start, cancel, announce).map(drop),
        observe,
    )
}

/// Deletes the cloud's resource group and proves it and every tagged resource
/// absent; recording the cloud as deleted is left to the caller.
pub(in crate::cloud_runtime) fn delete(
    client: &Azure,
    store: &Store,
    state: &mut Deployment,
    cancel: &Cancellation,
) -> Result<()> {
    let mut journal = load(store.root())?;
    let operation_id = state.cloud_id.clone();
    let mut operation = state.operation.clone();
    let mut saved = Saved::new(store, state, mutation::IGNORE);
    let deleted = cloud::delete(client, &operation_id, &mut operation, &mut journal, &mut saved, cancel);
    saved.finish(deleted)
}
