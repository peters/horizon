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

/// The Docker daemon of this computer a proof uses, pinned when the proof starts: every
/// docker command of the proof and of its cleanup goes to that daemon, so a Docker context
/// switched meanwhile never sends one to another machine.
#[derive(Clone, Debug, Default)]
pub(in crate::app::cloud_panel::production::repository_setup) struct LocalDocker {
    /// The daemon's address; none only before a proof's run has checked it, when the run
    /// sends no docker command.
    host: Option<String>,
}

/// A proof's pinned daemon, shared by the proof's run, its Stop and its cleanup; empty
/// until the proof's run has checked it.
pub(in crate::app::cloud_panel::production::repository_setup) type Pinned =
    std::sync::Arc<std::sync::OnceLock<LocalDocker>>;

impl LocalDocker {
    /// The daemon the Docker CLI uses now, accepted only through a socket or pipe of
    /// this computer. The proof mounts files of this computer into its containers, the
    /// repository's bundle and snapshot and Horizon's localci, and a daemon elsewhere
    /// would look for those paths on its own machine. A socket needs no TLS settings, so
    /// its address alone pins it; the mount check then refuses a socket forwarded from
    /// another machine.
    pub(in crate::app::cloud_panel::production::repository_setup) fn resolve() -> Result<Self, String> {
        Self::accept(horizon_core::cloud_runtime::docker_daemon::Target::default().daemon())
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
            }),
            Some(endpoint) => Err(format!(
                "Docker here uses the daemon at {}, not a socket of this computer. Horizon's proof mounts files from this computer, so it needs Docker running on this computer through its socket: switch to a local Docker context (docker context use default) or unset DOCKER_HOST, then start the setup again.",
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
    daemon: &'a LocalDocker,
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
pub(super) fn remove_now(daemon: &LocalDocker, args: &[&str]) -> Option<String> {
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
    pinned: Option<Pinned>,
    wait: bool,
) {
    let (image, volume) = run_names(repository, run);
    let remove = move || {
        if let Some(after) = after {
            let _ = after.join();
        }
        // The daemon the proof pinned; without a proof that checked one, the daemon of
        // this computer Docker uses now. A daemon on another machine is never touched.
        let pinned = pinned.as_ref().and_then(|pinned| pinned.get().cloned());
        let Some(daemon) = pinned.or_else(|| LocalDocker::resolve().ok()) else {
            return;
        };
        let cleanup = &Cleanup {
            daemon: &daemon,
            deadline: std::time::Instant::now() + CLEANUP_TIMEOUT,
        };
        // The proof's own containers and the worker-check containers of its profiles.
        // A check stopped mid-way can still have its container appear a moment later,
        // so the search runs once more after a pause.
        let checks = format!("^/horizon-contract-{}-", check_operations(&volume));
        for pass in 0..2 {
            if pass > 0 {
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            let found: Vec<String> = containers(cleanup, &format!("^/{volume}-"))
                .into_iter()
                .chain(containers(cleanup, &checks))
                .collect();
            if !found.is_empty() {
                let mut remove = arguments(["rm", "--force", "--volumes"]);
                remove.extend(found);
                let _ = docker(cleanup, &remove);
            }
        }
        let filter = format!("reference={volume}-build*");
        let images: Vec<String> = docker(
            cleanup,
            &arguments(["images", "--format", "{{.Repository}}:{{.Tag}}", "--filter", &filter]),
        )
        .map(|output| output.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
        let mut remove_images = arguments(["rmi", "--force", &image]);
        remove_images.extend(images);
        let _ = docker(cleanup, &remove_images);
        let _ = docker(cleanup, &arguments(["volume", "rm", "--force", &volume]));
    };
    if wait {
        remove();
    } else {
        super::super::run::keep_ending(std::thread::spawn(remove));
    }
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
    fn only_a_socket_of_this_computer_is_pinned() {
        use horizon_core::cloud_runtime::docker_daemon::Endpoint;
        let socket = LocalDocker::accept(Some(Endpoint::parse("unix:///run/user/1000/docker.sock"))).unwrap();
        assert_eq!(socket.host(), Some("unix:///run/user/1000/docker.sock"));
        for refused in ["tcp://127.0.0.1:2376", "ssh://builder@build.example"] {
            assert!(
                LocalDocker::accept(Some(Endpoint::parse(refused))).is_err(),
                "{refused}"
            );
        }
        assert!(LocalDocker::accept(None).is_err(), "an unknown daemon is refused");
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
