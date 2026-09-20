//! Explicit worker power actions, independent of local presentation lifetime.
use super::{
    Cancellation, CreateState, Error, Result, Stage,
    settings::Settings,
    state::{Deployment, Store},
};
use horizon_cloud::{WorkerStatus, runpod::RunPod};
use std::path::Path;

/// # Errors
/// Persists stop intent before provider I/O. A lost response never causes automatic resume.
pub fn stop(root: &Path, settings: &Settings, cancel: &Cancellation) -> Result<Deployment> {
    let store = Store::lock(root)?;
    let mut state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    let CreateState::Bound { worker_id } = &state.operation else {
        return Err(Error::Invalid("Reconcile a bound worker before stopping it"));
    };
    let id = worker_id.clone();
    let spec = state
        .spec
        .as_ref()
        .ok_or(Error::Invalid("Missing worker specification"))?;
    let provider = RunPod::new(settings.credential()?);
    let worker = provider
        .inspect(&id, cancel)?
        .ok_or(horizon_cloud::CloudError::WorkerLost)?;
    worker.verify(spec)?;
    if state.requires_browserstack_release() {
        if worker.status() == WorkerStatus::Stopped {
            return Err(Error::Invalid(
                "Worker stopped before remote-device release; resume it to verify cleanup",
            ));
        }
        let connection = super::ssh::Connection::new(&worker, settings, store.root())?;
        super::browser_auth::revoke(
            &connection,
            &super::command::Runner {
                cancel,
                emit: &|_| {},
                secrets: Vec::new(),
            },
        )?;
    }
    if state.profile.capabilities.browserstack.is_some() {
        state.browserstack_released = true;
        store.save(&state)?;
    }
    let previous = (state.stop_requested, state.stage);
    state.stop_requested = true;
    state.stage = Stage::Stopping;
    store.save(&state)?;
    if worker.status() != WorkerStatus::Stopped
        && let Err(error) = provider.stop(spec, &id, cancel)
    {
        if matches!(
            error,
            horizon_cloud::CloudError::Unauthorized
                | horizon_cloud::CloudError::Rejected
                | horizon_cloud::CloudError::Cancelled
        ) {
            (state.stop_requested, state.stage) = previous;
            store.save(&state)?;
        }
        return Err(error.into());
    }
    state.worker = provider.inspect(&id, cancel)?;
    if !state
        .worker
        .as_ref()
        .is_some_and(|worker| worker.status() == WorkerStatus::Stopped)
    {
        store.save(&state)?;
        return Err(Error::Invalid(
            "Stop requested but not confirmed. Reconcile the stop before resuming.",
        ));
    }
    state.stage = Stage::Stopped;
    store.save(&state)?;
    Ok(state)
}

/// # Errors
/// Resumes the existing worker only. Missing workers and sessions remain explicit losses.
pub fn resume(root: &Path, settings: &Settings, cancel: &Cancellation) -> Result<()> {
    let store = Store::lock(root)?;
    let mut state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    if state.stage == Stage::Stopping {
        return Err(Error::Invalid("Reconcile the pending stop before resuming"));
    }
    let CreateState::Bound { worker_id } = &state.operation else {
        return Err(Error::Invalid("No existing worker to resume"));
    };
    let spec = state
        .spec
        .as_ref()
        .ok_or(Error::Invalid("Missing worker specification"))?;
    let provider = RunPod::new(settings.credential()?);
    let worker = provider
        .inspect(worker_id, cancel)?
        .ok_or(horizon_cloud::CloudError::WorkerLost)?;
    worker.verify(spec)?;
    if worker.status() == WorkerStatus::Stopped {
        provider.start(spec, worker_id, cancel)?;
    }
    state.stop_requested = false;
    state.stage = Stage::Readiness;
    store.save(&state)
}

/// # Errors
/// Removes worker copies only after hosted browser release is confirmed.
pub fn revoke_browserstack(root: &Path, settings: &Settings, cancel: &Cancellation) -> Result<Deployment> {
    let store = Store::lock(root)?;
    let mut state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    let spec = state.spec.as_ref().ok_or(Error::Invalid("No worker specification"))?;
    let CreateState::Bound { worker_id } = &state.operation else {
        return Err(Error::Invalid("No bound worker"));
    };
    let provider = RunPod::new(settings.credential()?);
    let worker = provider
        .inspect(worker_id, cancel)?
        .ok_or(horizon_cloud::CloudError::WorkerLost)?;
    worker.verify(spec)?;
    let connection = super::ssh::Connection::new(&worker, settings, store.root())?;
    super::browser_auth::revoke(
        &connection,
        &super::command::Runner {
            cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        },
    )?;
    state.browserstack_released = true;
    store.save(&state)?;
    Ok(state)
}
