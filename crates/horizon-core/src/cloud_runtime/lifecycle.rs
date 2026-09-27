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
impl ReconciledDeployment {
    /// The provider returned this cloud's verified worker and reports it stopped.
    #[must_use]
    pub fn confirmed_stopped(&self) -> bool {
        self.state.stage == Stage::Stopped
            && match self.stopped() {
                // Stopping deleted the server, which check proved gone; there may be
                // no recorded worker to report.
                horizon_cloud::provider::StoppedCost::ServerDeleted => {
                    matches!(
                        self.report.outcome,
                        horizon_cloud::runpod::recovery::Outcome::Inactive { .. }
                    )
                }
                horizon_cloud::provider::StoppedCost::WorkerKept => self
                    .report
                    .worker
                    .as_ref()
                    .is_some_and(|worker| worker.status() == WorkerStatus::Stopped),
            }
    }

    /// What a stop kept of this cloud's worker, and so what Resume does.
    #[must_use]
    pub fn stopped(&self) -> horizon_cloud::provider::StoppedCost {
        horizon_cloud::provider::by_id(&self.state.profile.provider)
            .map_or(horizon_cloud::provider::StoppedCost::WorkerKept, |provider| {
                provider.stopped
            })
    }
}

/// # Errors
/// Refuses removal while any owned worker or workspace storage may remain.
pub fn can_remove(store: &Store, state: &Deployment) -> Result<bool> {
    if !matches!(state.operation, CreateState::Prepared | CreateState::Terminated { .. }) {
        return Ok(false);
    }
    if state.profile.provider == horizon_cloud::hetzner::PROVIDER {
        return Ok(!super::deployment::hetzner::retained(store.root())?);
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
    let (report, operation) = if hetzner(&state) {
        let report = super::deployment::hetzner::lifecycle::reconcile(&store, &mut state, settings, cancel)?;
        (report, state.operation.clone())
    } else {
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
        (report, operation)
    };
    let changed = report.worker.is_some() || matches!(operation, CreateState::Terminated { .. });
    if let Some(worker) = &report.worker {
        state.worker = Some(worker.clone());
        record_stopped(&mut state);
    }
    if matches!(operation, CreateState::Terminated { .. }) {
        let retained = if hetzner(&state) {
            super::deployment::hetzner::retained(store.root())?
        } else {
            super::deployment::storage::retained(&store, &state)?
        };
        if retained {
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

/// Hetzner clouds release their server on stop; see `deployment::hetzner::lifecycle`.
fn hetzner(state: &Deployment) -> bool {
    state.profile.provider == horizon_cloud::hetzner::PROVIDER
}

/// Hetzner profiles cannot request hosted devices, so a Hetzner cloud has none to
/// release; nothing here may reach `RunPod` for it.
fn refuse_hetzner(state: &Deployment) -> Result<()> {
    if state.profile.provider == horizon_cloud::hetzner::PROVIDER {
        return Err(Error::Invalid("Hetzner clouds hold no hosted devices to release"));
    }
    Ok(())
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
    if hetzner(&state) {
        super::deployment::hetzner::lifecycle::stop(&store, &mut state, settings, cancel)?;
        return Ok(state);
    }
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
    if hetzner(&state) {
        return super::deployment::hetzner::lifecycle::resume(&store, &mut state);
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
    let requested = std::time::SystemTime::now();
    if worker.status() == WorkerStatus::Stopped {
        provider.start(spec, worker_id, cancel)?;
    }
    state.timeline = Some(super::timeline::Timeline::resume_requested(requested));
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
    refuse_hetzner(&state)?;
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

    #[test]
    fn a_hetzner_stop_is_confirmed_by_its_released_server_being_gone_without_a_worker() {
        use horizon_cloud::{
            provider::StoppedCost,
            runpod::recovery::{Outcome, Reconciliation},
        };
        let profile = serde_json::json!({"provider":"hetzner","image":"registry.example/worker","cpu":4,"memory_gb":8});
        let state: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"hetzner-cloud","repository":"/synthetic","revision":"a".repeat(40),
            "profile":profile,"stage":"Stopped","operation":{"state":"bound","worker_id":"42"},
            "spec":null,"worker":null,"sessions":[],"stop_requested":true
        }))
        .unwrap();
        let report = Reconciliation {
            operation_id: "hetzner-cloud".into(),
            outcome: Outcome::Inactive { worker_id: "42".into() },
            worker: None,
        };
        let reconciled = ReconciledDeployment { state, report };
        assert_eq!(
            reconciled.stopped(),
            StoppedCost::ServerDeleted,
            "resume creates a new server"
        );
        assert!(reconciled.confirmed_stopped());
    }

    #[test]
    fn hetzner_clouds_never_reach_runpod_lifecycle_code() {
        let root = tempfile::tempdir().unwrap();
        let profile = serde_json::json!({"provider":"hetzner","image":"registry.example/worker","cpu":4,"memory_gb":8});
        let state: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"hetzner-cloud","repository":"/synthetic","revision":"a".repeat(40),
            "profile":profile,"stage":"Ready","operation":{"state":"bound","worker_id":"42"},
            "spec":{
                "operation_id":"hetzner-cloud","image_digest":format!("registry.example/worker@sha256:{}", "a".repeat(64)),
                "profile":profile,"public_key":"unused","registry_auth_id":null,"gpu_types":[],
                "cpu_flavors":["cx23"],"data_centers":["hel1"]
            },
            "worker":null,"sessions":[]
        }))
        .unwrap();
        let store = Store::lock(root.path()).unwrap();
        store.save(&state).unwrap();
        drop(store);
        // A settings file whose RunPod key cannot even be read: any RunPod path would fail differently.
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":"/missing","ssh_identity_file":"/missing","docker_config":"/missing",
            "registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]
        }))
        .unwrap();
        let cancel = Cancellation::default();
        // Every Hetzner action goes to the Hetzner path, which needs Hetzner settings.
        let no_hetzner = "Add a hetzner section to the cloud settings before deploying a Hetzner cloud";
        assert_eq!(
            reconcile(root.path(), &settings, None, &cancel)
                .unwrap_err()
                .to_string(),
            no_hetzner
        );
        assert_eq!(
            stop(root.path(), &settings, &cancel).unwrap_err().to_string(),
            no_hetzner
        );
        assert_eq!(
            resume(root.path(), &settings, &cancel).unwrap_err().to_string(),
            "Stop the Hetzner cloud before resuming it"
        );
        assert_eq!(
            revoke_browserstack(root.path(), &settings, &cancel)
                .unwrap_err()
                .to_string(),
            "Hetzner clouds hold no hosted devices to release"
        );
        let error = super::super::deployment::terminate(root.path(), &settings, &cancel, &|_| {}).unwrap_err();
        assert_eq!(error.to_string(), no_hetzner);
        let request = super::super::deployment::Request::new(
            state.cloud_id.clone(),
            state.repository.clone(),
            state.revision.clone(),
            state.profile.clone(),
            root.path().into(),
            settings.clone(),
        );
        let error = super::super::deployment::replacement::rebuild(&request, "cpu", &cancel, &|_| {}).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Rebuilding a Hetzner cloud's image is not available yet"
        );
        let store = Store::lock(root.path()).unwrap();
        assert_eq!(
            store.load().unwrap().unwrap().stage,
            Stage::Ready,
            "nothing was recorded"
        );
        let mut deleted = state;
        deleted.operation = CreateState::Terminated { worker_id: "42".into() };
        assert!(
            can_remove(&store, &deleted).unwrap(),
            "nothing on Hetzner was ever recorded"
        );
        std::fs::write(
            root.path().join("hetzner.json"),
            br#"{"volume":{"state":"bound","worker_id":"9"}}"#,
        )
        .unwrap();
        assert!(
            !can_remove(&store, &deleted).unwrap(),
            "the workspace volume may still exist"
        );
        std::fs::write(
            root.path().join("hetzner.json"),
            br#"{"volume":{"state":"prepared"},"key":"ssh-ed25519 AAAA"}"#,
        )
        .unwrap();
        assert!(
            !can_remove(&store, &deleted).unwrap(),
            "the registered SSH key may still exist"
        );
    }

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
