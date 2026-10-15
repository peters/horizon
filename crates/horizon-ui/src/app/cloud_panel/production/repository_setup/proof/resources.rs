//! The names of one setup run's proof image and volume, and their removal.
use std::path::Path;
use std::process::Command;

/// The image and volume of one setup run's proofs: unique to the run, so two runs or
/// two Horizon processes on the same checkout never share or remove each other's.
pub(in crate::app::cloud_panel::production::repository_setup) fn run_names(
    repository: &Path,
    run: &str,
) -> (String, String) {
    let volume = format!("horizon-setup-{}-{run}", slug(repository));
    (format!("{volume}:candidate"), volume)
}

/// A short name for the setup run whose status file is `status`.
pub(in crate::app::cloud_panel::production::repository_setup) fn run_id(status: &Path) -> String {
    let folder = status.parent().unwrap_or(status).to_string_lossy();
    let hash = folder.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{:08x}", hash & 0xffff_ffff)
}

/// The start of the worker-check operation names of the run whose volume is `volume`;
/// each check adds its profile's number.
pub(super) fn check_operations(volume: &str) -> String {
    let hash = volume.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("horizon-setup-check-{:08x}", hash & 0xffff_ffff)
}

/// The Docker daemon a proof uses, pinned when the proof starts: every docker command of
/// the proof and of its cleanup goes to that daemon, so a Docker context switched
/// meanwhile never sends one to another machine. It is this computer's, or the engine of
/// a Docker host saved in Cloud settings, forwarded over SSH while this value lives.
#[derive(Clone, Debug, Default)]
pub(in crate::app::cloud_panel::production::repository_setup) struct Daemon {
    /// The daemon's address; none only before a proof's run has checked it, when the run
    /// sends no docker command.
    host: Option<String>,
    /// The Docker host whose engine `host` forwards, with the session that keeps it.
    remote: Option<std::sync::Arc<super::remote::Remote>>,
}

/// A proof's pinned daemon, shared by the proof's run, its Stop and its cleanup; empty
/// until the proof's run has checked it.
pub(in crate::app::cloud_panel::production::repository_setup) type Pinned = std::sync::Arc<std::sync::OnceLock<Daemon>>;

impl Daemon {
    /// The daemon the Docker CLI uses now, accepted only through a socket or pipe of
    /// this computer: a proof runs on this computer's Docker unless the person chose
    /// another. The proof copies the files it needs into its containers and mounts none
    /// of this computer's, so it would run the same elsewhere. A socket needs no TLS
    /// settings, so its address alone pins it.
    pub(in crate::app::cloud_panel::production::repository_setup) fn resolve() -> Result<Self, String> {
        Self::accept(horizon_core::cloud_runtime::docker_daemon::Target::default().daemon())
    }

    /// The daemon of `host`, a Docker host saved in Cloud settings, or this computer's for
    /// none or for a host that is this computer.
    pub(in crate::app::cloud_panel::production::repository_setup) fn on(
        host: Option<&horizon_core::cloud_runtime::docker_host::Host>,
    ) -> Result<Self, String> {
        match host.filter(|host| host.access != horizon_core::cloud_runtime::docker_host::Access::ThisPc) {
            None => Self::resolve(),
            Some(host) => {
                let remote = super::remote::Remote::open(host)?;
                Ok(Self {
                    host: Some(remote.address()),
                    remote: Some(std::sync::Arc::new(remote)),
                })
            }
        }
    }

    /// The Docker host it is on, when that is not this computer.
    pub(in crate::app::cloud_panel::production::repository_setup) fn remote(&self) -> Option<&super::remote::Remote> {
        self.remote.as_deref()
    }

    /// What its build slots are kept by: the Docker host's name, else this computer's
    /// daemon address.
    pub(in crate::app::cloud_panel::production::repository_setup) fn slot_key(&self) -> String {
        self.remote().map_or_else(
            || self.host.clone().unwrap_or_default(),
            |remote| format!("docker-host:{}", remote.name()),
        )
    }

    /// The daemon at `endpoint`, as [`Self::resolve`] accepts it.
    fn accept(endpoint: Option<horizon_core::cloud_runtime::docker_daemon::Endpoint>) -> Result<Self, String> {
        use horizon_core::cloud_runtime::docker_daemon::Endpoint;
        match endpoint {
            None => Err(
                "Horizon cannot tell which Docker daemon Docker uses here (docker context inspect names none), so it runs no proof: check that Docker is installed and its context works, then start the setup again."
                    .to_owned(),
            ),
            Some(endpoint @ (Endpoint::Socket(_) | Endpoint::Pipe(_))) => Ok(Self {
                host: Some(endpoint.address()),
                remote: None,
            }),
            Some(endpoint) => Err(format!(
                "Docker here uses the daemon at {}, not a socket of this computer. A proof on this computer runs on Docker through a socket of this computer: switch to a local Docker context (docker context use default) or unset DOCKER_HOST, then start the setup again.",
                endpoint.address()
            )),
        }
    }

    /// The daemon's address, for a Docker command line's `--host`.
    pub(in crate::app::cloud_panel::production::repository_setup) fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    /// A docker command that talks to this daemon only.
    pub(in crate::app::cloud_panel::production::repository_setup) fn command(&self) -> Command {
        let mut command = Command::new("docker");
        self.pin(&mut command);
        command
    }

    /// Points `command`, a docker command, at this daemon only.
    pub(in crate::app::cloud_panel::production::repository_setup) fn pin(&self, command: &mut Command) {
        if let Some(host) = &self.host {
            command.env("DOCKER_HOST", host).env_remove("DOCKER_CONTEXT");
        }
    }
}

/// The most one cleanup command may take before Horizon gives up on it and goes on with
/// the next.
const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// The most a whole cleanup may take, all its commands together, so a daemon that does
/// not answer holds an exit or a new setup for this long at most.
const CLEANUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Where a cleanup's docker commands go and when it gives up on the rest of them.
struct Cleanup<'a> {
    daemon: &'a Daemon,
    deadline: std::time::Instant,
}

/// Runs a docker cleanup command; its standard output when it succeeded in time. One
/// that runs past [`COMMAND_TIMEOUT`] or the cleanup's deadline is ended with everything
/// it started, and none starts once the deadline has passed.
fn docker(cleanup: &Cleanup<'_>, args: &[String]) -> Option<String> {
    use std::io::Read as _;
    let now = std::time::Instant::now();
    if now >= cleanup.deadline {
        return None;
    }
    let deadline = cleanup.deadline.min(now + COMMAND_TIMEOUT);
    let mut command = cleanup.daemon.command();
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut output = String::new();
        if let Some(stdout) = stdout {
            let _ = stdout.take(1024 * 1024).read_to_string(&mut output);
        }
        output
    });
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return reader.join().ok().filter(|_| status.success()),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            _ => {
                #[cfg(unix)]
                if let Some(group) = i32::try_from(child.id()).ok().and_then(rustix::process::Pid::from_raw) {
                    let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// `words` as the arguments of a docker command.
fn arguments<'a>(words: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    words.into_iter().map(str::to_owned).collect()
}

/// Runs one docker command against `daemon` within [`COMMAND_TIMEOUT`], ended with
/// what it started past that; its standard output when it succeeded in time. For a
/// removal that must also run after a proof was stopped.
pub(in crate::app::cloud_panel::production::repository_setup) fn remove_now(
    daemon: &Daemon,
    args: &[&str],
) -> Option<String> {
    let cleanup = Cleanup {
        daemon,
        deadline: std::time::Instant::now() + COMMAND_TIMEOUT,
    };
    docker(&cleanup, &arguments(args.iter().copied()))
}

/// The containers whose names match `pattern`, a Docker name filter.
fn containers(cleanup: &Cleanup<'_>, pattern: &str) -> Vec<String> {
    docker(
        cleanup,
        &arguments(["ps", "--all", "--quiet", "--filter", &format!("name={pattern}")]),
    )
    .map(|output| output.split_whitespace().map(str::to_owned).collect())
    .unwrap_or_default()
}

/// Removes a finished run's containers, image and volume once `after`, the run's
/// stopped proof and agent, has ended and can create nothing more: in the background, or
/// before returning with `wait`, as when Horizon exits. A run without a proof still has
/// them removed, such as the agent's preview container. The commands go to the daemon
/// `pinned` names, each has a time limit and all of them together one more.
pub(in crate::app::cloud_panel::production::repository_setup) fn remove_run_resources(
    repository: &Path,
    run: &str,
    after: Option<std::thread::JoinHandle<()>>,
    (pinned, build_on): (Option<Pinned>, Option<horizon_core::cloud_runtime::docker_host::Host>),
    wait: bool,
) {
    // A pinned daemon says where they are: on a Docker host by its name and engine, or on
    // this computer by its socket, with no host name. Without one, as when a replacement
    // proof failed before it reached a daemon, the run's own Docker host on another
    // machine does.
    let build_on = build_on.filter(|host| host.access != horizon_core::cloud_runtime::docker_host::Access::ThisPc);
    let known = pinned.as_ref().and_then(|pinned| pinned.get());
    let on_host = known.and_then(Daemon::remote);
    let docker_host = docker_host_of(known, build_on.as_ref());
    let engine_id = on_host.map(|remote| remote.engine_id().to_owned());
    expect_removal(repository, run, (docker_host.as_deref(), engine_id.as_deref()));
    let (image, volume) = run_names(repository, run);
    let resources = RunResources::new(image, volume);
    let remove = move || {
        if let Some(after) = after {
            let _ = after.join();
        }
        // The daemon the proof pinned; without a proof that checked one, the run's Docker
        // host, else the daemon of this computer Docker uses now. Resources of a run on a
        // Docker host are never looked for on this computer, where they would look gone.
        let pinned = pinned.as_ref().and_then(|pinned| pinned.get().cloned());
        let daemon = pinned.or_else(|| match &build_on {
            Some(host) => reopen_host(host, None),
            None => cleanup_daemon(),
        });
        let Some(daemon) = daemon else {
            resources.set_waiting(None);
            return;
        };
        resources.remove_or_keep(daemon);
    };
    if wait {
        remove();
    } else {
        // It waits for the run's agent to end, so a new setup of the repository waits for it.
        super::super::run::keep_ending_of(repository, std::thread::spawn(remove));
    }
}

/// What a run made with Docker, by name: its proof and check containers, its images and
/// its volume.
#[derive(Clone, Debug)]
struct RunResources {
    image: String,
    volume: String,
    /// The name filter of its worker-check containers.
    checks: String,
}

/// A run's resources that are being removed or wait for another try, with the daemon
/// they are on once a proof or cleanup knew it.
struct Pending {
    resources: RunResources,
    daemon: Option<Daemon>,
    /// A thread is removing them now.
    working: bool,
    /// The Docker host they are on, by name, as an earlier Horizon kept them: its forward
    /// opens again for the retry.
    docker_host: Option<String>,
    /// That host's engine ID when they were made, so a retry never removes on another
    /// engine the same name reaches later.
    engine_id: Option<String>,
}

/// Every run whose resources are not shown gone yet: from the moment the run is let go
/// until a cleanup showed them gone. A new setup and Horizon's exit try those not being
/// worked on again, and what is still here when Horizon exits is kept in
/// [`leftovers_file`] for its next run.
static PENDING: std::sync::Mutex<Vec<Pending>> = std::sync::Mutex::new(Vec::new());

fn pending() -> std::sync::MutexGuard<'static, Vec<Pending>> {
    PENDING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The Docker host a run's resources are on, by name: the pinned daemon's when there is
/// one, none for this computer's; else the run's own Docker host on another machine.
pub(in crate::app::cloud_panel::production::repository_setup) fn docker_host_of(
    pinned: Option<&Daemon>,
    build_on: Option<&horizon_core::cloud_runtime::docker_host::Host>,
) -> Option<String> {
    match pinned {
        Some(daemon) => daemon.remote().map(|remote| remote.name().to_owned()),
        None => build_on
            .filter(|host| host.access != horizon_core::cloud_runtime::docker_host::Access::ThisPc)
            .map(|host| host.name.clone()),
    }
}

/// Notes that the resources of `run` in `repository` are to be removed, before any thread
/// works on them, so an exit that comes first keeps them for the next run, on
/// `docker_host` when the run built on one.
pub(in crate::app::cloud_panel::production::repository_setup) fn expect_removal(
    repository: &Path,
    run: &str,
    (docker_host, engine_id): (Option<&str>, Option<&str>),
) {
    let (image, volume) = run_names(repository, run);
    let mut pending = pending();
    match pending.iter_mut().find(|known| known.resources.volume == volume) {
        Some(known) => {
            if known.docker_host.is_none() {
                known.docker_host = docker_host.map(str::to_owned);
            }
            if known.engine_id.is_none() {
                known.engine_id = engine_id.map(str::to_owned);
            }
        }
        None => pending.push(Pending {
            resources: RunResources::new(image, volume),
            daemon: None,
            working: true,
            docker_host: docker_host.map(str::to_owned),
            engine_id: engine_id.map(str::to_owned),
        }),
    }
}

/// The daemon of this computer that a cleanup without a pinned daemon goes to.
#[cfg(not(test))]
fn cleanup_daemon() -> Option<Daemon> {
    Daemon::resolve().ok()
}

/// A test never sends a cleanup to the Docker daemon of the computer it runs on. A daemon
/// that does not answer would hold each dropped run for the whole cleanup timeout, and
/// each test app's exit for its wait, which made one Windows CI run time out.
#[cfg(test)]
fn cleanup_daemon() -> Option<Daemon> {
    None
}

/// The daemon of the Docker host `name` saved in Cloud settings, forwarded again for a
/// retry; none when the host is no longer saved or cannot be reached now.
#[cfg(not(test))]
fn reopen(name: &str, engine_id: Option<&str>) -> Option<Daemon> {
    let path = horizon_core::HorizonHome::resolve()
        .root()
        .join("cloud")
        .join("settings.json");
    let settings = horizon_core::cloud_runtime::settings::Settings::load(&path).ok()?;
    let host = settings.docker_hosts.iter().find(|host| host.name == name)?;
    reopen_host(host, engine_id)
}

/// A test never reaches a Docker host.
#[cfg(test)]
fn reopen(_name: &str, _engine_id: Option<&str>) -> Option<Daemon> {
    None
}

/// The daemon of `host`, forwarded again for a retry, when it is still the engine
/// `engine_id` names; none when it cannot be reached or is another engine now.
fn reopen_host(host: &horizon_core::cloud_runtime::docker_host::Host, engine_id: Option<&str>) -> Option<Daemon> {
    let daemon = Daemon::on(Some(host))
        .map_err(|error| {
            tracing::warn!(
                host = host.name,
                "cannot reach the Docker host to remove a setup run's resources: {error}"
            );
        })
        .ok()?;
    let found = daemon.remote().map(super::remote::Remote::engine_id);
    if engine_id.is_some_and(|expected| found != Some(expected)) {
        tracing::warn!(
            host = host.name,
            "the Docker host is another engine now; a setup run's resources stay recorded for the one they are on"
        );
        return None;
    }
    Some(daemon)
}

/// Where Horizon keeps the run resources it could not remove before it exited.
#[cfg(not(test))]
fn leftovers_file() -> std::path::PathBuf {
    horizon_core::HorizonHome::resolve()
        .root()
        .join("cloud")
        .join("setup-leftovers.json")
}

/// A test keeps its leftovers to itself, never in the person's Horizon home.
#[cfg(test)]
fn leftovers_file() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("horizon-test-setup-leftovers-{}.json", std::process::id()))
}

/// Resources waiting for another try, with the daemon, the Docker host and the engine
/// they are on.
type Waiting = (RunResources, Option<Daemon>, Option<String>, Option<String>);

/// Tries again to remove what earlier cleanups could not, this run of Horizon's or one
/// before it, in the background; Horizon's exit waits for it.
pub(in crate::app::cloud_panel::production::repository_setup) fn retry_unremoved() {
    let file = leftovers_file();
    if let Ok(bytes) = super::super::untrusted::read_regular(&file, 1024 * 1024) {
        let _ = std::fs::remove_file(&file);
        let kept: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap_or_default();
        let mut pending = pending();
        for entry in kept {
            if let (Some(image), Some(volume)) = (entry["image"].as_str(), entry["volume"].as_str())
                && !pending.iter().any(|known| known.resources.volume == volume)
            {
                let docker_host = entry["docker_host"].as_str().map(str::to_owned);
                let engine_id = entry["engine_id"].as_str().map(str::to_owned);
                pending.push(Pending {
                    resources: RunResources::new(image.to_owned(), volume.to_owned()),
                    // A forwarded socket is gone with the Horizon that opened it.
                    daemon: entry["host"]
                        .as_str()
                        .filter(|_| docker_host.is_none())
                        .map(|host| Daemon {
                            host: Some(host.to_owned()),
                            remote: None,
                        }),
                    working: false,
                    docker_host,
                    engine_id,
                });
            }
        }
    }
    let waiting: Vec<Waiting> = pending()
        .iter_mut()
        .filter(|known| !known.working)
        .map(|known| {
            known.working = true;
            let route = (known.docker_host.clone(), known.engine_id.clone());
            (known.resources.clone(), known.daemon.clone(), route.0, route.1)
        })
        .collect();
    for (resources, daemon, docker_host, engine_id) in waiting {
        super::super::run::keep_ending(std::thread::spawn(move || {
            // Resources on a Docker host are never looked for on this computer's daemon,
            // where they are missing and would look removed.
            // A forward that ended forwards nothing again: the host's name opens a new one.
            let daemon = daemon.filter(|daemon| daemon.remote().is_none_or(super::remote::Remote::alive));
            let daemon = match (daemon, docker_host) {
                (Some(daemon), _) => Some(daemon),
                (None, Some(name)) => reopen(&name, engine_id.as_deref()),
                (None, None) => cleanup_daemon(),
            };
            match daemon {
                Some(daemon) => resources.remove_or_keep(daemon),
                None => resources.set_waiting(None),
            }
        }));
    }
}

/// Keeps every run whose resources are not shown gone, as Horizon exits, for its next
/// run to remove: those that failed and those a thread still works on.
pub(in crate::app::cloud_panel::production::repository_setup) fn keep_unremoved() {
    let kept: Vec<serde_json::Value> = pending()
        .iter()
        .map(|known| {
            serde_json::json!({
                "image": known.resources.image,
                "volume": known.resources.volume,
                "host": known.daemon.as_ref().and_then(|daemon| daemon.host.clone()),
                "docker_host": known
                    .daemon
                    .as_ref()
                    .and_then(Daemon::remote)
                    .map(super::remote::Remote::name)
                    .or(known.docker_host.as_deref()),
                "engine_id": known
                    .daemon
                    .as_ref()
                    .and_then(Daemon::remote)
                    .map(super::remote::Remote::engine_id)
                    .or(known.engine_id.as_deref()),
            })
        })
        .collect();
    if kept.is_empty() {
        return;
    }
    let file = leftovers_file();
    let written = file.parent().is_some_and(|folder| {
        std::fs::create_dir_all(folder).is_ok()
            && tempfile::NamedTempFile::new_in(folder).is_ok_and(|mut partial| {
                std::io::Write::write_all(&mut partial, &serde_json::to_vec(&kept).unwrap_or_default()).is_ok()
                    && partial.persist(&file).is_ok()
            })
    });
    if !written {
        tracing::warn!(file = %file.display(), "could not keep the setup runs' leftover Docker resources for the next start");
    }
}

impl RunResources {
    fn new(image: String, volume: String) -> Self {
        let checks = format!("^/horizon-contract-{}-", check_operations(&volume));
        Self { image, volume, checks }
    }

    /// Removes them; what a cleanup cannot show gone is logged and kept for a retry.
    fn remove_or_keep(self, daemon: Daemon) {
        {
            let mut pending = pending();
            let docker_host = daemon.remote().map(|remote| remote.name().to_owned());
            let engine_id = daemon.remote().map(|remote| remote.engine_id().to_owned());
            match pending.iter_mut().find(|known| known.resources.volume == self.volume) {
                Some(known) => {
                    known.daemon = Some(daemon.clone());
                    known.working = true;
                    known.docker_host = docker_host.or_else(|| known.docker_host.take());
                    known.engine_id = engine_id.or_else(|| known.engine_id.take());
                }
                None => pending.push(Pending {
                    resources: self.clone(),
                    daemon: Some(daemon.clone()),
                    working: true,
                    docker_host,
                    engine_id,
                }),
            }
        }
        if self.remove(&daemon) {
            pending().retain(|known| known.resources.volume != self.volume);
        } else {
            tracing::warn!(
                image = %self.image,
                volume = %self.volume,
                "a setup run's Docker resources are not removed yet; the next setup or exit tries again"
            );
            self.set_waiting(Some(daemon));
        }
    }

    /// Marks them as waiting for another try, on `daemon` when it is known.
    fn set_waiting(&self, daemon: Option<Daemon>) {
        if let Some(known) = pending().iter_mut().find(|known| known.resources.volume == self.volume) {
            known.working = false;
            known.daemon = daemon.or_else(|| known.daemon.take());
        }
    }

    /// Removes them, again and again until a listing shows none left, within the
    /// cleanup's deadline; whether one did. A check stopped mid-way can still have its
    /// container appear a moment later, so a second pass always follows a pause.
    fn remove(&self, daemon: &Daemon) -> bool {
        // A Docker host's socket can lead to another engine since the forward opened;
        // there these resources would look gone, so they are kept for a retry.
        if daemon.remote().is_some_and(|remote| !remote.same_engine()) {
            tracing::warn!(
                volume = %self.volume,
                "the Docker host answers as another engine now; a setup run's resources stay recorded"
            );
            return false;
        }
        let cleanup = &Cleanup {
            daemon,
            deadline: std::time::Instant::now() + CLEANUP_TIMEOUT,
        };
        let mut pass = 0;
        loop {
            if pass > 0 {
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            self.remove_once(cleanup);
            if pass > 0 && self.left(cleanup).is_some_and(|left| left.is_empty()) {
                return true;
            }
            if std::time::Instant::now() >= cleanup.deadline {
                return false;
            }
            pass += 1;
        }
    }

    fn remove_once(&self, cleanup: &Cleanup<'_>) {
        let found: Vec<String> = containers(cleanup, &format!("^/{}-", self.volume))
            .into_iter()
            .chain(containers(cleanup, &self.checks))
            .collect();
        if !found.is_empty() {
            let mut remove = arguments(["rm", "--force", "--volumes"]);
            remove.extend(found);
            let _ = docker(cleanup, &remove);
        }
        let filter = format!("reference={}-build*", self.volume);
        let images: Vec<String> = docker(
            cleanup,
            &arguments(["images", "--format", "{{.Repository}}:{{.Tag}}", "--filter", &filter]),
        )
        .map(|output| output.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
        let mut remove_images = arguments(["rmi", "--force", &self.image]);
        remove_images.extend(images);
        let _ = docker(cleanup, &remove_images);
        let _ = docker(cleanup, &arguments(["volume", "rm", "--force", &self.volume]));
        if self.network_left(cleanup).is_some_and(|left| left) {
            let _ = docker(cleanup, &arguments(["network", "rm", &preview_network(&self.volume)]));
        }
    }

    /// Whether the run's preview network is still there; none when the listing failed.
    fn network_left(&self, cleanup: &Cleanup<'_>) -> Option<bool> {
        let network = preview_network(&self.volume);
        docker(
            cleanup,
            &arguments([
                "network",
                "ls",
                "--format",
                "{{.Name}}",
                "--filter",
                &format!("name={network}"),
            ]),
        )
        .map(|output| output.lines().any(|name| name == network))
    }

    /// What is left of them, by Docker's own listings; none when a listing failed, so a
    /// daemon that does not answer is never taken for one that has nothing left.
    fn left(&self, cleanup: &Cleanup<'_>) -> Option<Vec<String>> {
        let list = |args: Vec<String>| {
            docker(cleanup, &args).map(|output| output.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
        };
        let mut left = list(arguments([
            "ps",
            "--all",
            "--quiet",
            "--filter",
            &format!("name=^/{}-", self.volume),
        ]))?;
        left.extend(list(arguments([
            "ps",
            "--all",
            "--quiet",
            "--filter",
            &format!("name={}", self.checks),
        ]))?);
        left.extend(list(arguments([
            "images",
            "--quiet",
            "--filter",
            &format!("reference={}", self.image),
        ]))?);
        left.extend(list(arguments([
            "images",
            "--quiet",
            "--filter",
            &format!("reference={}-build*", self.volume),
        ]))?);
        let volumes = list(arguments([
            "volume",
            "ls",
            "--quiet",
            "--filter",
            &format!("name={}", self.volume),
        ]))?;
        left.extend(volumes.into_iter().filter(|name| *name == self.volume));
        if self.network_left(cleanup)? {
            left.push(preview_network(&self.volume));
        }
        Some(left)
    }
}

/// The bridge network of a run's desktop previews, only theirs: a container on Docker's
/// default bridge cannot reach a preview's desktop. The run's cleanup removes it.
pub(in crate::app::cloud_panel::production::repository_setup) fn preview_network(volume: &str) -> String {
    format!("{volume}-preview")
}

/// A name Docker accepts for the repository: its folder name and a short hash of its
/// path, so two checkouts with the same folder name never share an image or a volume.
fn slug(repository: &Path) -> String {
    // The path's own bytes, so two paths that only differ in bytes that are not UTF-8
    // never share a name.
    #[cfg(unix)]
    let bytes = std::os::unix::ffi::OsStrExt::as_bytes(repository.as_os_str()).to_vec();
    #[cfg(not(unix))]
    let bytes = repository.to_string_lossy().into_owned().into_bytes();
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{}-{:06x}", folder_slug(repository), hash & 0xff_ffff)
}

/// The longest part of a name the folder gives; the path's hash tells folders apart,
/// and Docker names stay well within their limits.
const FOLDER_SLUG_LIMIT: usize = 32;

fn folder_slug(repository: &Path) -> String {
    let name = repository
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let slug: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug: String = slug.trim_matches('-').chars().take(FOLDER_SLUG_LIMIT).collect();
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "repository".to_owned()
    } else {
        slug.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pinned_daemon_says_where_a_runs_resources_are_and_this_computer_has_no_host_name() {
        use horizon_core::cloud_runtime::docker_host::{Access, Host};
        let host = |name: &str, access: Access| Host {
            name: name.into(),
            access,
            address: (access != Access::ThisPc).then(|| format!("me@{name}")),
            docker: None,
        };
        let local = Daemon {
            host: Some("unix:///run/user/1000/docker.sock".into()),
            remote: None,
        };
        let builder = host("builder", Access::Ssh);
        assert_eq!(docker_host_of(Some(&local), Some(&builder)), None, "the pin decides");
        assert_eq!(docker_host_of(None, Some(&builder)).as_deref(), Some("builder"));
        assert_eq!(docker_host_of(None, Some(&host("local", Access::ThisPc))), None);
        assert_eq!(docker_host_of(None, None), None);
    }

    #[test]
    fn only_a_socket_of_this_computer_is_pinned() {
        use horizon_core::cloud_runtime::docker_daemon::Endpoint;
        let socket = Daemon::accept(Some(Endpoint::parse("unix:///run/user/1000/docker.sock"))).unwrap();
        assert_eq!(socket.host(), Some("unix:///run/user/1000/docker.sock"));
        for refused in ["tcp://127.0.0.1:2376", "ssh://builder@build.example"] {
            assert!(Daemon::accept(Some(Endpoint::parse(refused))).is_err(), "{refused}");
        }
        assert!(Daemon::accept(None).is_err(), "an unknown daemon is refused");
    }

    #[test]
    fn the_image_and_volume_take_a_name_docker_accepts() {
        assert_eq!(folder_slug(Path::new("/home/me/YouPay v2")), "youpay-v2");
        assert_eq!(folder_slug(Path::new("/")), "repository");
        assert_eq!(
            folder_slug(Path::new(&format!("/home/me/{}", "a".repeat(255)))).len(),
            FOLDER_SLUG_LIMIT
        );
        let (a, b) = (slug(Path::new("/home/a/app")), slug(Path::new("/home/b/app")));
        assert!(a.starts_with("app-") && b.starts_with("app-"));
        assert_ne!(a, b, "two checkouts named app keep their own image and volume");
        assert_eq!(a, slug(Path::new("/home/a/app")), "the same path keeps its volume");
    }

    #[test]
    fn each_setup_run_names_its_own_image_and_volume() {
        let repository = Path::new("/home/me/app");
        let (first, second) = (
            run_id(Path::new("/tmp/horizon-cloud-setup-1-10/status.json")),
            run_id(Path::new("/tmp/horizon-cloud-setup-1-11/status.json")),
        );
        assert_ne!(first, second);
        let (image, volume) = run_names(repository, &first);
        assert_eq!(image, format!("{volume}:candidate"));
        assert_ne!(
            volume,
            run_names(repository, &second).1,
            "two runs never share a volume"
        );
    }
}
