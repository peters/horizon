//! The Hetzner half of an image rebuild (`provider::Rebuild::NewServer`). Hetzner
//! cannot report which image a server runs, so a rebuild releases the server as
//! Stop does, keeps the workspace volume, and lets the next reconnect create a new
//! server on the rebuilt image. The rebuild journal keeps the deployment in the
//! `Replace` stage throughout, so these records never record a stop.
use super::{Compute, Journal, JournalFile as _, provision::Saved};
use crate::cloud_runtime::{
    Error, Result,
    settings::Settings,
    state::{Deployment, Store},
};
use horizon_cloud::{
    Cancellation, CloudError, CreateState,
    hetzner::cloud::{self, Records, Stop, StopRecords},
};

/// Releases the bound server and keeps the volume, as Stop does, but leaves the
/// deployment in its `Replace` stage. A retry after an interruption finishes the
/// release: the recorded release is not recorded again, and the server is looked
/// up once more to prove it gone.
/// # Errors
/// Reports provider and persistence failures, leaving records a retry resumes from.
pub(in crate::cloud_runtime) fn release(
    store: &Store,
    state: &mut Deployment,
    settings: &Settings,
    cancel: &Cancellation,
) -> Result<()> {
    release_with(&Compute::cleanup(settings)?, store, state, cancel)
}

pub(super) fn release_with(
    compute: &Compute,
    store: &Store,
    state: &mut Deployment,
    cancel: &Cancellation,
) -> Result<()> {
    let mut journal = Journal::load(store.root())?;
    let operation_id = state.cloud_id.clone();
    let operation = state.operation.clone();
    let mut records = Rebuilding(Saved::new(store, state));
    let released = cloud::stop(
        compute.cloud(&operation_id, cancel),
        &operation,
        &mut journal,
        &mut records,
    );
    records.0.finish(released)
}

/// Saves `state`, the rebuild's outcome, with the fence of the server the rebuild
/// released cleared, then clears the release, as Resume does. The next reconnect
/// creates a new server that attaches the same volume.
/// # Errors
/// Refuses unless the bound server is the one released.
pub(in crate::cloud_runtime) fn reopen(store: &Store, state: &mut Deployment) -> Result<()> {
    let mut journal = Journal::load(store.root())?;
    let CreateState::Bound { worker_id } = &state.operation else {
        return Err(Error::Invalid(
            "Only a bound worker's server can be released for a rebuild",
        ));
    };
    if journal.released.as_deref() != Some(worker_id.as_str()) {
        return Err(Error::Invalid(
            "The server has not been released yet; continue the rebuild to release it",
        ));
    }
    state.operation = CreateState::Prepared;
    state.worker = None;
    state.stop_requested = false;
    store.save(state)?;
    // Saved second: a crash in between leaves a release no bound server matches,
    // which provisioning ignores, as after Resume.
    journal.released = None;
    // A server held this volume, so it is never deleted to move the cloud.
    journal.unused = false;
    journal.save(store.root())
}

/// A stop's records for a rebuild: the journal and server fence as a stop keeps
/// them, but no stop stage, since the rebuild journal holds the `Replace` stage.
struct Rebuilding<'a>(Saved<'a>);

impl Records for Rebuilding<'_> {
    fn journal(&mut self, journal: &Journal) -> std::result::Result<(), CloudError> {
        self.0.journal(journal)
    }

    fn operation(&mut self, operation: &CreateState) -> std::result::Result<(), CloudError> {
        self.0.operation(operation)
    }
}

impl StopRecords for Rebuilding<'_> {
    fn stop(&mut self, stop: Stop) -> std::result::Result<(), CloudError> {
        if stop == Stop::Stopping {
            return Ok(());
        }
        // The last known worker, as a released worker with no endpoint.
        let worker = self.0.state.worker.as_ref().map(cloud::released).transpose()?;
        self.0.state.worker = worker;
        let saved = self.0.store.save(self.0.state);
        self.0.keep(saved)
    }
}

/// Whether a rebuild's release of the bound server has begun: a stop records the
/// release before it shuts the server down, so without it the server is untouched.
/// # Errors
/// Reports an unreadable journal.
pub(in crate::cloud_runtime) fn released(store: &Store, state: &Deployment) -> Result<bool> {
    let journal = Journal::load(store.root())?;
    Ok(
        matches!(&state.operation, CreateState::Bound { worker_id } if journal.released.as_deref() == Some(worker_id.as_str())),
    )
}
