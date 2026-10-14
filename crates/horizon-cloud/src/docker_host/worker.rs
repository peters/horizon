//! A cloud's container and workspace volume on a Docker host, without any storage of
//! its own: callers persist the [`Journal`] and the operation fence through [`Records`]
//! before each Docker request they guard. Only a container and a volume that carry the
//! cloud's owner label and its recorded names are ever changed.
use super::{Docker, Failure, Host, OWNER_LABEL};
use crate::{CloudError, CreateState, Reason, Worker, WorkerSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

/// Host loopback ports for worker SSH, below the ephemeral ranges of Linux
/// (32768-60999) and macOS (49152-65535), so the host's own outgoing connections do
/// not take them.
const PORTS: std::ops::Range<u16> = 31_000..32_000;
/// Containers a create may try when another program takes the chosen port first.
const PORT_ATTEMPTS: usize = 4;
const QUICK: Duration = Duration::from_secs(30);
/// Starting, and stopping, which waits 30 seconds for the worker to exit.
const POWER: Duration = Duration::from_secs(90);
const PULL: Duration = Duration::from_mins(30);
const NANO_CPUS: u64 = 1_000_000_000;
const GIB: u64 = 1 << 30;

/// What a cloud keeps about its place on a host, besides the operation fence.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    /// The saved host the cloud lives on, fixed by its first request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// How that host was reached then, so a saved host that now names another engine
    /// is refused rather than asked about this cloud's resources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<Route>,
    /// The engine's own ID from `docker info`, which a route cannot prove: an SSH alias
    /// or a name can come to point at another machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,
    /// The host loopback port of the worker's SSH, pinned for the container's life.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// The workspace volume may exist: set before it is created, cleared once a delete
    /// proved it gone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub volume: bool,
    /// Set when a delete starts; nothing starts the cloud again until it finished.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleting: bool,
}

/// The access and address of a saved host, which name its engine; its name and docker
/// path may change without moving the cloud.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub access: super::Access,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

impl Route {
    fn of(host: &Host) -> Self {
        Self {
            access: host.access,
            address: host.address.clone(),
        }
    }
}

impl Journal {
    /// Refuses `host` unless it is the saved host, reaching the engine, that the cloud
    /// recorded.
    /// # Errors
    /// Another saved host, or the same name pointing at another engine.
    pub fn admits(&self, host: &Host) -> Result<(), CloudError> {
        if self.host.as_ref().is_some_and(|recorded| *recorded != host.name) {
            return Err(CloudError::Invalid("This cloud lives on another Docker host"));
        }
        if self.engine.as_ref().is_some_and(|engine| *engine != Route::of(host)) {
            return Err(CloudError::Invalid(
                "This cloud's saved Docker host now reaches another engine; restore its access and address",
            ));
        }
        Ok(())
    }
}

impl Journal {
    /// Refuses an engine other than the one the cloud recorded, before its answers,
    /// such as a container that is not there, may count for this cloud.
    /// # Errors
    /// Another engine, or Docker failures.
    pub fn confirms(&self, docker: &impl Docker) -> Result<(), CloudError> {
        match &self.engine_id {
            Some(recorded) if *recorded != engine_id(docker)? => Err(CloudError::Invalid(
                "This cloud's saved Docker host now reaches another Docker engine than the one the cloud runs on",
            )),
            _ => Ok(()),
        }
    }
}

fn engine_id(docker: &impl Docker) -> Result<String, CloudError> {
    let id = docker.docker(&["info".into(), "--format".into(), "{{.ID}}".into()], QUICK)?;
    let id = id.trim();
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        return Err(CloudError::InvalidResponse);
    }
    Ok(id.to_owned())
}

/// Where a provisioning or delete step persists what it is about to do.
pub trait Records {
    /// # Errors
    /// The journal could not be saved.
    fn journal(&mut self, journal: &Journal) -> Result<(), CloudError>;
    /// # Errors
    /// The operation fence could not be saved.
    fn operation(&mut self, operation: &CreateState) -> Result<(), CloudError>;
    /// Called just before a request that changes a container whose record is already
    /// saved, such as starting a stopped worker.
    /// # Errors
    /// The pending change could not be recorded.
    fn mutation(&mut self) -> Result<(), CloudError>;
}

/// A container as `docker container inspect` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub owner: Option<String>,
    pub running: bool,
    /// Created, but never started.
    pub fresh: bool,
    /// The host port of the worker's SSH, when it is published on the host's loopback only.
    pub port: Option<u16>,
    pub image: String,
    pub nano_cpus: u64,
    pub memory: u64,
    /// The volume mounted as the workspace.
    pub workspace: Option<String>,
    /// The `HORIZON_` variables of its environment.
    pub env: BTreeMap<String, String>,
}

impl Container {
    fn parse(value: &Value) -> Option<Self> {
        let (config, host) = (&value["Config"], &value["HostConfig"]);
        let port = match host["PortBindings"]["22/tcp"].as_array().map(Vec::as_slice) {
            Some([binding]) if binding["HostIp"] == "127.0.0.1" => binding["HostPort"].as_str()?.parse().ok(),
            _ => None,
        };
        // An image's own volumes add anonymous mounts beside the workspace.
        let mounts = value["Mounts"].as_array().into_iter().flatten();
        let workspace = mounts
            .filter(|mount| mount["Type"] == "volume" && mount["Destination"] == "/workspace")
            .find_map(|mount| mount["Name"].as_str().map(str::to_owned));
        let env = config["Env"].as_array().into_iter().flatten();
        Some(Self {
            id: value["Id"].as_str()?.to_owned(),
            name: value["Name"].as_str()?.trim_start_matches('/').to_owned(),
            owner: config["Labels"][OWNER_LABEL].as_str().map(str::to_owned),
            running: value["State"]["Running"].as_bool()?,
            fresh: value["State"]["StartedAt"] == "0001-01-01T00:00:00Z",
            port,
            image: config["Image"].as_str()?.to_owned(),
            nano_cpus: host["NanoCpus"].as_u64().unwrap_or(0),
            memory: host["Memory"].as_u64().unwrap_or(0),
            workspace,
            env: env
                .filter_map(|entry| entry.as_str()?.split_once('='))
                .filter(|(key, _)| key.starts_with("HORIZON_"))
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect(),
        })
    }
}

fn container_name(operation_id: &str) -> String {
    format!("horizon-cloud-{operation_id}")
}

fn volume_name(operation_id: &str) -> String {
    format!("horizon-cloud-{operation_id}-workspace")
}

fn owner(operation_id: &str) -> String {
    format!("{OWNER_LABEL}={operation_id}")
}

/// Runs `command`, whose words are IDs, names, numbers and image references, which
/// hold no space.
fn run(docker: &impl Docker, command: &str, timeout: Duration) -> Result<String, Failure> {
    let args: Vec<String> = command.split(' ').map(str::to_owned).collect();
    docker.docker(&args, timeout)
}

fn call(docker: &impl Docker, command: &str, timeout: Duration) -> Result<String, CloudError> {
    Ok(run(docker, command, timeout)?)
}

/// The containers that carry the label `filter` names.
fn containers(docker: &impl Docker, filter: &str) -> Result<Vec<Container>, CloudError> {
    let list = format!("container ls --all --no-trunc --filter label={filter} --format {{{{.ID}}}}");
    let mut attempts = 0;
    let inspected = loop {
        let listed = call(docker, &list, QUICK)?;
        let ids: Vec<&str> = listed.split_whitespace().collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        match run(docker, &format!("container inspect {}", ids.join(" ")), QUICK) {
            // Another cloud's container was removed between the two calls.
            Err(Failure::Refused(error)) if attempts < 2 && error.contains("No such") => attempts += 1,
            result => break result?,
        }
    };
    let values: Vec<Value> = serde_json::from_str(&inspected).map_err(|_| CloudError::InvalidResponse)?;
    values
        .iter()
        .map(|value| Container::parse(value).ok_or(CloudError::InvalidResponse))
        .collect()
}

/// The cloud's own container: the one with its owner label and its name.
/// # Errors
/// Reports Docker failures and more than one such container.
pub fn find(docker: &impl Docker, operation_id: &str) -> Result<Option<Container>, CloudError> {
    // Docker commands are split on spaces, so the ID must be plain.
    if !crate::valid_id(operation_id) {
        return Err(CloudError::Invalid("Invalid cloud identity"));
    }
    let mut found = containers(docker, &owner(operation_id))?;
    found.retain(|container| container.name == container_name(operation_id));
    match found.len() {
        0 | 1 => Ok(found.pop()),
        _ => Err(CloudError::DuplicateWorkers),
    }
}

fn bound(docker: &impl Docker, operation_id: &str, worker_id: &str) -> Result<Container, CloudError> {
    match find(docker, operation_id)? {
        Some(container) if container.id == worker_id => Ok(container),
        Some(_) => Err(CloudError::IdentityMismatch),
        None => Err(CloudError::WorkerLost),
    }
}

/// Whether the cloud's own volume exists.
fn volume(docker: &impl Docker, operation_id: &str) -> Result<bool, CloudError> {
    let filter = owner(operation_id);
    let listed = call(
        docker,
        &format!("volume ls --filter label={filter} --format {{{{.Name}}}}"),
        QUICK,
    )?;
    Ok(listed.split_whitespace().any(|name| name == volume_name(operation_id)))
}

/// The worker `container` is, with the limits, workspace and loopback-only SSH it was
/// created with.
/// # Errors
/// Refuses a container that is not this cloud's worker as Horizon created it.
pub fn describe(container: &Container, host: &Host, spec: &WorkerSpec) -> Result<Worker, CloudError> {
    let id = &spec.operation_id;
    let port = container.port.ok_or(CloudError::IdentityMismatch)?;
    if container.owner.as_ref() != Some(id)
        || container.workspace != Some(volume_name(id))
        || container.nano_cpus != u64::from(spec.profile.cpu) * NANO_CPUS
        || container.memory != u64::from(spec.profile.memory_gb) * GIB
    {
        return Err(CloudError::IdentityMismatch);
    }
    let worker: Worker = serde_json::from_value(serde_json::json!({
        "id": container.id, "name": container.name, "imageName": container.image,
        "desiredStatus": if container.running { "RUNNING" } else { "EXITED" },
        "publicIp": "127.0.0.1", "portMappings": {"22": port},
        "vcpuCount": spec.profile.cpu, "memoryInGb": spec.profile.memory_gb, "volumeMountPath": "/workspace",
        "dataCenterId": host.name, "env": container.env, "dockerHost": host,
    }))
    .map_err(|_| CloudError::InvalidResponse)?;
    worker.verify(spec)?;
    Ok(worker)
}

/// Refuses a worker that the host's free CPU or memory cannot hold, after the other
/// Horizon workers that run there. Disk is not checked: a Docker volume has no size.
fn check_capacity(docker: &impl Docker, spec: &WorkerSpec) -> Result<(), CloudError> {
    let info = engine(docker)?;
    let (Some(cpus), Some(memory)) = (info["NCPU"].as_u64(), info["MemTotal"].as_u64()) else {
        return Err(CloudError::InvalidResponse);
    };
    let mut running = containers(docker, OWNER_LABEL)?;
    running.retain(|container| container.running);
    let free_cpus = (cpus * NANO_CPUS).saturating_sub(running.iter().map(|container| container.nano_cpus).sum());
    let free_memory = memory.saturating_sub(running.iter().map(|container| container.memory).sum());
    let (cpu, memory_gb) = (spec.profile.cpu, spec.profile.memory_gb);
    if u64::from(cpu) * NANO_CPUS > free_cpus || u64::from(memory_gb) * GIB > free_memory {
        let text = format!(
            "the host has {} CPUs and {} GB of memory free; this cloud needs {cpu} CPUs and {memory_gb} GB",
            tenths(free_cpus, NANO_CPUS),
            tenths(free_memory, GIB),
        );
        return Err(CloudError::Capacity(Reason::from_text(&text, "")));
    }
    Ok(())
}

/// The engine's `docker info`, once it runs Linux containers and keeps ports published
/// on the loopback private: Docker before stable 28 lets hosts on the same network
/// segment reach them.
fn engine(docker: &impl Docker) -> Result<Value, CloudError> {
    let info = docker.docker(&["info".into(), "--format".into(), "{{json .}}".into()], QUICK)?;
    let info: Value = serde_json::from_str(&info).map_err(|_| CloudError::InvalidResponse)?;
    if info["OSType"] != "linux" {
        return Err(CloudError::Invalid(
            "Workers need a Docker host that runs Linux containers",
        ));
    }
    if !info["ServerVersion"].as_str().is_some_and(private_ports) {
        return Err(CloudError::Invalid(
            "Use a stable Docker Engine 28 or later on the host, so the worker's SSH port stays private",
        ));
    }
    Ok(info)
}

/// Whether `version` is a stable Docker Engine 28 or later, such as `28.5.1`,
/// `29.0.0-ce` or a distribution build such as `28.1.2+dfsg1`.
fn private_ports(version: &str) -> bool {
    let version = version.split_once('+').map_or(version, |(version, _build)| version);
    let (release, suffix) = version.split_once('-').unwrap_or((version, ""));
    let parts: Vec<&str> = release.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        && parts[0].parse::<u32>().is_ok_and(|major| major >= 28)
        && matches!(suffix, "" | "ce" | "ee")
}

/// `value` in `unit`s with one decimal, rounded down.
fn tenths(value: u64, unit: u64) -> String {
    let tenths = value.saturating_mul(10) / unit;
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// A port that no Horizon worker on the host holds, from where the cloud's ID starts.
fn choose_port(docker: &impl Docker, operation_id: &str, avoid: &BTreeSet<u16>) -> Result<u16, CloudError> {
    let mut held: BTreeSet<u16> = containers(docker, OWNER_LABEL)?.iter().filter_map(|c| c.port).collect();
    held.extend(avoid);
    let (count, start) = (PORTS.end - PORTS.start, first_port(operation_id) - PORTS.start);
    (0..count)
        .map(|offset| PORTS.start + (start + offset) % count)
        .find(|port| !held.contains(port))
        .ok_or(CloudError::Capacity(Reason::from_text(
            "no SSH port is free on the host",
            "",
        )))
}

/// The port a cloud tries first, from an FNV-1a hash of its ID, so that clouds created
/// at the same time rarely try the same one.
pub(super) fn first_port(operation_id: &str) -> u16 {
    let hash = (operation_id.bytes()).fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    PORTS.start + u16::try_from(hash % u32::from(PORTS.end - PORTS.start)).unwrap_or_default()
}

fn port_taken(error: &str) -> bool {
    error.contains("port is already allocated") || error.contains("address already in use")
}

/// Changes `journal` only once the change is persisted, so a failed save leaves the
/// caller's copy as it was and a retry records it again.
fn record(
    journal: &mut Journal,
    records: &mut impl Records,
    change: impl FnOnce(&mut Journal),
) -> Result<(), CloudError> {
    let mut next = journal.clone();
    change(&mut next);
    records.journal(&next)?;
    *journal = next;
    Ok(())
}

fn save(operation: &mut CreateState, next: CreateState, records: &mut impl Records) -> Result<(), CloudError> {
    records.operation(&next)?;
    *operation = next;
    Ok(())
}

/// Creates the cloud's worker, or finds the one it has, starts it when it is stopped
/// and describes it. The host, the workspace volume and the pinned port are recorded
/// before they are used, and the operation is `Requested` before the container is
/// created; the container's name fences a second one.
/// # Errors
/// Refuses another host than the recorded one, missing capacity, a deleting cloud and
/// any container or volume that is not the cloud's own; reports Docker and persistence
/// failures.
pub fn provision(
    docker: &impl Docker,
    host: &Host,
    spec: &WorkerSpec,
    operation: &mut CreateState,
    journal: &mut Journal,
    records: &mut impl Records,
) -> Result<Worker, CloudError> {
    if journal.deleting {
        return Err(CloudError::Invalid("Finish deleting this cloud before it starts again"));
    }
    if spec.profile.gpu || spec.startup_metadata.is_some() {
        return Err(CloudError::Invalid("A Docker host runs dedicated CPU workers only"));
    }
    // As `WorkerSpec::validate`, whose profile rules do not know this provider yet.
    // Docker commands are split on spaces, so their words must be plain too.
    if !crate::valid_id(&spec.operation_id)
        || !crate::valid_image(&spec.image_digest)
        || !spec.image_digest.contains("@sha256:")
        || !crate::valid_public_key(&spec.public_key)
        // Docker reads a zero limit as no limit at all.
        || spec.profile.cpu == 0
        || spec.profile.memory_gb == 0
    {
        return Err(CloudError::Invalid(
            "Worker requires a stable operation ID, an immutable image digest, an Ed25519 public key, CPU and memory",
        ));
    }
    let id = spec.operation_id.as_str();
    journal.admits(host)?;
    journal.confirms(docker)?;
    if *operation == CreateState::Prepared {
        check_capacity(docker, spec)?;
        // No platform is named: Docker takes the host's own from a multi-platform image, and
        // runs an amd64-only image under emulation on an ARM host.
        let pull = format!("pull --quiet {}", spec.image_digest);
        call(docker, &pull, PULL)?;
    }
    // Recorded once nothing refused the cloud, before anything is created for it.
    if journal.host.is_none() || journal.engine.is_none() || journal.engine_id.is_none() {
        let engine = engine_id(docker)?;
        record(journal, records, |next| {
            next.host = Some(host.name.clone());
            next.engine = Some(Route::of(host));
            next.engine_id = Some(engine);
        })?;
    }
    if *operation == CreateState::Prepared {
        if !volume(docker, id)? {
            if !journal.volume {
                record(journal, records, |next| next.volume = true)?;
            }
            call(
                docker,
                &format!("volume create --label {} {}", owner(id), volume_name(id)),
                QUICK,
            )?;
            // Docker returns a volume of the same name unchanged, whoever made it.
            if !volume(docker, id)? {
                return Err(CloudError::Invalid(
                    "The host has a volume of this cloud's name that Horizon did not create; it was left alone",
                ));
            }
        }
        save(operation, CreateState::Requested, records)?;
    }
    if *operation == CreateState::Requested {
        bind(docker, spec, operation, journal, records)?;
    }
    let CreateState::Bound { worker_id } = operation.clone() else {
        return Err(CloudError::WorkerLost);
    };
    power(docker, id, &worker_id, true, &mut || records.mutation())?;
    describe(&bound(docker, id, &worker_id)?, host, spec)
}

/// Creates and starts the requested container, or adopts the one an earlier attempt
/// created, and binds the operation to it. A container that never started because
/// another program took its port is removed and created again on another port.
fn bind(
    docker: &impl Docker,
    spec: &WorkerSpec,
    operation: &mut CreateState,
    journal: &mut Journal,
    records: &mut impl Records,
) -> Result<(), CloudError> {
    let id = spec.operation_id.as_str();
    // Also for a request an earlier attempt recorded: the engine may have changed since.
    engine(docker)?;
    let mut avoid = BTreeSet::new();
    for _ in 0..PORT_ATTEMPTS {
        let container = if let Some(container) = find(docker, id)? {
            container
        } else {
            let port = if let Some(port) = journal.port.filter(|port| !avoid.contains(port)) {
                port
            } else {
                let port = choose_port(docker, id, &avoid)?;
                record(journal, records, |next| next.port = Some(port))?;
                port
            };
            // A named mount would silently make a new, unlabelled volume if the
            // cloud's own one were gone, and delete would then leave it behind.
            if !volume(docker, id)? {
                return Err(CloudError::Invalid(
                    "This cloud's workspace volume is gone from the host; delete the cloud and create it again",
                ));
            }
            create(docker, spec, port)?;
            find(docker, id)?.ok_or(CloudError::InvalidResponse)?
        };
        if container.port.is_none() || container.port != journal.port || container.image != spec.image_digest {
            return Err(CloudError::IdentityMismatch);
        }
        if !container.running {
            match run(docker, &format!("container start {}", container.id), POWER) {
                Err(Failure::Refused(error)) if container.fresh && port_taken(&error) => {
                    avoid.extend(container.port);
                    call(
                        docker,
                        &format!("container rm --force --volumes {}", container.id),
                        QUICK,
                    )?;
                    continue;
                }
                result => result?,
            };
        }
        return save(
            operation,
            CreateState::Bound {
                worker_id: container.id,
            },
            records,
        );
    }
    Err(CloudError::Capacity(Reason::from_text(
        "other programs hold every SSH port tried",
        "",
    )))
}

pub(super) fn create(docker: &impl Docker, spec: &WorkerSpec, port: u16) -> Result<(), CloudError> {
    let (id, cpu, memory_gb) = (&spec.operation_id, spec.profile.cpu, spec.profile.memory_gb);
    let shm_gb = crate::hetzner::cloud::shared_memory_gb(memory_gb);
    let command = format!(
        "container create --name {} --label {} --cpus {cpu} --memory {memory_gb}g \
         --shm-size {shm_gb}g --publish 127.0.0.1:{port}:22 --mount type=volume,src={},dst=/workspace",
        container_name(id),
        owner(id),
        volume_name(id),
    );
    let mut args: Vec<String> = command.split(' ').map(str::to_owned).collect();
    for (key, value) in spec.container_environment()? {
        args.extend(["--env".to_owned(), format!("{key}={value}")]);
    }
    args.push(spec.image_digest.clone());
    docker.docker(&args, QUICK)?;
    Ok(())
}

/// Starts (`on`) or stops the bound worker unless it already is so, calling `announce`
/// just before the request. A start uses the worker's pinned port.
/// # Errors
/// Refuses a container that is not the bound one and a port another program now
/// holds; reports Docker failures.
pub fn power(
    docker: &impl Docker,
    operation_id: &str,
    worker_id: &str,
    on: bool,
    announce: &mut dyn FnMut() -> Result<(), CloudError>,
) -> Result<(), CloudError> {
    let container = bound(docker, operation_id, worker_id)?;
    if container.running == on {
        return Ok(());
    }
    if on {
        engine(docker)?;
    }
    announce()?;
    let command = if on { "start" } else { "stop --time 30" };
    match run(docker, &format!("container {command} {}", container.id), POWER) {
        Err(Failure::Refused(error)) if port_taken(&error) => Err(CloudError::Rejected(Reason::from_text(
            "another program on the host now holds this worker's SSH port; free it and resume again",
            "",
        ))),
        result => result.map(drop).map_err(Into::into),
    }
}

/// Removes the cloud's container and workspace volume, each proven gone, and records
/// the operation as terminated. Containers and volumes without the cloud's owner
/// label and recorded name are never touched; the container's own anonymous volumes go
/// with it.
/// # Errors
/// Reports Docker and persistence failures, leaving records a retry resumes from.
pub fn delete(
    docker: &impl Docker,
    host: &Host,
    operation_id: &str,
    operation: &mut CreateState,
    journal: &mut Journal,
    records: &mut impl Records,
) -> Result<(), CloudError> {
    // Another host's engine would find nothing and report the cloud deleted.
    journal.admits(host)?;
    journal.confirms(docker)?;
    if !crate::valid_id(operation_id) {
        return Err(CloudError::Invalid("Invalid cloud identity"));
    }
    if !journal.deleting {
        record(journal, records, |next| next.deleting = true)?;
    }
    if let Some(container) = find(docker, operation_id)? {
        call(
            docker,
            &format!("container rm --force --volumes {}", container.id),
            POWER,
        )?;
    }
    if volume(docker, operation_id)? {
        call(docker, &format!("volume rm {}", volume_name(operation_id)), QUICK)?;
    }
    if find(docker, operation_id)?.is_some() || volume(docker, operation_id)? {
        return Err(CloudError::Invalid(
            "The Docker host did not remove the worker and its volume",
        ));
    }
    let worker_id = match operation {
        CreateState::Bound { worker_id } | CreateState::Terminated { worker_id } => worker_id.clone(),
        CreateState::Prepared | CreateState::Requested => container_name(operation_id),
    };
    save(operation, CreateState::Terminated { worker_id }, records)?;
    record(journal, records, |next| *next = Journal::default())
}
