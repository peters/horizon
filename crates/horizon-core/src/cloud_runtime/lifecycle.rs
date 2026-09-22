//! Explicit worker power actions, independent of local presentation lifetime.
use super::{
    Cancellation, CreateState, Error, Result, Stage,
    settings::Settings,
    state::{Deployment, Store},
};
use horizon_cloud::{WorkerStatus, runpod::RunPod};
use std::path::Path;

#[derive(Debug)]
pub struct ReconciledDeployment {
    pub state: Deployment,
    pub report: horizon_cloud::runpod::recovery::Reconciliation,
}

/// # Errors
/// Refuses removal while any owned worker or workspace storage may remain.
pub fn can_remove(store: &Store, state: &Deployment) -> Result<bool> {
    if !matches!(state.operation, CreateState::Prepared | CreateState::Terminated { .. }) {
        return Ok(false);
    }
    match &state.spec {
        Some(spec) => Ok(!super::deployment::storage::retained(store, spec)?),
        None => Ok(!store.root().join("workspace-volume.json").exists()
            && !store.root().join("workspace-volume.required").exists()),
    }
}

/// # Errors
/// Checks only the recorded operation. A provider-confirmed worker hint cannot reset its fence.
/// Image builds, source preparation and agent credentials are not needed for recovery.
pub fn reconcile(
    root: &Path,
    settings: &Settings,
    worker_hint: Option<&str>,
    cancel: &Cancellation,
) -> Result<ReconciledDeployment> {
    let store = Store::lock(root)?;
    let mut state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    let spec = state.spec.clone().ok_or(Error::Invalid("No worker was requested"))?;
    if spec.operation_id != state.cloud_id || spec.profile != state.profile {
        return Err(Error::Invalid("Deployment and worker identities differ"));
    }
    let provider = RunPod::new(settings.credential()?);
    let mut operation = state.operation.clone();
    let report = provider.reconcile(&spec, &mut operation, worker_hint, cancel, |next| {
        if matches!(next, CreateState::Terminated { .. }) && state.requires_browserstack_release() {
            return Err(horizon_cloud::CloudError::Invalid(
                "Worker terminated; verify hosted-device release before recording cleanup",
            ));
        }
        state.operation = next.clone();
        store.save(&state).map_err(|_| horizon_cloud::CloudError::Persistence)
    })?;
    let changed = report.worker.is_some() || matches!(operation, CreateState::Terminated { .. });
    if let Some(worker) = &report.worker {
        state.worker = Some(worker.clone());
    }
    if matches!(operation, CreateState::Terminated { .. }) {
        if super::deployment::storage::retained(&store, &spec)? {
            return Err(Error::Invalid(
                "Worker termination is confirmed, but workspace storage remains; explicitly delete the cloud to finish cleanup",
            ));
        }
        state.worker = None;
        state.stage = Stage::Deleted;
    }
    if changed {
        store.save(&state)?;
    }
    Ok(ReconciledDeployment { state, report })
}

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
