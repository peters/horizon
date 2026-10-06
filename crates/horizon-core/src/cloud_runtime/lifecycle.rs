//! Explicit worker power actions, independent of local presentation lifetime.
use super::{
    Cancellation, CreateState, Error, Result, Stage, mutation,
    settings::Settings,
    ssh::Connection,
    state::{Deployment, Store},
};
use horizon_cloud::WorkerStatus;
use std::path::Path;

mod outside;
mod power;
pub use outside::{IdleSample, StopCause, check_lost_worker, may_stop_outside, worker_stopped};
#[cfg(all(test, unix))]
use power::Announce;
pub(in crate::cloud_runtime) use power::{request_resume, request_stop};

pub use super::deployment::hetzner::idle::IdleCheck;

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
        horizon_cloud::provider::Description::of(&self.state.profile).stopped
    }
}

/// # Errors
/// Refuses removal while any owned worker or workspace storage may remain.
pub fn can_remove(store: &Store, state: &Deployment) -> Result<bool> {
    if !matches!(state.operation, CreateState::Prepared | CreateState::Terminated { .. }) {
        return Ok(false);
    }
    Ok(!super::providers::retained(store, state)?)
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
    reconcile_locked(&store, settings, worker_hint, cancel)
}

pub(in crate::cloud_runtime) fn reconcile_locked(
    store: &Store,
    settings: &Settings,
    worker_hint: Option<&str>,
    cancel: &Cancellation,
) -> Result<ReconciledDeployment> {
    let mut state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    let spec = state.spec.as_ref().ok_or(Error::Invalid("No worker was requested"))?;
    if spec.operation_id != state.cloud_id || spec.profile != state.profile {
        return Err(Error::Invalid("Deployment and worker identities differ"));
    }
    let (report, operation) =
        super::providers::lifecycle(&state, settings).check(store, &mut state, worker_hint, cancel)?;
    let changed = report.worker.is_some() || matches!(operation, CreateState::Terminated { .. });
    if let Some(worker) = &report.worker {
        state.worker = Some(worker.clone());
        record_stopped(&mut state);
    }
    if matches!(operation, CreateState::Terminated { .. }) {
        if super::providers::retained(store, &state)? {
            return Err(Error::Invalid(
                "Worker termination is confirmed, but workspace storage remains; explicitly delete the cloud to finish cleanup",
            ));
        }
        super::deployment::drop_replacement(store, &mut state)?;
        state.worker = None;
        state.stage = Stage::Deleted;
    }
    if changed {
        store.save(&state)?;
    }
    Ok(ReconciledDeployment { state, report })
}

/// # Errors
/// The SSH endpoint of the cloud's running worker as the provider reports it now,
/// through the same check as [`reconcile`], with the host key Horizon pinned for the
/// worker. The provider may assign a new public port whenever the worker starts.
/// Refuses a worker that is not running, or whose host key Horizon has not pinned.
pub fn endpoint(root: &Path, settings: &Settings, cancel: &Cancellation) -> Result<Connection> {
    let reconciled = reconcile(root, settings, None, cancel)?;
    pinned_endpoint(&reconciled, settings, root)
}

fn pinned_endpoint(reconciled: &ReconciledDeployment, settings: &Settings, root: &Path) -> Result<Connection> {
    let worker = reconciled
        .report
        .worker
        .as_ref()
        .filter(
            |worker| matches!(&reconciled.state.operation, CreateState::Bound { worker_id } if *worker_id == worker.id),
        )
        .ok_or(Error::Invalid(
            "The cloud has no running worker; deploy, resume or reconnect it first",
        ))?;
    if worker.status() != WorkerStatus::Running {
        return Err(Error::Invalid(
            "The worker is not running; resume or reconnect it first",
        ));
    }
    let connection = Connection::new(worker, settings, root)?;
    if !connection.known_hosts.try_exists()? {
        return Err(Error::Invalid(
            "Horizon has not pinned this worker's host key yet; reconnect it first",
        ));
    }
    Ok(connection)
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
    stop_locked(&store, settings, cancel, mutation::IGNORE)
}

pub(in crate::cloud_runtime) fn stop_locked(
    store: &Store,
    settings: &Settings,
    cancel: &Cancellation,
    observe: mutation::Observer<'_>,
) -> Result<Deployment> {
    let mut state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    state.refuse_pending_replacement()?;
    super::providers::lifecycle(&state, settings).stop(store, &mut state, cancel, observe)?;
    Ok(state)
}

/// # Errors
/// Reads a Hetzner cloud's idle record and stops the cloud, as Stop does, once it
/// has been idle for its whole period. A `RunPod` worker stops itself, so its record
/// is only read: it reports `Active` even when the worker is about to stop. Any
/// other cloud, and a worker that keeps no record, reports `NotWatched`.
pub fn idle_check(root: &Path, settings: &Settings, cancel: &Cancellation) -> Result<IdleCheck> {
    match outside::sample(root, settings, cancel)? {
        IdleCheck::NotWatched => super::deployment::hetzner::idle::check(root, settings, cancel),
        sampled => Ok(sampled),
    }
}

/// # Errors
/// Resumes a stopped cloud as its provider stops it (see `provider::StoppedCost`):
/// `RunPod` resumes the existing worker only, while a Hetzner stop released its
/// server, so the next reconnect creates a new one on the same workspace volume.
/// Missing workers and sessions remain explicit losses.
pub fn resume(root: &Path, settings: &Settings, cancel: &Cancellation) -> Result<()> {
    let store = Store::lock(root)?;
    resume_locked(&store, settings, cancel, mutation::IGNORE)
}

pub(in crate::cloud_runtime) fn resume_locked(
    store: &Store,
    settings: &Settings,
    cancel: &Cancellation,
    observe: mutation::Observer<'_>,
) -> Result<()> {
    let mut state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    state.refuse_pending_replacement()?;
    if state.stage == Stage::Stopping {
        return Err(Error::Invalid("Reconcile the pending stop before resuming"));
    }
    super::providers::lifecycle(&state, settings).resume(store, &mut state, cancel, observe)
}

/// # Errors
/// Removes worker copies only after hosted browser release is confirmed.
pub fn revoke_browserstack(root: &Path, settings: &Settings, cancel: &Cancellation) -> Result<Deployment> {
    let store = Store::lock(root)?;
    let mut state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    state.refuse_pending_replacement()?;
    super::providers::lifecycle(&state, settings).release_devices(&store, &mut state, cancel)?;
    Ok(state)
}

#[cfg(all(test, unix))]
mod mutation_tests;

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
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(matches!(
            resume(root.path(), &settings, &cancelled).unwrap_err(),
            Error::Provider(horizon_cloud::CloudError::Cancelled)
        ));
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
        // A Hetzner rebuild runs on a new server; this profile has no recipe to rebuild.
        let error = super::super::deployment::replacement::rebuild(&request, "cpu", &cancel, &|_| {}).unwrap_err();
        assert_eq!(
            error.to_string(),
            "This cloud's profile has no build section, so there is no recipe to rebuild its image from"
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

    #[test]
    fn the_endpoint_follows_the_reported_port_and_needs_the_pinned_host_key() {
        use horizon_cloud::runpod::recovery::{Outcome, Reconciliation};
        let temp = tempfile::tempdir().unwrap();
        pending(temp.path(), false);
        let state = Store::lock(temp.path()).unwrap().load().unwrap().unwrap();
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":"/unused","ssh_identity_file":"/synthetic/identity","docker_config":"/unused",
            "registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]
        }))
        .unwrap();
        let reported = |id: &str, status: &str, port: u16| ReconciledDeployment {
            state: state.clone(),
            report: Reconciliation {
                operation_id: "pending".into(),
                outcome: Outcome::Found { worker_id: id.into() },
                worker: Some(
                    serde_json::from_value(serde_json::json!({
                        "id":id,"name":"pending","imageName":"registry.example/worker","desiredStatus":status,
                        "publicIp":"192.0.2.10","portMappings":{"22":port}
                    }))
                    .unwrap(),
                ),
            },
        };
        let error = |reconciled: &ReconciledDeployment| {
            pinned_endpoint(reconciled, &settings, temp.path())
                .unwrap_err()
                .to_string()
        };
        assert!(error(&reported("worker1", "RUNNING", 40022)).contains("not pinned"));
        std::fs::write(
            temp.path().join("known-hosts-worker1"),
            b"horizon-cloud-worker1 ssh-ed25519 AAAA\n",
        )
        .unwrap();
        for port in [40022, 41517] {
            let connection = pinned_endpoint(&reported("worker1", "RUNNING", port), &settings, temp.path()).unwrap();
            assert_eq!((connection.host.as_str(), connection.port), ("192.0.2.10", port));
            assert_eq!(connection.host_key_alias, "horizon-cloud-worker1");
            assert_eq!(connection.known_hosts, temp.path().join("known-hosts-worker1"));
            assert_eq!(connection.identity, Path::new("/synthetic/identity"));
        }
        assert!(error(&reported("worker1", "EXITED", 40022)).contains("not running"));
        assert!(error(&reported("worker1", "STARTING", 40022)).contains("not running"));
        assert!(
            error(&reported("worker2", "RUNNING", 40022)).contains("no running worker"),
            "only the recorded worker is this cloud's"
        );
        let mut missing = reported("worker1", "RUNNING", 40022);
        missing.report.worker = None;
        assert!(error(&missing).contains("no running worker"));
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
