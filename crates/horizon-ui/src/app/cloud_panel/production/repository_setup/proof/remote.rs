//! A proof's Docker on another machine: a Docker host saved in Cloud settings and reached
//! over the person's SSH. An SSH session forwards the host's engine socket to a socket in
//! a private folder of this computer, and every docker command of the proof talks to
//! that socket. The session lives as long as the proof's daemon does.
use horizon_core::cloud_runtime::docker_host::Host;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long the SSH session may take to open its forward.
const OPEN_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the forwarded engine may take to answer its first `docker version`.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(20);

/// The process groups of the SSH forwards that are open now, so Horizon's exit ends
/// those that a value it never drops still holds, such as a cleanup still waiting.
static OPEN: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

fn open_forwards() -> std::sync::MutexGuard<'static, Vec<u32>> {
    OPEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Ends every SSH forward that is still open, as Horizon exits: a forward must not
/// outlive the Horizon that opened it.
pub(in crate::app::cloud_panel::production::repository_setup) fn close_forwards() {
    for group in std::mem::take(&mut *open_forwards()) {
        super::stream::end_group(group);
    }
}

/// An SSH session that keeps one forward open; it ends with this value.
pub(in crate::app::cloud_panel::production::repository_setup) struct Forward {
    child: Child,
}

impl Forward {
    /// Starts `command`, an SSH forward, in its own process group, so its end takes
    /// everything it started with it.
    fn start(mut command: Command) -> Result<Self, String> {
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot start ssh: {error}"))?;
        open_forwards().push(child.id());
        Ok(Self { child })
    }

    /// Whether the SSH session still runs.
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Waits until `ready` says the forward works, or fails when the session ended or
    /// `limit` passed; then the session's own words say why.
    fn wait(&mut self, limit: Duration, ready: impl Fn() -> bool) -> Result<(), String> {
        let started = Instant::now();
        loop {
            if ready() {
                return Ok(());
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                let mut text = String::new();
                if let Some(pipe) = self.child.stderr.take() {
                    use std::io::Read as _;
                    let _ = pipe.take(2048).read_to_string(&mut text);
                }
                return Err(format!(
                    "the SSH session ended with status {}: {}",
                    status.code().unwrap_or(-1),
                    text.trim()
                ));
            }
            if started.elapsed() > limit {
                return Err(format!("the SSH forward was not ready within {} s", limit.as_secs()));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        open_forwards().retain(|group| *group != self.child.id());
        super::stream::end_group(self.child.id());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl std::fmt::Debug for Forward {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Forward")
            .field("pid", &self.child.id())
            .finish()
    }
}

/// A Docker host's engine, forwarded to a socket of this computer.
#[derive(Debug)]
pub(in crate::app::cloud_panel::production::repository_setup) struct Remote {
    host: Host,
    /// The private folder that holds the forwarded socket; it goes with this value.
    _folder: tempfile::TempDir,
    socket: PathBuf,
    forward: std::sync::Mutex<Forward>,
}

impl Remote {
    /// Opens the SSH forward of `host`'s engine socket and checks that the engine
    /// answers through it.
    pub(in crate::app::cloud_panel::production::repository_setup) fn open(host: &Host) -> Result<Self, String> {
        let folder = private_folder()?;
        let socket = folder.path().join("docker.sock");
        let command = host
            .socket_forward(&socket)
            .map_err(|error| error.to_string())?
            .ok_or("this computer's Docker needs no forward")?;
        let mut forward = Forward::start(command)?;
        forward
            .wait(OPEN_TIMEOUT, || socket.exists())
            .map_err(|error| format!("cannot reach the Docker of {}: {error}", host.name))?;
        let address = format!("unix://{}", socket.display());
        forward
            .wait(ANSWER_TIMEOUT, || answers(&address))
            .map_err(|error| format!("the Docker of {} does not answer: {error}", host.name))?;
        Ok(Self {
            host: host.clone(),
            _folder: folder,
            socket,
            forward: std::sync::Mutex::new(forward),
        })
    }

    /// Whether its SSH session still forwards the engine; a session that ended, as when
    /// the network dropped, forwards nothing again.
    pub(in crate::app::cloud_panel::production::repository_setup) fn alive(&self) -> bool {
        self.forward
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .alive()
    }

    /// The Docker host's name, as Cloud settings save it.
    pub(in crate::app::cloud_panel::production::repository_setup) fn name(&self) -> &str {
        &self.host.name
    }

    /// The `DOCKER_HOST` of the forwarded engine.
    pub(in crate::app::cloud_panel::production::repository_setup) fn address(&self) -> String {
        format!("unix://{}", self.socket.display())
    }

    /// Forwards `remote_port` on the host's loopback to a free port on this computer's
    /// loopback, as for a preview's desktop; the port and the session that keeps it.
    pub(in crate::app::cloud_panel::production::repository_setup) fn forward_port(
        &self,
        remote_port: u16,
    ) -> Result<(u16, Forward), String> {
        let local = free_port()?;
        let command = self
            .host
            .port_forward(local, remote_port)
            .map_err(|error| error.to_string())?
            .ok_or("this computer's Docker needs no forward")?;
        let mut forward = Forward::start(command)?;
        forward.wait(OPEN_TIMEOUT, || {
            std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], local)),
                Duration::from_millis(200),
            )
            .is_ok()
        })?;
        Ok((local, forward))
    }
}

/// Whether the engine at `address` answers `docker version` within a few seconds.
fn answers(address: &str) -> bool {
    let mut probe = Command::new("docker");
    probe
        .env("DOCKER_HOST", address)
        .env_remove("DOCKER_CONTEXT")
        .args(["version", "--format", "{{.Server.Version}}"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut probe, 0);
    let Ok(mut child) = probe.spawn() else {
        return false;
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                super::stream::end_group(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// A new folder only this user may open, for a forwarded socket: in the user's runtime
/// folder when there is one, since a socket path must stay short.
fn private_folder() -> Result<tempfile::TempDir, String> {
    let parent: Option<PathBuf> = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|folder| folder.is_dir());
    let mut builder = tempfile::Builder::new();
    builder.prefix("horizon-docker-");
    #[cfg(unix)]
    builder.permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700));
    match parent {
        Some(parent) => builder.tempdir_in(parent),
        None => builder.tempdir(),
    }
    .map_err(|error| format!("cannot make a private folder for the Docker forward: {error}"))
}

/// A port of this computer's loopback that no program holds now.
fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|error| format!("cannot find a free port: {error}"))
}

/// Whether `path` is short enough for a Unix socket address.
#[cfg(test)]
fn fits_a_socket(path: &std::path::Path) -> bool {
    path.as_os_str().len() < 100
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The forwards of these tests share one registry, and one test ends them all.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn the_forwarded_socket_has_a_short_private_path() {
        let folder = private_folder().unwrap();
        assert!(
            fits_a_socket(&folder.path().join("docker.sock")),
            "{}",
            folder.path().display()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(folder.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[test]
    fn a_forward_that_ends_says_why() {
        let _serial = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut command = Command::new("sh");
        command.args(["-c", "echo 'Permission denied (publickey).' >&2; exit 255"]);
        let mut forward = Forward::start(command).unwrap();
        let error = forward.wait(Duration::from_secs(5), || false).unwrap_err();
        assert!(error.contains("status 255") && error.contains("publickey"), "{error}");
    }

    #[test]
    fn horizons_exit_ends_a_forward_that_is_never_dropped() {
        let _serial = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut command = Command::new("sleep");
        command.arg("60");
        let forward = Forward::start(command).unwrap();
        let pid = forward.child.id();
        // Held where nothing drops it, as a cleanup still waiting at exit holds its daemon.
        let held = Box::leak(Box::new(forward));
        assert!(held.alive());
        close_forwards();
        let started = std::time::Instant::now();
        while held.alive() {
            assert!(started.elapsed() < Duration::from_secs(5), "the forward {pid} ended");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
