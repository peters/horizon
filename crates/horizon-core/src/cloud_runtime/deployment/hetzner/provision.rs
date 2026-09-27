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
    let mut saved = Saved::new(store, state);
    let worker = cloud::provision(
        &compute.client,
        request,
        &mut operation,
        &mut journal,
        &mut saved,
        cancel,
        |progress| emit(Event::Output(format!("{progress:?}"))),
    );
    let worker = saved.finish(worker)?;
    state.worker = Some(worker);
    store.save(state)
}

/// The deployment record and `hetzner.json`, each saved durably. A save that
/// fails is kept, so the caller reports its cause rather than the provider
/// layer's generic persistence error.
pub(super) struct Saved<'a> {
    pub(super) store: &'a Store,
    pub(super) state: &'a mut Deployment,
    failed: Option<Error>,
    /// Whether a stop's stage is saved again even when it is already recorded, as
    /// a stop does before each provider request; a check saves only a change.
    pub(super) resave: bool,
}

impl<'a> Saved<'a> {
    pub(super) fn new(store: &'a Store, state: &'a mut Deployment) -> Self {
        Self {
            store,
            state,
            failed: None,
            resave: false,
        }
    }

    /// The result of a provider call made with these records, with a failed
    /// save reported as itself.
    pub(super) fn finish<T>(self, result: std::result::Result<T, CloudError>) -> Result<T> {
        result.map_err(|error| match (error, self.failed) {
            (CloudError::Persistence, Some(failed)) => failed,
            (error, _) => error.into(),
        })
    }

    pub(super) fn keep(&mut self, saved: Result<()>) -> std::result::Result<(), CloudError> {
        saved.map_err(|error| {
            self.failed = Some(error);
            CloudError::Persistence
        })
    }
}

impl Records for Saved<'_> {
    fn journal(&mut self, journal: &Journal) -> std::result::Result<(), CloudError> {
        let saved = journal.save(self.store.root());
        self.keep(saved)
    }

    fn operation(&mut self, operation: &CreateState) -> std::result::Result<(), CloudError> {
        self.state.operation = operation.clone();
        let saved = self.store.save(self.state);
        self.keep(saved)
    }
}
