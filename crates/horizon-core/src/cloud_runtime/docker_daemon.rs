//! Whether the Docker a cloud builds with answers, and restarting it when it does not.
//!
//! A Docker daemon can hang so that every command waits forever. What a person then sees
//! is the next symptom, such as a container name still in use by a create that was
//! stopped. A short `docker version` tells a stuck daemon from a slow command.
mod plan;
pub use plan::{Endpoint, Facts, Os, Plan, plan};
use std::{
    ffi::OsStr,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

/// How long a health check waits for Docker to answer.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long one restart command may take. systemd waits up to 90 s for a hung daemon
/// to stop, and `pkexec` waits while the person types a password.
const STEP_TIMEOUT: Duration = Duration::from_mins(5);
/// How long Docker may take to answer after a restart.
const ANSWER_TIMEOUT: Duration = Duration::from_mins(2);
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
const CONTEXT_FORMAT: &str = "{{.Name}}\t{{.Endpoints.docker.Host}}";

/// How Horizon runs the Docker CLI for a cloud: the program, its configuration
/// directory and the daemon address from cloud settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub program: PathBuf,
    pub config: Option<PathBuf>,
    pub host: Option<String>,
}

impl Default for Target {
    fn default() -> Self {
        Self {
            program: PathBuf::from("docker"),
            config: None,
            host: None,
        }
    }
}

/// What a health check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Health {
    Answering {
        version: String,
    },
    /// Docker refused the connection or is not running; `detail` is what the CLI said.
    NotRunning {
        detail: String,
    },
    /// Docker gave no answer in time: the daemon is stuck.
    NotResponding,
}

impl Target {
    #[must_use]
    pub fn from_settings(settings: &super::settings::Settings) -> Self {
        Self {
            config: Some(settings.docker_config.clone()),
            host: settings.docker_host.clone(),
            ..Self::default()
        }
    }

    /// The target of a Docker command Horizon built, read from its leading global
    /// options; `None` for any other program.
    #[must_use]
    pub fn of(command: &Command) -> Option<Self> {
        let program = Path::new(command.get_program());
        let name = program.file_stem().and_then(OsStr::to_str)?;
        if name != "docker" {
            return None;
        }
        let mut target = Self {
            program: program.to_path_buf(),
            ..Self::default()
        };
        let mut args = command.get_args();
        while let Some(option) = args.next() {
            let mut value = || args.next().map(|value| value.to_string_lossy().into_owned());
            match option.to_str() {
                Some("--config") => target.config = value().map(PathBuf::from),
                Some("--host" | "-H") => target.host = value(),
                _ => break,
            }
        }
        Some(target)
    }

    fn docker(&self) -> Command {
        let mut command = Command::new(&self.program);
        if let Some(config) = &self.config {
            command.arg("--config").arg(config);
        }
        if let Some(host) = &self.host {
            command.arg("--host").arg(host);
        }
        command
    }

    /// The program and its global options, for a command that must reach this Docker too.
    fn prefix(&self) -> Vec<String> {
        let command = self.docker();
        std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(|part| part.to_string_lossy().into_owned())
            .collect()
    }

    /// Asks the daemon for its version, waiting at most `timeout` or until `stop`.
    #[must_use]
    pub fn probe(&self, timeout: Duration, stop: &dyn Fn() -> bool) -> Health {
        let mut command = self.docker();
        command.args(["version", "--format", "{{.Server.Version}}"]);
        match bounded(command, timeout, stop) {
            Ran::Exited {
                success: true, stdout, ..
            } if !stdout.trim().is_empty() => Health::Answering {
                version: stdout.trim().to_owned(),
            },
            Ran::Exited { stderr, .. } => Health::NotRunning {
                detail: last_line(&stderr)
                    .unwrap_or("Docker did not report its version")
                    .to_owned(),
            },
            Ran::TimedOut => Health::NotResponding,
            Ran::Failed(error) => Health::NotRunning {
                detail: error.to_string(),
            },
        }
    }

    /// Observes where this Docker runs and what can restart it. Runs only commands that
    /// change nothing.
    #[must_use]
    pub fn facts(&self) -> Facts {
        let os = Os::current();
        let inherited = std::env::var("DOCKER_HOST").ok().filter(|host| !host.is_empty());
        let (context, endpoint) = self.endpoint(inherited);
        let endpoint = endpoint.map(|endpoint| match endpoint {
            Endpoint::Socket(path) => Endpoint::Socket(path.canonicalize().unwrap_or(path)),
            other => other,
        });
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|home| PathBuf::from(&home).canonicalize().unwrap_or_else(|_| home.into()));
        let systemd = os == Os::Linux && on_path("systemctl").is_some();
        let desktop_relevant = os != Os::Linux
            || context.as_deref() == Some("desktop-linux")
            || matches!((&endpoint, &home), (Some(Endpoint::Socket(path)), Some(home)) if path.starts_with(home.join(".docker")));
        let desktop = desktop_relevant.then(|| self.desktop_cli()).flatten();
        Facts {
            os,
            context,
            runtime_dir: std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
            home,
            user_unit: systemd && unit_active(true),
            system_unit: systemd && unit_active(false),
            pkexec: on_path("pkexec").filter(|_| os == Os::Linux),
            desktop_cli: desktop.is_some(),
            docker: desktop.unwrap_or_default(),
            endpoint,
        }
    }

    /// The context name and daemon address the CLI uses. As for the CLI, an explicit
    /// host wins, then the `inherited` `DOCKER_HOST`, then the context.
    fn endpoint(&self, inherited: Option<String>) -> (Option<String>, Option<Endpoint>) {
        if let Some(host) = self.host.clone().or(inherited) {
            return (None, Some(Endpoint::parse(&host)));
        }
        let mut command = self.docker();
        command.args(["context", "inspect", "--format", CONTEXT_FORMAT]);
        match bounded(command, LOOKUP_TIMEOUT, &|| false) {
            Ran::Exited {
                success: true, stdout, ..
            } => match stdout.trim().split_once('\t') {
                Some((name, host)) if !host.is_empty() => (Some(name.to_owned()), Some(Endpoint::parse(host))),
                _ => (None, None),
            },
            _ => (None, None),
        }
    }

    /// The Docker command that answers `docker desktop version`, which then restarts
    /// Docker Desktop too: with the cloud's options first, then without them, as the
    /// default configuration may be the one that finds the plugin.
    fn desktop_cli(&self) -> Option<Vec<String>> {
        let plain = Self {
            program: self.program.clone(),
            ..Self::default()
        };
        std::iter::once(self)
            .chain((*self != plain).then_some(&plain))
            .find(|target| {
                let mut command = target.docker();
                command.args(["desktop", "version"]);
                matches!(
                    bounded(command, LOOKUP_TIMEOUT, &|| false),
                    Ran::Exited { success: true, .. }
                )
            })
            .map(Self::prefix)
    }

    /// Runs `argv`, then waits until Docker answers. `waiting` hears when the wait starts.
    /// # Errors
    /// The command failing or outliving its time, or Docker still not answering.
    pub fn restart(&self, argv: &[String], waiting: &dyn Fn()) -> Result<String, RestartError> {
        let shown = argv.join(" ");
        let mut command = Command::new(argv.first().map_or("", String::as_str));
        command.args(argv.iter().skip(1));
        let refused = |detail: &str| RestartError::Refused {
            command: shown.clone(),
            detail: detail.to_owned(),
        };
        match bounded(command, STEP_TIMEOUT, &|| false) {
            Ran::Exited { success: true, .. } => {}
            Ran::Exited { stderr, stdout, .. } => {
                return Err(refused(
                    last_line(&stderr)
                        .or_else(|| last_line(&stdout))
                        .unwrap_or("it gave no reason"),
                ));
            }
            Ran::TimedOut => return Err(refused("it did not finish in 5 minutes")),
            Ran::Failed(error) => return Err(refused(&error.to_string())),
        }
        waiting();
        let deadline = Instant::now() + ANSWER_TIMEOUT;
        loop {
            let health = self.probe(PROBE_TIMEOUT, &|| false);
            if let Health::Answering { version } = health {
                return Ok(version);
            }
            if Instant::now() >= deadline {
                return Err(RestartError::NotAnswering { last: health });
            }
            thread::sleep(Duration::from_secs(2));
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RestartError {
    #[error("{command} failed: {detail}")]
    Refused { command: String, detail: String },
    #[error("Docker restarted but still does not answer{}", match last {
        Health::NotRunning { detail } => format!(": {detail}"),
        _ => String::new(),
    })]
    NotAnswering { last: Health },
}

/// Whether systemd runs `docker.service` now. A hung daemon still counts as active; an
/// installed unit that is stopped does not serve the socket in use.
fn unit_active(user: bool) -> bool {
    let mut command = Command::new("systemctl");
    if user {
        command.arg("--user");
    }
    command.args(["show", "--property=ActiveState", "--value", "docker.service"]);
    matches!(bounded(command, LOOKUP_TIMEOUT, &|| false), Ran::Exited { success: true, stdout, .. } if stdout.trim() == "active")
}

fn on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(program))
        .find(|path| path.is_file())
}

fn last_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).rfind(|line| !line.is_empty())
}

enum Ran {
    Exited {
        success: bool,
        stdout: String,
        stderr: String,
    },
    TimedOut,
    Failed(std::io::Error),
}

/// Runs `command` for at most `timeout`, or until `stop`. A process it leaves behind
/// holding its output open cannot hold the caller longer than a second past the exit.
fn bounded(mut command: Command, timeout: Duration, stop: &dyn Fn() -> bool) -> Ran {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return Ran::Failed(error),
    };
    let stdout = child.stdout.take().map(read);
    let stderr = child.stderr.take().map(read);
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let collect = |pipe: Option<mpsc::Receiver<String>>| {
                    pipe.and_then(|pipe| pipe.recv_timeout(Duration::from_secs(1)).ok())
                        .unwrap_or_default()
                };
                return Ran::Exited {
                    success: status.success(),
                    stdout: collect(stdout),
                    stderr: collect(stderr),
                };
            }
            Ok(None) if Instant::now() < deadline && !stop() => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                end(child);
                return Ran::TimedOut;
            }
            Err(error) => {
                end(child);
                return Ran::Failed(error);
            }
        }
    }
}

/// Kills the process group, or on Windows the process tree, as an `ssh` that
/// `docker --host ssh://` starts or a CLI plugin is in it. A child that outlives the
/// kill, such as the root `systemctl` that `pkexec` starts, is waited for on its own
/// thread so that the caller never blocks on it.
fn end(mut child: Child) {
    #[cfg(unix)]
    if let Some(id) = rustix::process::Pid::from_raw(child.id().cast_signed()) {
        let _ = rustix::process::kill_process_group(id, rustix::process::Signal::KILL);
    }
    // Before the kill: `taskkill /T` finds the descendants through their living parent.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &child.id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
    let _ = child.kill();
    thread::spawn(move || child.wait());
}

fn read(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        let _ = sender.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    receiver
}

/// A `docker` program that runs `script` with the arguments it was given. It is returned
/// once it starts: a child that another test thread forked while the file was open for
/// writing keeps it busy until that child runs its own program.
#[cfg(all(test, unix))]
pub(super) fn fake_docker(directory: &Path, script: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = directory.join("docker");
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..200 {
        match Command::new(&path).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            Ok(child) => {
                end(child);
                return path;
            }
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("{}: {error}", path.display()),
        }
    }
    panic!("{} stayed busy", path.display());
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_docker_command_names_its_target_and_other_programs_have_none() {
        let mut command = Command::new("/usr/local/bin/docker");
        command.args("--config /state/docker --host ssh://build.example create --host x".split(' '));
        let target = Target {
            program: "/usr/local/bin/docker".into(),
            config: Some("/state/docker".into()),
            host: Some("ssh://build.example".into()),
        };
        assert_eq!(Target::of(&command), Some(target));
        let mut plain = Command::new("docker");
        plain.args(["push", "--config", "ignored"]);
        assert_eq!(Target::of(&plain), Some(Target::default()));
        assert_eq!(Target::of(&Command::new("git")), None);
        assert_eq!(Target::of(&Command::new("dockerd")), None);
    }

    #[test]
    fn a_health_check_tells_an_answer_a_refusal_and_a_hang_apart() {
        let temp = tempfile::tempdir().unwrap();
        let target = |script: &str| Target {
            program: fake_docker(temp.path(), script),
            config: Some("/state/docker".into()),
            host: Some("unix:///run/user/1000/docker.sock".into()),
        };
        let answer = |version: &str| Health::Answering {
            version: version.into(),
        };
        assert_eq!(
            target("echo \"$*\"").probe(PROBE_TIMEOUT, &|| false),
            answer(
                "--config /state/docker --host unix:///run/user/1000/docker.sock version --format {{.Server.Version}}"
            ),
            "the health check asks the cloud's own Docker"
        );
        let refusal = "Cannot connect to the Docker daemon at unix:///run/docker.sock.";
        assert_eq!(
            target(&format!("echo '{refusal}' >&2; exit 1")).probe(PROBE_TIMEOUT, &|| false),
            Health::NotRunning { detail: refusal.into() }
        );
        let started = Instant::now();
        assert_eq!(
            target("exec sleep 30").probe(Duration::from_millis(300), &|| false),
            Health::NotResponding
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn an_explicit_host_is_the_endpoint_without_asking_docker() {
        let remote = Target {
            program: "/nonexistent/docker".into(),
            host: Some("ssh://build.example".into()),
            ..Target::default()
        };
        let inherited = || Some("tcp://192.0.2.10:2376".to_owned());
        assert_eq!(
            remote.endpoint(inherited()),
            (None, Some(Endpoint::Remote("ssh://build.example".into())))
        );
        let temp = tempfile::tempdir().unwrap();
        let context = Target {
            program: fake_docker(temp.path(), "printf 'rootless\\tunix:///run/user/1000/docker.sock\\n'"),
            ..Target::default()
        };
        let socket = Endpoint::Socket("/run/user/1000/docker.sock".into());
        assert_eq!(context.endpoint(None), (Some("rootless".into()), Some(socket)));
        assert_eq!(
            context.endpoint(inherited()),
            (None, Some(Endpoint::Remote("tcp://192.0.2.10:2376".into()))),
            "DOCKER_HOST overrides the context, as it does for the CLI"
        );
    }

    #[test]
    fn docker_desktop_is_found_and_restarted_with_the_cloud_docker_options() {
        let temp = tempfile::tempdir().unwrap();
        let target = Target {
            program: fake_docker(
                temp.path(),
                "[ \"$1 $2 $3 $4\" = '--config /state/docker desktop version' ]",
            ),
            config: Some("/state/docker".into()),
            host: None,
        };
        let program = target.program.to_string_lossy().into_owned();
        let configured = [program.as_str(), "--config", "/state/docker"].map(String::from);
        assert_eq!(target.desktop_cli(), Some(configured.into()));
        // The plugin found only through the default configuration restarts without the cloud's.
        fake_docker(temp.path(), "[ \"$1 $2\" = 'desktop version' ]");
        assert_eq!(target.desktop_cli(), Some(vec![program]));
        fake_docker(temp.path(), "exit 1");
        assert_eq!(target.desktop_cli(), None);
    }

    #[test]
    fn a_restart_waits_for_an_answer_and_a_refused_one_says_why() {
        let temp = tempfile::tempdir().unwrap();
        let target = Target {
            program: fake_docker(temp.path(), "echo 29.8.1"),
            ..Target::default()
        };
        let waited = std::cell::Cell::new(false);
        assert_eq!(
            target.restart(&["true".into()], &|| waited.set(true)),
            Ok("29.8.1".into())
        );
        assert!(waited.get());

        let script = "echo 'Interactive authentication required.' >&2; exit 1";
        let refused = target.restart(&["sh".into(), "-c".into(), script.into()], &|| {
            panic!("nothing to wait for")
        });
        let detail = "Interactive authentication required.".into();
        assert_eq!(
            refused,
            Err(RestartError::Refused {
                command: format!("sh -c {script}"),
                detail
            })
        );
    }

    #[test]
    fn a_child_holding_the_output_open_cannot_hold_the_caller() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & echo done"]);
        let started = Instant::now();
        assert!(matches!(
            bounded(command, Duration::from_secs(10), &|| false),
            Ran::Exited { success: true, .. }
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
