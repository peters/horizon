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
    process::{Command, Stdio},
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
    /// Docker accepted no answer in time: the daemon is stuck.
    NotResponding,
    /// No Docker CLI to ask.
    Missing,
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

    /// Asks the daemon for its version, waiting at most `timeout`.
    #[must_use]
    pub fn probe(&self, timeout: Duration) -> Health {
        let mut command = self.docker();
        command.args(["version", "--format", "{{.Server.Version}}"]);
        match bounded(command, timeout) {
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
            Ran::Failed(_) => Health::Missing,
        }
    }

    /// Observes where this Docker runs and what can restart it. Runs only commands that
    /// change nothing.
    #[must_use]
    pub fn facts(&self) -> Facts {
        let os = Os::current();
        let (context, endpoint) = self.endpoint();
        let endpoint = endpoint.map(|endpoint| match endpoint {
            Endpoint::Socket(path) => Endpoint::Socket(path.canonicalize().unwrap_or(path)),
            other => other,
        });
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from);
        let systemd = os == Os::Linux && on_path("systemctl");
        let desktop_relevant = os != Os::Linux
            || context.as_deref() == Some("desktop-linux")
            || matches!((&endpoint, &home), (Some(Endpoint::Socket(path)), Some(home)) if path.starts_with(home.join(".docker")));
        Facts {
            os,
            context,
            runtime_dir: std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
            home,
            user_unit: systemd && unit_loaded(true, "docker.service"),
            system_unit: systemd && unit_loaded(false, "docker.service"),
            pkexec: os == Os::Linux && on_path("pkexec"),
            desktop_cli: desktop_relevant && self.desktop_cli(),
            desktop_app: os == Os::MacOs && Path::new("/Applications/Docker.app").is_dir(),
            endpoint,
        }
    }

    /// The context name and daemon address the CLI uses. An explicit host wins, as it
    /// does for the CLI; `DOCKER_HOST` shows as the default context's address.
    fn endpoint(&self) -> (Option<String>, Option<Endpoint>) {
        if let Some(host) = &self.host {
            return (None, Some(Endpoint::parse(host)));
        }
        let mut command = self.docker();
        command.args([
            "context",
            "inspect",
            "--format",
            "{{.Name}}\t{{.Endpoints.docker.Host}}",
        ]);
        match bounded(command, LOOKUP_TIMEOUT) {
            Ran::Exited {
                success: true, stdout, ..
            } => match stdout.trim().split_once('\t') {
                Some((name, host)) if !host.is_empty() => (Some(name.to_owned()), Some(Endpoint::parse(host))),
                _ => (None, None),
            },
            _ => (None, None),
        }
    }

    fn desktop_cli(&self) -> bool {
        let mut command = Command::new(&self.program);
        command.args(["desktop", "version"]);
        matches!(bounded(command, LOOKUP_TIMEOUT), Ran::Exited { success: true, .. })
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
        match bounded(command, STEP_TIMEOUT) {
            Ran::Exited { success: true, .. } => {}
            Ran::Exited { stderr, stdout, .. } => {
                return Err(refused(
                    last_line(&stderr)
                        .or_else(|| last_line(&stdout))
                        .unwrap_or("it gave no reason"),
                ));
            }
            Ran::TimedOut => return Err(RestartError::TimedOut { command: shown }),
            Ran::Failed(error) => return Err(refused(&error.to_string())),
        }
        waiting();
        let deadline = Instant::now() + ANSWER_TIMEOUT;
        loop {
            let health = self.probe(PROBE_TIMEOUT);
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
    #[error("{command} did not finish in {} minutes", STEP_TIMEOUT.as_secs() / 60)]
    TimedOut { command: String },
    #[error("Docker restarted but still does not answer{}", match last {
        Health::NotRunning { detail } => format!(": {detail}"),
        _ => String::new(),
    })]
    NotAnswering { last: Health },
}

fn unit_loaded(user: bool, unit: &str) -> bool {
    let mut command = Command::new("systemctl");
    if user {
        command.arg("--user");
    }
    command.args(["show", "--property=LoadState", "--value", unit]);
    matches!(bounded(command, LOOKUP_TIMEOUT), Ran::Exited { success: true, stdout, .. } if stdout.trim() == "loaded")
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|directory| directory.join(program).is_file()))
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

/// Runs `command` for at most `timeout`. A process it leaves behind holding its
/// output open cannot hold the caller longer than a second past the exit.
fn bounded(mut command: Command, timeout: Duration) -> Ran {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Ran::TimedOut;
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Ran::Failed(error);
            }
        }
    }
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A `docker` program that runs `script` with the arguments it was given.
    fn fake_docker(directory: &Path, script: &str) -> PathBuf {
        let path = directory.join("docker");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn a_docker_command_names_its_target_and_other_programs_have_none() {
        let mut command = Command::new("/usr/local/bin/docker");
        command.args([
            "--config",
            "/state/docker",
            "--host",
            "ssh://build.example",
            "create",
            "--host",
            "x",
        ]);
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
            target("echo \"$*\"").probe(PROBE_TIMEOUT),
            answer(
                "--config /state/docker --host unix:///run/user/1000/docker.sock version --format {{.Server.Version}}"
            ),
            "the health check asks the cloud's own Docker"
        );
        let refusal = "Cannot connect to the Docker daemon at unix:///run/docker.sock.";
        assert_eq!(
            target(&format!("echo '{refusal}' >&2; exit 1")).probe(PROBE_TIMEOUT),
            Health::NotRunning { detail: refusal.into() }
        );
        let started = Instant::now();
        assert_eq!(
            target("exec sleep 30").probe(Duration::from_millis(300)),
            Health::NotResponding
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        let missing = Target {
            program: temp.path().join("absent").join("docker"),
            ..Target::default()
        };
        assert_eq!(missing.probe(PROBE_TIMEOUT), Health::Missing);
    }

    #[test]
    fn an_explicit_host_is_the_endpoint_without_asking_docker() {
        let remote = Target {
            program: "/nonexistent/docker".into(),
            host: Some("ssh://build.example".into()),
            ..Target::default()
        };
        assert_eq!(
            remote.endpoint(),
            (None, Some(Endpoint::Remote("ssh://build.example".into())))
        );
        let temp = tempfile::tempdir().unwrap();
        let context = Target {
            program: fake_docker(temp.path(), "printf 'rootless\\tunix:///run/user/1000/docker.sock\\n'"),
            ..Target::default()
        };
        let socket = Endpoint::Socket("/run/user/1000/docker.sock".into());
        assert_eq!(context.endpoint(), (Some("rootless".into()), Some(socket)));
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
            bounded(command, Duration::from_secs(10)),
            Ran::Exited { success: true, .. }
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
