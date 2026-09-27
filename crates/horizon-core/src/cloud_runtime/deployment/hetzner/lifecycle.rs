//! Check, stop, resume and delete for Hetzner clouds through
//! `horizon_cloud::hetzner::cloud`, keeping their records beside the deployment:
//! the server fence and the stop in the deployment record, the rest in the
//! cloud's `hetzner.json`.
use super::{Compute, Journal, JournalFile as _, provision::Saved};
use crate::cloud_runtime::{
    Result, Stage,
    settings::Settings,
    state::{Deployment, Store},
};
use horizon_cloud::{
    Cancellation, CloudError, CreateState,
    hetzner::cloud::{self, Check, Stop, StopRecords},
    runpod::recovery::{Outcome, Reconciliation},
};
use std::time::Duration;

/// How long a delete waits before trusting that an uncertain request created nothing.
#[cfg(not(test))]
const UNRESOLVED_GRACE: Duration = cloud::UNRESOLVED_GRACE;
#[cfg(test)]
const UNRESOLVED_GRACE: Duration = Duration::ZERO;

/// Checks the recorded server without creating, starting or deleting anything.
pub(in crate::cloud_runtime) fn reconcile(
    store: &Store,
    state: &mut Deployment,
    settings: &Settings,
    cancel: &Cancellation,
) -> Result<Reconciliation> {
    // Ownership checks alone settle a released or missing server, so a changed
    // placement policy never blocks finishing a stop; the policy in force applies
    // only when a live server is reported as the worker.
    // A policy that cannot be built is reported as itself, not as the provider
    // layer's generic refusal.
    let refused = std::cell::RefCell::new(None);
    let policy = || {
        Compute::new(settings).map(|compute| compute.allowed).map_err(|error| {
            *refused.borrow_mut() = Some(error);
            CloudError::Invalid("The Hetzner settings no longer allow this cloud's placement")
        })
    };
    reconcile_with(&Compute::cleanup(settings)?, &policy, store, state, cancel)
        .map_err(|error| refused.take().unwrap_or(error))
}

/// As `reconcile`, with the Hetzner client given.
pub(super) fn reconcile_with(
    compute: &Compute,
    policy: &dyn Fn() -> std::result::Result<super::Allowed, CloudError>,
    store: &Store,
    state: &mut Deployment,
    cancel: &Cancellation,
) -> Result<Reconciliation> {
    let journal = Journal::load(store.root())?;
    let operation_id = state.cloud_id.clone();
    let spec = state.spec.clone();
    let mut operation = state.operation.clone();
    let mut saved = Saved::new(store, state);
    let found = cloud::check(
        compute.cloud(&operation_id, cancel),
        spec.as_ref(),
        &mut operation,
        &journal,
        policy,
        &mut saved,
    );
    let found = saved.finish(found)?;
    let mut report = Reconciliation {
        operation_id,
        outcome: Outcome::Prepared,
        worker: None,
    };
    report.outcome = match found {
        Check::Prepared => Outcome::Prepared,
        Check::Terminated { worker_id } => Outcome::Terminated { worker_id },
        Check::Unresolved => Outcome::Unresolved,
        Check::Conflicting { worker_ids } => Outcome::Conflicting { worker_ids },
        Check::Released { worker_id, gone } => {
            if gone {
                report.worker.clone_from(&state.worker);
            }
            Outcome::Inactive { worker_id }
        }
        Check::Missing { worker_id } => Outcome::Missing { worker_id },
        Check::Found { worker_id, worker } => {
            report.worker = Some(*worker);
            Outcome::Found { worker_id }
        }
    };
    Ok(report)
}

/// Releases the server and keeps the workspace volume.
pub(in crate::cloud_runtime) fn stop(
    store: &Store,
    state: &mut Deployment,
    settings: &Settings,
    cancel: &Cancellation,
) -> Result<()> {
    stop_with(&Compute::cleanup(settings)?, store, state, cancel)
}

/// As `stop`, with the Hetzner client given.
pub(super) fn stop_with(compute: &Compute, store: &Store, state: &mut Deployment, cancel: &Cancellation) -> Result<()> {
    let mut journal = Journal::load(store.root())?;
    let operation_id = state.cloud_id.clone();
    let operation = state.operation.clone();
    let mut saved = Saved::new(store, state);
    // A stop records its stage before each provider request, even on a retry.
    saved.resave = true;
    let stopped = cloud::stop(
        compute.cloud(&operation_id, cancel),
        &operation,
        &mut journal,
        &mut saved,
    );
    saved.finish(stopped)?;
    Ok(())
}

/// Clears the released server's fence. The next reconnect creates a new server
/// that attaches the same volume, then waits for readiness as a first start does.
pub(in crate::cloud_runtime) fn resume(store: &Store, state: &mut Deployment) -> Result<()> {
    let mut journal = Journal::load(store.root())?;
    cloud::resumable(&state.operation, &journal, state.stage == Stage::Stopped)?;
    state.operation = CreateState::Prepared;
    state.worker = None;
    state.stop_requested = false;
    state.stage = Stage::Readiness;
    state.timeline = Some(crate::cloud_runtime::timeline::Timeline::resume_requested(
        std::time::SystemTime::now(),
    ));
    store.save(state)?;
    journal.released = None;
    journal.save(store.root())
}

/// Deletes the server, the workspace volume and the SSH key, each proven absent.
pub(in crate::cloud_runtime) fn delete(
    store: &Store,
    state: &mut Deployment,
    settings: &Settings,
    cancel: &Cancellation,
) -> Result<()> {
    delete_with(&Compute::cleanup(settings)?, store, state, cancel)
}

/// As `delete`, with the Hetzner client given.
pub(super) fn delete_with(
    compute: &Compute,
    store: &Store,
    state: &mut Deployment,
    cancel: &Cancellation,
) -> Result<()> {
    let mut journal = Journal::load(store.root())?;
    let operation_id = state.cloud_id.clone();
    let mut operation = state.operation.clone();
    let mut saved = Saved::new(store, state);
    let deleted = cloud::delete(
        compute.cloud(&operation_id, cancel),
        &mut operation,
        &mut journal,
        &mut saved,
        UNRESOLVED_GRACE,
    );
    saved.finish(deleted)?;
    Ok(())
}

impl StopRecords for Saved<'_> {
    fn stop(&mut self, stop: Stop) -> std::result::Result<(), CloudError> {
        let stage = match stop {
            Stop::Stopping => Stage::Stopping,
            Stop::Stopped => {
                // The last known worker, as a stopped worker with no endpoint.
                let worker = self.state.worker.as_ref().map(cloud::released).transpose()?;
                self.state.worker = worker;
                Stage::Stopped
            }
        };
        if !self.resave && self.state.stop_requested && self.state.stage == stage {
            return Ok(());
        }
        self.state.stop_requested = true;
        self.state.stage = stage;
        let saved = self.store.save(self.state);
        self.keep(saved)
    }
}
