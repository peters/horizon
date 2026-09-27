//! Provisions a Hetzner cloud's worker through `horizon_cloud::hetzner::cloud`,
//! keeping its records beside the deployment: the server fence in the deployment
//! record and the rest in the cloud's `hetzner.json`.
use super::{Compute, Journal, JournalFile as _};
use crate::cloud_runtime::{
    Error, Event, Result,
    state::{Deployment, Store},
};
use horizon_cloud::{
    Cancellation, CloudError, CreateState, WorkerSpec,
    hetzner::cloud::{self, Records},
};

pub(in crate::cloud_runtime::deployment) fn provision(
    compute: &Compute,
    store: &Store,
    state: &mut Deployment,
    spec: &WorkerSpec,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<()> {
    // Resources are named after the deployment, and user data and verification
    // follow the spec, so the two must describe the same cloud before any request.
    if spec.operation_id != state.cloud_id || spec.profile != state.profile {
        return Err(Error::Invalid("Deployment and worker identities differ"));
    }
    // Only a request that can still create a server needs the pull login.
    let login = if state.operation == CreateState::Prepared {
        super::pull_login(&compute.settings, compute.registries.as_ref(), &spec.profile.image)?
    } else {
        None
    };
    let request = cloud::Request {
        spec,
        policy: &compute.allowed,
        login,
        // A redeploy resets the source the deployment claims for its workspace.
        fresh: !state.source_ready,
    };
    let mut journal = Journal::load(store.root())?;
    let mut operation = state.operation.clone();
    let worker = cloud::provision(
        &compute.client,
        request,
        &mut operation,
        &mut journal,
        &mut Saved {
            store,
            state: &mut *state,
        },
        cancel,
        |progress| emit(Event::Output(format!("{progress:?}"))),
    )?;
    state.worker = Some(worker);
    store.save(state)
}

/// The deployment record and `hetzner.json`, each saved durably.
struct Saved<'a> {
    store: &'a Store,
    state: &'a mut Deployment,
}

impl Records for Saved<'_> {
    fn journal(&mut self, journal: &Journal) -> std::result::Result<(), CloudError> {
        journal.save(self.store.root()).map_err(|_| CloudError::Persistence)
    }

    fn operation(&mut self, operation: &CreateState) -> std::result::Result<(), CloudError> {
        self.state.operation = operation.clone();
        self.store.save(self.state).map_err(|_| CloudError::Persistence)
    }
}
