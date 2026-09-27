//! `RunPod`'s lifecycle: a stop keeps the worker and Resume starts the same one;
//! hosted devices and image replacements are settled around it.
use super::{
    super::{Error, Stage, command::Runner, deployment, ssh::Connection},
    Cancellation, CreateState, Deployment, Lifecycle, Result, Settings, Store,
};
use horizon_cloud::{CloudError, WorkerStatus, runpod::recovery::Reconciliation};

/// The worker's workspace storage, or a volume request recorded before any worker.
pub(super) fn retained(store: &Store, state: &Deployment) -> Result<bool> {
    match &state.spec {
        Some(_) => deployment::storage::retained(store, state),
        None => Ok(store.root().join("workspace-volume.json").try_exists()?
            || store.root().join("workspace-volume.required").try_exists()?),
    }
}

pub(super) struct RunPod<'a> {
    pub(super) settings: &'a Settings,
}

impl RunPod<'_> {
    fn client(&self) -> Result<horizon_cloud::runpod::RunPod> {
        Ok(horizon_cloud::runpod::RunPod::new(self.settings.credential()?))
    }

    /// Removes the hosted-device credentials from the running `worker`.
    fn revoke(&self, store: &Store, worker: &horizon_cloud::Worker, cancel: &Cancellation) -> Result<()> {
        let connection = Connection::new(worker, self.settings, store.root())?;
        super::super::browser_auth::revoke(
            &connection,
            &Runner {
                cancel,
                emit: &|_| {},
                secrets: Vec::new(),
            },
        )
    }
}

impl Lifecycle for RunPod<'_> {
    fn check(
        &self,
        store: &Store,
        state: &mut Deployment,
        worker_hint: Option<&str>,
        cancel: &Cancellation,
    ) -> Result<(Reconciliation, CreateState)> {
        let provider = self.client()?;
        // Settling commits only a new image and pull credential for the same worker.
        deployment::replacement::settle(&provider, store, state, cancel)?;
        let spec = state.spec.clone().ok_or(Error::Invalid("No worker was requested"))?;
        let mut operation = state.operation.clone();
        let report = provider.reconcile(&spec, &mut operation, worker_hint, cancel, |next| {
            if matches!(next, CreateState::Terminated { .. }) && state.requires_browserstack_release() {
                return Err(CloudError::Invalid(
                    "Worker terminated; verify hosted-device release before recording cleanup",
                ));
            }
            state.operation = next.clone();
            store.save(state).map_err(|_| CloudError::Persistence)
        })?;
        Ok((report, operation))
    }

    fn stop(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation) -> Result<()> {
        let CreateState::Bound { worker_id } = &state.operation else {
            return Err(Error::Invalid("Reconcile a bound worker before stopping it"));
        };
        let id = worker_id.clone();
        let spec = state
            .spec
            .clone()
            .ok_or(Error::Invalid("Missing worker specification"))?;
        let provider = self.client()?;
        let worker = provider.inspect(&id, cancel)?.ok_or(CloudError::WorkerLost)?;
        worker.verify(&spec)?;
        if state.requires_browserstack_release() {
            if worker.status() == WorkerStatus::Stopped {
                return Err(Error::Invalid(
                    "Worker stopped before remote-device release; resume it to verify cleanup",
                ));
            }
            self.revoke(store, &worker, cancel)?;
        }
        if state.profile.capabilities.browserstack.is_some() {
            state.browserstack_released = true;
            store.save(state)?;
        }
        let previous = (state.stop_requested, state.stage);
        state.stop_requested = true;
        state.stage = Stage::Stopping;
        store.save(state)?;
        if worker.status() != WorkerStatus::Stopped
            && let Err(error) = provider.stop(&spec, &id, cancel)
        {
            if matches!(
                error,
                CloudError::Unauthorized | CloudError::Rejected(_) | CloudError::Cancelled
            ) {
                (state.stop_requested, state.stage) = previous;
                store.save(state)?;
            }
            return Err(error.into());
        }
        state.worker = provider.inspect(&id, cancel)?;
        if !state
            .worker
            .as_ref()
            .is_some_and(|worker| worker.status() == WorkerStatus::Stopped)
        {
            store.save(state)?;
            return Err(Error::Invalid(
                "Stop requested but not confirmed. Reconcile the stop before resuming.",
            ));
        }
        state.stage = Stage::Stopped;
        store.save(state)
    }

    fn resume(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation) -> Result<()> {
        let CreateState::Bound { worker_id } = &state.operation else {
            return Err(Error::Invalid("No existing worker to resume"));
        };
        let worker_id = worker_id.clone();
        let spec = state
            .spec
            .as_ref()
            .ok_or(Error::Invalid("Missing worker specification"))?;
        let provider = self.client()?;
        let worker = provider.inspect(&worker_id, cancel)?.ok_or(CloudError::WorkerLost)?;
        worker.verify(spec)?;
        let requested = std::time::SystemTime::now();
        if worker.status() == WorkerStatus::Stopped {
            provider.start(spec, &worker_id, cancel)?;
        }
        state.timeline = Some(super::super::timeline::Timeline::resume_requested(requested));
        state.stop_requested = false;
        state.stage = Stage::Readiness;
        store.save(state)
    }

    fn release_devices(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation) -> Result<()> {
        let spec = state.spec.as_ref().ok_or(Error::Invalid("No worker specification"))?;
        let CreateState::Bound { worker_id } = &state.operation else {
            return Err(Error::Invalid("No bound worker"));
        };
        let provider = self.client()?;
        let worker = provider.inspect(worker_id, cancel)?.ok_or(CloudError::WorkerLost)?;
        worker.verify(spec)?;
        self.revoke(store, &worker, cancel)?;
        state.browserstack_released = true;
        store.save(state)
    }
}
