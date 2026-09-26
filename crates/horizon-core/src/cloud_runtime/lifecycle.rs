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
        Some(_) => Ok(!super::deployment::storage::retained(store, state)?),
        None => Ok(!store.root().join("workspace-volume.json").try_exists()?
            && !store.root().join("workspace-volume.required").try_exists()?),
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
    let spec = state.spec.as_ref().ok_or(Error::Invalid("No worker was requested"))?;
    if spec.operation_id != state.cloud_id || spec.profile != state.profile {
        return Err(Error::Invalid("Deployment and worker identities differ"));
    }
    let provider = RunPod::new(settings.credential()?);
    // Settling commits only a new image and pull credential for the same worker.
    super::deployment::replacement::settle(&provider, &store, &mut state, cancel)?;
    let spec = state.spec.clone().ok_or(Error::Invalid("No worker was requested"))?;
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
        record_stopped(&mut state);
    }
    if matches!(operation, CreateState::Terminated { .. }) {
        if super::deployment::storage::retained(&store, &state)? {
            return Err(Error::Invalid(
                "Worker termination is confirmed, but workspace storage remains; explicitly delete the cloud to finish cleanup",
            ));
        }
        super::deployment::drop_replacement(&store, &mut state)?;
        state.worker = None;
        state.stage = Stage::Deleted;
    }
    if changed {
        store.save(&state)?;
    }
    Ok(ReconciledDeployment { state, report })
}

/// A verified worker that stopped itself or was stopped elsewhere becomes an
/// ordinary stopped cloud, so only an explicit Resume starts it again.
fn record_stopped(state: &mut Deployment) {
    if matches!(state.operation, CreateState::Bound { .. })
        && !matches!(state.stage, Stage::Replace | Stage::Deleted)
        && state
            .worker
            .as_ref()
            .is_some_and(|worker| worker.status() == WorkerStatus::Stopped)
    {
        state.stop_requested = true;
        state.stage = Stage::Stopped;
    }
}

/// # Errors
/// Persists stop intent before provider I/O. A lost response never causes automatic resume.
pub fn stop(root: &Path, settings: &Settings, cancel: &Cancellation) -> Result<Deployment> {
    let store = Store::lock(root)?;
    let mut state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    state.refuse_pending_replacement()?;
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
                | horizon_cloud::CloudError::Rejected(_)
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
    state.refuse_pending_replacement()?;
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
    state.refuse_pending_replacement()?;
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::cloud_runtime::state::{OperationId, REPLACEMENT_PENDING, ReplacementImage};

    fn pending(root: &Path, requested: bool) -> Vec<u8> {
        let profile = serde_json::json!({"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8});
        let mut state: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"pending","repository":"/synthetic","revision":"a".repeat(40),
            "profile":profile,"stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},
            "spec":{
                "operation_id":"pending","image_digest":format!("registry.example/worker@sha256:{}", "a".repeat(64)),
                "profile":profile,"public_key":"unused","registry_auth_id":null,"gpu_types":[],
                "cpu_flavors":["cpu3c"],"data_centers":[]
            },
            "worker":null,"sessions":[]
        }))
        .unwrap();
        state
            .begin_replacement(OperationId::generate(), "c".repeat(40))
            .unwrap();
        state
            .replacement_built(ReplacementImage {
                digest: format!("registry.example/worker@sha256:{}", "b".repeat(64)),
                registry_auth_id: None,
                registry_generation: None,
            })
            .unwrap();
        if requested {
            state.request_replacement().unwrap();
        }
        Store::lock(root).unwrap().save(&state).unwrap();
        std::fs::read(root.join("deployment.json")).unwrap()
    }

    #[test]
    fn a_worker_found_stopped_is_recorded_as_stopped_and_stays_stopped() {
        let worker = |status: &str| {
            serde_json::from_value::<horizon_cloud::Worker>(serde_json::json!({
                "id":"worker1","name":"pending","imageName":"registry.example/worker","desiredStatus":status
            }))
            .unwrap()
        };
        let temp = tempfile::tempdir().unwrap();
        pending(temp.path(), false);
        let mut state = Store::lock(temp.path()).unwrap().load().unwrap().unwrap();
        for (stage, status, stopped) in [
            (Stage::Ready, "EXITED", true),
            (Stage::Stopping, "EXITED", true),
            (Stage::Ready, "RUNNING", false),
            (Stage::Replace, "EXITED", false),
            (Stage::Deleted, "EXITED", false),
        ] {
            state.stage = stage;
            state.stop_requested = false;
            state.worker = Some(worker(status));
            record_stopped(&mut state);
            assert_eq!(state.stage == Stage::Stopped, stopped, "{stage:?} {status}");
            assert_eq!(state.stop_requested, stopped);
        }
        state.stage = Stage::Ready;
        state.operation = CreateState::Requested;
        state.worker = Some(worker("EXITED"));
        record_stopped(&mut state);
        assert_eq!(state.stage, Stage::Ready);
    }

    fn refused<T>(result: &Result<T>) -> bool {
        matches!(result, Err(Error::Invalid(message)) if *message == REPLACEMENT_PENDING)
    }

    #[test]
    fn worker_actions_refuse_a_pending_image_replacement_before_provider_io() {
        let temp = tempfile::tempdir().unwrap();
        // Missing credentials: an action that got past its guard would fail differently.
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":temp.path().join("missing"),"ssh_identity_file":temp.path().join("missing"),
            "docker_config":temp.path().join("docker"),"registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]
        }))
        .unwrap();
        let cancel = Cancellation::default();
        for requested in [false, true] {
            let root = temp.path().join(format!("cloud-{requested}"));
            let saved = pending(&root, requested);
            assert!(refused(&stop(&root, &settings, &cancel)));
            assert!(refused(&resume(&root, &settings, &cancel)));
            assert!(refused(&revoke_browserstack(&root, &settings, &cancel)));
            // The provider check needs the provider, which these settings cannot reach.
            assert!(reconcile(&root, &settings, None, &cancel).is_err());
            assert_eq!(std::fs::read(root.join("deployment.json")).unwrap(), saved);
        }
        // Identity is checked before a pending update is settled through the provider.
        let root = temp.path().join("cloud-foreign");
        pending(&root, true);
        let store = Store::lock(&root).unwrap();
        let mut state = store.load().unwrap().unwrap();
        state.cloud_id = "foreign".into();
        store.save(&state).unwrap();
        drop(store);
        let error = reconcile(&root, &settings, None, &cancel).unwrap_err().to_string();
        assert!(error.contains("identities differ"), "{error}");
    }
}
