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
/// those that a value it never drops still holds, such as a cleanup still waiting; and
/// whether the exit closed them, after which no forward may open.
struct Open {
    groups: Vec<u32>,
    closed: bool,
}

static OPEN: std::sync::Mutex<Open> = std::sync::Mutex::new(Open {
    groups: Vec::new(),
    closed: false,
});

fn open_forwards() -> std::sync::MutexGuard<'static, Open> {
    OPEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Ends every SSH forward that is still open, as Horizon exits, and refuses any later
/// one: a forward must not outlive the Horizon that opened it.
pub(in crate::app::cloud_panel::production::repository_setup) fn close_forwards() {
    // The tests of one process each drop their app while other tests hold forwards; the
    // registry's own test closes it.
    #[cfg(not(test))]
    close(true);
}

/// Ends every open forward, and refuses later ones when `refuse`.
fn close(refuse: bool) {
    let mut open = open_forwards();
    open.closed |= refuse;
    for group in std::mem::take(&mut open.groups) {
        super::stream::end_group(group);
    }
}

/// An SSH session that keeps one forward open; it ends with this value.
pub(in crate::app::cloud_panel::production::repository_setup) struct Forward {
    child: Child,
    /// The session's first words on its standard error, read as they come so a talkative
    /// SSH never blocks on a full pipe; the rest is read and dropped.
    said: std::sync::Arc<std::sync::Mutex<String>>,
}

/// How much of an SSH session's standard error a forward keeps to say why it ended.
const SAID_LIMIT: usize = 2048;

impl Forward {
    /// Starts `command`, an SSH forward, in its own process group, so its end takes
    /// everything it started with it.
    fn start(mut command: Command) -> Result<Self, String> {
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        // Started and registered under the registry's lock, so an exit that closes the
        // forwards meanwhile cannot miss this one.
        let mut open = open_forwards();
        if open.closed {
            return Err("Horizon is exiting".to_owned());
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot start ssh: {error}"))?;
        open.groups.push(child.id());
        let said = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        if let Some(mut pipe) = child.stderr.take() {
            let kept = std::sync::Arc::clone(&said);
            std::thread::spawn(move || {
                let mut buffer = [0_u8; 4096];
                while let Ok(read) = std::io::Read::read(&mut pipe, &mut buffer) {
                    if read == 0 {
                        break;
                    }
                    let mut kept = kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    let room = SAID_LIMIT.saturating_sub(kept.len()).min(read);
                    kept.push_str(&String::from_utf8_lossy(&buffer[..room]));
                }
            });
        }
        Ok(Self { child, said })
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
                // The reader takes what the session wrote last a moment after it ended.
                std::thread::sleep(Duration::from_millis(100));
                let text = self
                    .said
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
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
        open_forwards().groups.retain(|group| *group != self.child.id());
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
            .finish_non_exhaustive()
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
    /// The engine's own ID and its version, as it answered when the forward opened.
    engine_id: String,
    engine_version: String,
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
        let (engine_id, engine_version) =
            engine(&address).ok_or_else(|| format!("the Docker of {} does not say which engine it is", host.name))?;
        Ok(Self {
            host: host.clone(),
            _folder: folder,
            socket,
            forward: std::sync::Mutex::new(forward),
            engine_id,
            engine_version,
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

    /// The engine's own ID, which tells this engine from another one the same name
    /// reaches later.
    pub(in crate::app::cloud_panel::production::repository_setup) fn engine_id(&self) -> &str {
        &self.engine_id
    }

    /// Whether the forward reaches the engine it reached when it opened, asked now: a host
    /// whose socket another engine took since answers with another ID, and one that does
    /// not answer counts as another.
    pub(in crate::app::cloud_panel::production::repository_setup) fn same_engine(&self) -> bool {
        engine(&self.address()).is_some_and(|(id, _)| id == self.engine_id)
    }

    /// Whether the engine keeps ports it publishes on its loopback private: Docker before
    /// stable 28 lets hosts on the same network reach them, so a preview's desktop, which
    /// has no password, is not published there.
    pub(in crate::app::cloud_panel::production::repository_setup) fn loopback_private(&self) -> bool {
        horizon_core::cloud_runtime::docker_host::private_ports(&self.engine_version)
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

/// The ID and the version of the engine at `address`, or none when it does not say
/// within a few seconds.
fn engine(address: &str) -> Option<(String, String)> {
    let mut probe = Command::new("docker");
    probe
        .env("DOCKER_HOST", address)
        .env_remove("DOCKER_CONTEXT")
        .args(["info", "--format", "{{.ID}} {{.ServerVersion}}"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut probe, 0);
    let mut child = probe.spawn().ok()?;
    let started = Instant::now();
    while child.try_wait().ok()?.is_none() {
        if started.elapsed() > Duration::from_secs(10) {
            super::stream::end_group(child.id());
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::Read::take(child.stdout.take()?, 4096), &mut text).ok()?;
    let (id, version) = text.trim().split_once(' ')?;
    (!id.is_empty() && !version.is_empty()).then(|| (id.to_owned(), version.to_owned()))
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

    // A process group ends only on Unix.
    #[cfg(unix)]
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
        close(true);
        let started = std::time::Instant::now();
        while held.alive() {
            assert!(started.elapsed() < Duration::from_secs(5), "the forward {pid} ended");
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut command = Command::new("sleep");
        command.arg("60");
        assert!(
            Forward::start(command).is_err(),
            "no forward opens once the exit closed them"
        );
        // The other tests of this process open forwards again.
        open_forwards().closed = false;
    }
}
