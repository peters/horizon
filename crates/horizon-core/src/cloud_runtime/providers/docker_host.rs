//! A Docker host's lifecycle and deployment: `horizon_cloud::docker_host` on the saved
//! host the cloud records, with its journal in `docker-host.json` beside the deployment.
use super::{
    super::{
        Error, Stage, WorkerContract,
        command::Runner,
        deployment::{Compute, Request},
        mutation::State as Mutation,
        progress::Progress,
        ssh::Connection,
        timeline::AWAITING_SERVICES,
    },
    Cancellation, CreateState, Deployment, Event, Lifecycle, Observer, Result, Settings, Store,
};
use horizon_cloud::{
    CloudError, WorkerSpec,
    docker_host::{self, Engine, Host, Journal},
    runpod::recovery::{Outcome, Reconciliation},
};
use std::{
    io::Write as _,
    path::Path,
    time::{Duration, Instant},
};

const JOURNAL: &str = "docker-host.json";
const AMBIGUOUS: &str = "Choose one of the saved Docker hosts for this cloud";
const PROBE_INTERVAL: Duration = Duration::from_secs(1);
const TIMEOUT: &str = "Worker readiness timed out; the container remains on the Docker host for inspection or deletion";

/// The saved host a cloud runs on: the one its placement names, or the only saved host.
/// The spec keeps it as its CPU flavor and data center.
pub(super) fn hosts(settings: &Settings) -> Result<Vec<String>> {
    // Settings built in code skip `Settings::load`, so the hosts are checked here too.
    for host in &settings.docker_hosts {
        host.validate()?;
    }
    let chosen = settings
        .placement
        .as_ref()
        .map(|placement| placement.data_centers.as_slice())
        .unwrap_or_default();
    let names: Vec<String> = settings
        .docker_hosts
        .iter()
        .map(|host| host.name.clone())
        .filter(|name| chosen.is_empty() || chosen.contains(name))
        .collect();
    match names.len() {
        0 => Err(Error::Invalid(
            "Add the Docker host to docker_hosts in the cloud settings file first",
        )),
        1 => Ok(names),
        _ => Err(Error::Invalid(AMBIGUOUS)),
    }
}

/// Refuses a cloud without one saved host, and an image the host would need a registry
/// login for: the docker CLI there has none, and Horizon sends it no credential.
pub(super) fn preflight(profile: &horizon_cloud::Profile, settings: &Settings) -> Result<()> {
    // Only a host to choose from is needed here: a reconnect resolves the host its cloud
    // recorded, and a new cloud's spec refuses an ambiguous choice before anything is made.
    match hosts(settings) {
        Err(Error::Invalid(AMBIGUOUS)) => {}
        other => drop(other?),
    }
    let private = settings
        .registries
        .as_ref()
        .map(|registries| registries.select(&profile.image));
    if profile.build.is_some() || private.transpose()?.flatten().is_some() {
        return Err(Error::Invalid(
            "A Docker host runs public images only; choose a profile without a build or a private registry",
        ));
    }
    Ok(())
}

/// The container or the workspace volume may still exist.
pub(super) fn retained(store: &Store) -> Result<bool> {
    Ok(load(store.root())?.volume)
}

fn load(root: &Path) -> Result<Journal> {
    match std::fs::read(root.join(JOURNAL)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| Error::Json),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Journal::default()),
        Err(error) => Err(error.into()),
    }
}

/// Syncs the file and its directory, as the deployment record does.
fn save(root: &Path, journal: &Journal) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    file.write_all(&serde_json::to_vec_pretty(journal).map_err(|_| Error::Json)?)?;
    file.as_file().sync_all()?;
    file.persist(root.join(JOURNAL)).map_err(|error| error.error)?;
    #[cfg(unix)]
    std::fs::File::open(root)?.sync_all()?;
    Ok(())
}

/// The host the cloud recorded, or the one its spec names before anything was created.
fn host<'a>(settings: &'a Settings, journal: &Journal, spec: &WorkerSpec) -> Result<&'a Host> {
    let ((Some(name), _) | (None, [name])) = (&journal.host, spec.data_centers.as_slice()) else {
        return Err(Error::Invalid(AMBIGUOUS));
    };
    let host = settings
        .docker_hosts
        .iter()
        .find(|host| host.name == *name)
        .ok_or(Error::Invalid(
            "This cloud's Docker host is no longer saved; save it again under the same name",
        ))?;
    host.validate()?;
    // A name saved again for another engine must not answer for this cloud's resources.
    journal.admits(host)?;
    Ok(host)
}

fn engine<'a>(host: &'a Host, store: &Store, cancel: &'a Cancellation) -> Engine<'a> {
    Engine {
        host,
        local_config: store.root().join("docker-config"),
        cancel,
    }
}

/// The deployment record and the journal, each saved durably before the Docker request
/// it guards; the first save also records a pending mutation.
struct Saved<'a> {
    store: &'a Store,
    state: &'a mut Deployment,
    observe: Observer<'a>,
    pending: bool,
    failed: Option<Error>,
}

impl Saved<'_> {
    fn keep(&mut self, saved: Result<()>) -> std::result::Result<(), CloudError> {
        let saved = match saved {
            Ok(()) if !self.pending => (self.observe)(Mutation::Pending).map(|_| self.pending = true),
            other => other,
        };
        saved.map_err(|error| {
            self.failed = Some(error);
            CloudError::Persistence
        })
    }

    fn finish<T>(self, result: std::result::Result<T, CloudError>) -> Result<T> {
        result.map_err(|error| match (error, self.failed) {
            (CloudError::Persistence, Some(failed)) => failed,
            (error, _) => error.into(),
        })
    }
}

impl docker_host::Records for Saved<'_> {
    fn journal(&mut self, journal: &Journal) -> std::result::Result<(), CloudError> {
        let saved = save(self.store.root(), journal);
        self.keep(saved)
    }

    fn operation(&mut self, operation: &CreateState) -> std::result::Result<(), CloudError> {
        self.state.operation = operation.clone();
        let saved = self.store.save(self.state);
        self.keep(saved)
    }

    fn mutation(&mut self) -> std::result::Result<(), CloudError> {
        self.keep(Ok(()))
    }
}

pub(super) struct DockerHost<'a> {
    pub(super) settings: &'a Settings,
}

impl Compute for DockerHost<'_> {
    fn provision_ready(
        &self,
        request: &Request,
        store: &Store,
        runner: &Runner<'_>,
        state: &mut Deployment,
        spec: &WorkerSpec,
        observe: Observer<'_>,
    ) -> Result<(Connection, WorkerContract)> {
        if spec.operation_id != state.cloud_id || spec.profile != state.profile {
            return Err(Error::Invalid("Deployment and worker identities differ"));
        }
        let mut journal = load(store.root())?;
        let host = host(self.settings, &journal, spec)?;
        let engine = engine(host, store, runner.cancel);
        let mut operation = state.operation.clone();
        let mut saved = Saved {
            store,
            state,
            observe,
            pending: false,
            failed: None,
        };
        let provisioned = docker_host::provision(&engine, host, spec, &mut operation, &mut journal, &mut saved);
        let pending = saved.pending;
        let worker = saved.finish(provisioned)?;
        state.worker = Some(worker);
        store.save(state)?;
        if pending {
            observe(Mutation::Settled)?;
        }
        wait(request, store, runner, state, &engine, spec)
    }
}

/// Waits until the container's sshd answers through the host and the worker passes its
/// contract. The worker host key is pinned on the first answer, as for other providers.
fn wait(
    request: &Request,
    store: &Store,
    runner: &Runner<'_>,
    state: &mut Deployment,
    engine: &Engine<'_>,
    spec: &WorkerSpec,
) -> Result<(Connection, WorkerContract)> {
    let deadline = Instant::now() + Duration::from_secs(u64::from(state.profile.bootstrap.readiness_seconds));
    state.stage = Stage::Readiness;
    store.save(state)?;
    (runner.emit)(Event::stage(state.stage));
    let CreateState::Bound { worker_id } = state.operation.clone() else {
        return Err(Error::Invalid("Missing worker identity"));
    };
    loop {
        let container = docker_host::find(engine, &state.cloud_id)?
            .filter(|container| container.id == worker_id)
            .ok_or(CloudError::WorkerLost)?;
        if !container.running {
            return Err(Error::Invalid(
                "The worker container stopped on the Docker host; check its logs there",
            ));
        }
        let worker = docker_host::describe(&container, engine.host, spec)?;
        let connection = Connection::new(&worker, &request.settings, store.root())?;
        state.worker = Some(worker);
        store.save(state)?;
        (runner.emit)(Event::Progress(Progress::activity(AWAITING_SERVICES)));
        if let Ok(contract) = connection.ready(runner, &state.profile.capabilities, remaining(deadline, runner)?) {
            return Ok((connection, contract));
        }
        std::thread::sleep(PROBE_INTERVAL.min(remaining(deadline, runner)?));
    }
}

fn remaining(deadline: Instant, runner: &Runner<'_>) -> Result<Duration> {
    runner.cancel.check()?;
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or(Error::Invalid(TIMEOUT))
}

impl DockerHost<'_> {
    /// Starts (`on`) or stops the bound container as a Resume or a Stop records it.
    fn power(
        &self,
        store: &Store,
        state: &mut Deployment,
        cancel: &Cancellation,
        observe: Observer<'_>,
        on: bool,
    ) -> Result<()> {
        let spec = state.spec.as_ref().ok_or(Error::Invalid("No worker was requested"))?;
        let CreateState::Bound { worker_id } = state.operation.clone() else {
            return Err(Error::Invalid("Reconcile a bound worker first"));
        };
        let journal = load(store.root())?;
        if on && journal.deleting {
            return Err(Error::Invalid("Finish deleting this cloud before it starts again"));
        }
        let host = host(self.settings, &journal, spec)?;
        let (engine, cloud_id) = (engine(host, store, cancel), state.cloud_id.clone());
        journal.confirms(&engine)?;
        let needed = match docker_host::find(&engine, &cloud_id)? {
            Some(container) if container.id == worker_id => container.running != on,
            Some(_) => return Err(CloudError::IdentityMismatch.into()),
            None => return Err(CloudError::WorkerLost.into()),
        };
        let request = |announce: &mut dyn FnMut() -> std::result::Result<(), CloudError>| {
            docker_host::power(&engine, &cloud_id, &worker_id, on, announce)
        };
        if on {
            super::super::lifecycle::request_resume(store, state, needed, request, observe)
        } else {
            super::super::lifecycle::request_stop(store, state, needed, request, observe)
        }
    }
}

impl Lifecycle for DockerHost<'_> {
    fn check(
        &self,
        store: &Store,
        state: &mut Deployment,
        worker_hint: Option<&str>,
        cancel: &Cancellation,
    ) -> Result<(Reconciliation, CreateState)> {
        let spec = state.spec.clone().ok_or(Error::Invalid("No worker was requested"))?;
        let journal = load(store.root())?;
        let host = host(self.settings, &journal, &spec)?;
        let engine = engine(host, store, cancel);
        // A missing container counts only on the engine the cloud was created on.
        journal.confirms(&engine)?;
        let found = docker_host::find(&engine, &state.cloud_id)?;
        let worker = found
            .as_ref()
            .map(|container| docker_host::describe(container, host, &spec))
            .transpose()?;
        if worker_hint.is_some_and(|hint| worker.as_ref().is_none_or(|worker| worker.id != hint)) {
            return Err(Error::Invalid("The worker ID is not this cloud's container"));
        }
        // The container's name fences a second one, so one found for a request is the cloud's.
        if let (CreateState::Requested, Some(worker)) = (&state.operation, &worker) {
            state.operation = CreateState::Bound {
                worker_id: worker.id.clone(),
            };
            store.save(state)?;
        }
        let outcome = match (state.operation.clone(), &found) {
            (CreateState::Bound { worker_id }, Some(container)) if container.id == worker_id && container.running => {
                Outcome::Found { worker_id }
            }
            (CreateState::Bound { worker_id }, Some(container)) if container.id == worker_id => {
                Outcome::Inactive { worker_id }
            }
            (_, Some(_)) => return Err(CloudError::IdentityMismatch.into()),
            (CreateState::Prepared, None) => Outcome::Prepared,
            (CreateState::Requested, None) => Outcome::Unresolved,
            (CreateState::Bound { worker_id }, None) => Outcome::Missing { worker_id },
            (CreateState::Terminated { worker_id }, None) => Outcome::Terminated { worker_id },
        };
        let report = Reconciliation {
            operation_id: state.cloud_id.clone(),
            outcome,
            worker,
        };
        Ok((report, state.operation.clone()))
    }

    fn stop(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation, observe: Observer<'_>) -> Result<()> {
        self.power(store, state, cancel, observe, false)?;
        let (report, _) = self.check(store, state, None, cancel)?;
        state.worker = report.worker;
        if !matches!(report.outcome, Outcome::Inactive { .. }) {
            store.save(state)?;
            return Err(Error::Invalid(
                "Stop requested but not confirmed. Reconcile the stop before resuming.",
            ));
        }
        state.stage = Stage::Stopped;
        store.save(state)?;
        observe(Mutation::Settled).map(drop)
    }

    /// Starts the same container on its pinned port; the reconnect that follows waits
    /// for its sshd.
    fn resume(
        &self,
        store: &Store,
        state: &mut Deployment,
        cancel: &Cancellation,
        observe: Observer<'_>,
    ) -> Result<()> {
        self.power(store, state, cancel, observe, true)
    }

    /// Docker host profiles cannot request hosted devices.
    fn release_devices(&self, _store: &Store, _state: &mut Deployment, _cancel: &Cancellation) -> Result<()> {
        Err(Error::Invalid("Docker host clouds hold no hosted devices to release"))
    }

    fn delete(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation, emit: &dyn Fn(Event)) -> Result<()> {
        let spec = state.spec.clone().ok_or(Error::Invalid("No worker was requested"))?;
        let mut journal = load(store.root())?;
        let host = host(self.settings, &journal, &spec)?;
        emit(Event::stage(Stage::DeleteWorker));
        // Once deletion starts it runs to the end; each step is recorded so it can resume.
        cancel.check()?;
        let committed = Cancellation::default();
        let engine = engine(host, store, &committed);
        let cloud_id = state.cloud_id.clone();
        let mut operation = state.operation.clone();
        let mut saved = Saved {
            store,
            state,
            observe: super::super::mutation::IGNORE,
            pending: true,
            failed: None,
        };
        let deleted = docker_host::delete(&engine, host, &cloud_id, &mut operation, &mut journal, &mut saved);
        saved.finish(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(hosts: &serde_json::Value) -> Result<Settings> {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file": "/absent/key", "ssh_identity_file": "/absent/identity",
            "docker_config": "/absent/docker", "registry_pull_auth_id": null,
            "cpu_flavors": [], "gpu_types": [], "docker_hosts": hosts
        }))
        .map_err(|_| Error::Json)?;
        settings.validate().map(|()| settings)
    }

    #[test]
    fn a_cloud_uses_the_saved_host_its_placement_names() {
        let saved = settings(&serde_json::json!([
            {"name": "this-pc", "access": "this-pc"},
            {"name": "build-server", "access": "ssh", "address": "builder@build-server", "docker": "/usr/local/bin/docker"}
        ]))
        .unwrap();
        assert!(hosts(&saved).is_err(), "several hosts and no choice");
        // A reconnect resolves the host its cloud recorded, so preflight lets it through.
        let profile: horizon_cloud::Profile = serde_json::from_value(serde_json::json!({
            "provider": "docker-host", "image": "registry.example/worker", "cpu": 2, "memory_gb": 4
        }))
        .unwrap();
        preflight(&profile, &saved).unwrap();
        let mut chosen = saved.clone();
        chosen.placement = Some(crate::cloud_panel::Placement {
            data_centers: vec!["build-server".into()],
            ..Default::default()
        });
        assert_eq!(hosts(&chosen).unwrap(), ["build-server"]);
        chosen.docker_hosts.clear();
        assert!(hosts(&chosen).is_err());
        // Names are unique, and a host is checked when the settings load.
        let twice = serde_json::json!([{"name": "a", "access": "this-pc"}, {"name": "a", "access": "this-pc"}]);
        assert!(settings(&twice).is_err());
        assert!(settings(&serde_json::json!([{"name": "a", "access": "ssh", "address": "-oProxyCommand=x"}])).is_err());
        // Settings without hosts keep their encoding.
        let none = settings(&serde_json::json!([])).unwrap();
        assert!(serde_json::to_value(&none).unwrap().get("docker_hosts").is_none());
    }

    #[test]
    fn a_worker_on_a_remote_host_is_reached_through_it() {
        let saved = settings(&serde_json::json!([])).unwrap();
        let worker = |host: serde_json::Value| -> horizon_cloud::Worker {
            serde_json::from_value(serde_json::json!({
                "id": "worker1", "name": "horizon-cloud-one", "imageName": "fixture", "desiredStatus": "RUNNING",
                "publicIp": "127.0.0.1", "portMappings": {"22": 47001}, "dockerHost": host
            }))
            .unwrap()
        };
        let root = Path::new("/absent");
        let remote = worker(serde_json::json!({"name": "build-server", "access": "ssh", "address": "build-server"}));
        let args = Connection::new(&remote, &saved, root).unwrap().args();
        let proxy = args.iter().position(|arg| arg.starts_with("ProxyCommand=")).unwrap();
        assert_eq!(
            args[proxy],
            "ProxyCommand=ssh -o BatchMode=yes -o ConnectTimeout=10 -o ControlMaster=no -o ControlPath=none -W 127.0.0.1:%p -- 'build-server'"
        );
        assert_eq!(args[args.len() - 1], "root@127.0.0.1");
        assert_eq!(args[proxy - 1], "-o");
        let local = worker(serde_json::json!({"name": "this-pc", "access": "this-pc"}));
        let args = Connection::new(&local, &saved, root).unwrap().args();
        assert!(!args.iter().any(|arg| arg.starts_with("ProxyCommand=")));
    }
}
