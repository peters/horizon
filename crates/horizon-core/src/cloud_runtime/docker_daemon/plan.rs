//! How the Docker that Horizon builds with can be restarted, decided from what Horizon
//! observed rather than guessed. When the observations do not show one known setup,
//! the person gets instructions and Horizon runs nothing.
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
    #[default]
    Other,
}

impl Os {
    #[must_use]
    pub fn current() -> Self {
        match std::env::consts::OS {
            "linux" => Self::Linux,
            "macos" => Self::MacOs,
            "windows" => Self::Windows,
            _ => Self::Other,
        }
    }
}

/// The daemon address the Docker CLI uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Endpoint {
    /// A Unix socket on this computer, with symbolic links resolved when possible.
    Socket(PathBuf),
    /// A Windows named pipe, such as `//./pipe/docker_engine`.
    Pipe(String),
    /// Docker on another machine: `ssh://`, `tcp://` or any other address.
    Remote(String),
}

impl Endpoint {
    #[must_use]
    pub fn parse(host: &str) -> Self {
        let host = host.trim();
        if let Some(path) = host.strip_prefix("unix://") {
            Self::Socket(PathBuf::from(path))
        } else if let Some(pipe) = host.strip_prefix("npipe://") {
            Self::Pipe(pipe.to_owned())
        } else {
            Self::Remote(host.to_owned())
        }
    }
}

/// What Horizon observed about the Docker a cloud uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub os: Os,
    /// `None` when the Docker CLI could not say which daemon it uses.
    pub endpoint: Option<Endpoint>,
    /// The name of the Docker context in use, such as `rootless` or `desktop-linux`.
    pub context: Option<String>,
    /// `$XDG_RUNTIME_DIR`, where rootless Docker keeps its socket.
    pub runtime_dir: Option<PathBuf>,
    pub home: Option<PathBuf>,
    /// systemd runs the user unit `docker.service`, which runs rootless Docker.
    pub user_unit: bool,
    /// systemd runs the system unit `docker.service`.
    pub system_unit: bool,
    /// Where `pkexec` is on `PATH`, to ask for administrator rights.
    pub pkexec: Option<PathBuf>,
    /// `docker desktop version` answered: Docker Desktop 4.37 or later with its CLI.
    pub desktop_cli: bool,
    /// The Docker command that answered `docker desktop version`, which the Docker
    /// Desktop restart runs as well; plain `docker` when empty.
    pub docker: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Horizon restarts Docker by running `argv` once the person confirms. `shown`
    /// says what runs; `command` is what a person can type instead.
    Restart {
        argv: Vec<String>,
        shown: String,
        command: Option<String>,
    },
    /// Horizon runs nothing: the person restarts Docker as `instructions` say,
    /// with `command` to copy when there is one.
    Manual {
        instructions: String,
        command: Option<String>,
    },
}

impl Plan {
    /// The command a person can run themselves, if Horizon knows one.
    #[must_use]
    pub fn command(&self) -> Option<&str> {
        match self {
            Self::Restart { command, .. } | Self::Manual { command, .. } => command.as_deref(),
        }
    }

    /// Whether the restart asks the person for an administrator password.
    #[must_use]
    pub fn asks_password(&self) -> bool {
        matches!(self, Self::Restart { argv, .. } if argv.first().is_some_and(|program| program == "pkexec"))
    }
}

const DESKTOP_CONTEXT: &str = "desktop-linux";
const DESKTOP_CLI: [&str; 3] = ["docker", "desktop", "restart"];
const ROOTLESS: [&str; 4] = ["systemctl", "--user", "restart", "docker.service"];
const SERVICE: [&str; 4] = ["pkexec", "systemctl", "restart", "docker.service"];
const SERVICE_COMMAND: &str = "sudo systemctl restart docker";

const NO_ENDPOINT: &str = "Horizon could not find out where this Docker runs. Restart Docker the way it was started on this computer, then retry.";
const UNKNOWN: &str = "Horizon does not know how this Docker was started. Restart it the way it was started on this computer, then retry.";
const ROOTLESS_WITHOUT_UNIT: &str = "This is rootless Docker, but no systemd user unit runs it. Stop dockerd and start it again the way it was started, for example with dockerd-rootless.sh.";
const SERVICE_WITHOUT_PKEXEC: &str = "Docker runs as a system service, and restarting it needs administrator rights. Run this command in a terminal, then retry.";
const DESKTOP_MENU: &str = "Quit Docker Desktop from its menu, open it again, then retry.";
const MAC_OTHER: &str =
    "This Docker is not Docker Desktop. Restart the program that runs it, such as Colima or OrbStack, then retry.";
const WINDOWS_DESKTOP: &str = "Right-click the Docker Desktop icon in the notification area, choose Quit Docker Desktop, start Docker Desktop again, then retry.";
const WINDOWS_OTHER: &str = "Restart Docker Desktop from the icon in the notification area or, for the Docker service, run this command in an administrator PowerShell. Then retry.";

fn restart(argv: &[&str], shown: &str, command: Option<&str>) -> Plan {
    Plan::Restart {
        argv: argv.iter().map(|&part| part.to_owned()).collect(),
        shown: shown.to_owned(),
        command: command.map(str::to_owned),
    }
}

fn manual(instructions: impl Into<String>, command: Option<&str>) -> Plan {
    Plan::Manual {
        instructions: instructions.into(),
        command: command.map(str::to_owned),
    }
}

/// How to restart the Docker that `facts` describe.
#[must_use]
pub fn plan(facts: &Facts) -> Plan {
    match &facts.endpoint {
        None => manual(NO_ENDPOINT, None),
        Some(Endpoint::Remote(host)) => manual(
            format!(
                "This Docker is at {host}. Horizon restarts only a Docker that it finds on a socket of this computer, so restart Docker at that address, then retry."
            ),
            None,
        ),
        Some(Endpoint::Socket(path)) if facts.os == Os::Linux => linux(facts, path),
        Some(Endpoint::Socket(path)) if facts.os == Os::MacOs => mac(facts, path),
        Some(Endpoint::Pipe(pipe)) if facts.os == Os::Windows => windows(facts, pipe),
        Some(_) => manual(UNKNOWN, None),
    }
}

fn linux(facts: &Facts, socket: &Path) -> Plan {
    let rootless = facts
        .runtime_dir
        .as_deref()
        .map(|directory| directory.join("docker.sock"));
    let rootless = rootless.as_deref() == Some(socket);
    let system = [Path::new("/var/run/docker.sock"), Path::new("/run/docker.sock")].contains(&socket);
    let rootless_command = "systemctl --user restart docker";
    if rootless && facts.user_unit {
        restart(&ROOTLESS, rootless_command, Some(rootless_command))
    } else if rootless {
        manual(ROOTLESS_WITHOUT_UNIT, None)
    } else if is_desktop(facts, socket, ".docker/desktop/docker.sock") {
        desktop(facts, DESKTOP_MENU)
    } else if system && facts.system_unit && facts.pkexec.is_some() {
        restart(&SERVICE, "pkexec systemctl restart docker", Some(SERVICE_COMMAND))
    } else if system && facts.system_unit {
        manual(SERVICE_WITHOUT_PKEXEC, Some(SERVICE_COMMAND))
    } else {
        manual(UNKNOWN, None)
    }
}

fn mac(facts: &Facts, socket: &Path) -> Plan {
    if is_desktop(facts, socket, ".docker/run/docker.sock") {
        desktop(facts, DESKTOP_MENU)
    } else {
        manual(MAC_OTHER, None)
    }
}

fn windows(facts: &Facts, pipe: &str) -> Plan {
    // The generic `docker_engine` pipe can be the Windows Docker service even where
    // Docker Desktop's CLI is installed, so only Docker Desktop's own pipe or context counts.
    if facts.context.as_deref() == Some(DESKTOP_CONTEXT) || pipe.ends_with("dockerDesktopLinuxEngine") {
        desktop(facts, WINDOWS_DESKTOP)
    } else {
        manual(WINDOWS_OTHER, Some("Restart-Service docker"))
    }
}

/// Docker Desktop restarts through its own CLI when it has one, else as `instructions` say.
fn desktop(facts: &Facts, instructions: &str) -> Plan {
    let shown = DESKTOP_CLI.join(" ");
    if facts.desktop_cli {
        let docker: Vec<&str> = facts.docker.iter().map(String::as_str).collect();
        let docker = if docker.is_empty() {
            &DESKTOP_CLI[..1]
        } else {
            &docker[..]
        };
        let argv = [docker, &DESKTOP_CLI[1..]].concat();
        restart(&argv, &shown, Some(&shown))
    } else {
        manual(instructions, None)
    }
}

/// Docker Desktop: its context, or its socket under the home directory at `relative` or,
/// on macOS, inside the app's container where the socket link points.
fn is_desktop(facts: &Facts, socket: &Path, relative: &str) -> bool {
    facts.context.as_deref() == Some(DESKTOP_CONTEXT)
        || facts.home.as_deref().is_some_and(|home| {
            socket == home.join(relative) || socket.starts_with(home.join("Library/Containers/com.docker.docker"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux(endpoint: &str) -> Facts {
        Facts {
            os: Os::Linux,
            endpoint: Some(Endpoint::parse(endpoint)),
            runtime_dir: Some("/run/user/1000".into()),
            home: Some("/home/person".into()),
            ..Facts::default()
        }
    }

    /// What Horizon runs, or the instructions when it runs nothing.
    fn runs(facts: &Facts) -> Result<String, String> {
        match plan(facts) {
            Plan::Restart { argv, .. } => Ok(argv.join(" ")),
            Plan::Manual { instructions, .. } => Err(instructions),
        }
    }

    #[test]
    fn rootless_docker_restarts_through_its_user_unit_only_when_systemd_runs_it() {
        // A system unit and pkexec do not turn rootless Docker into the system service.
        let mut facts = Facts {
            user_unit: true,
            system_unit: true,
            pkexec: Some("/usr/bin/pkexec".into()),
            ..linux("unix:///run/user/1000/docker.sock")
        };
        assert_eq!(runs(&facts).as_deref(), Ok("systemctl --user restart docker.service"));
        assert_eq!(plan(&facts).command(), Some("systemctl --user restart docker"));
        assert!(!plan(&facts).asks_password());
        facts.user_unit = false;
        assert_eq!(runs(&facts), Err(ROOTLESS_WITHOUT_UNIT.into()));
    }

    #[test]
    fn the_system_service_asks_pkexec_for_rights_or_shows_the_command_to_copy() {
        for socket in ["unix:///var/run/docker.sock", "unix:///run/docker.sock"] {
            let mut facts = Facts {
                system_unit: true,
                pkexec: Some("/usr/bin/pkexec".into()),
                ..linux(socket)
            };
            assert_eq!(runs(&facts).as_deref(), Ok("pkexec systemctl restart docker.service"));
            assert!(plan(&facts).asks_password());
            assert_eq!(plan(&facts).command(), Some(SERVICE_COMMAND), "a person types sudo");
            facts.pkexec = None;
            assert_eq!(runs(&facts), Err(SERVICE_WITHOUT_PKEXEC.into()), "{socket}");
            assert_eq!(plan(&facts).command(), Some(SERVICE_COMMAND));
            // A system socket that no systemd unit runs is not restarted.
            facts.pkexec = Some("/usr/bin/pkexec".into());
            facts.system_unit = false;
            assert_eq!(runs(&facts), Err(UNKNOWN.into()), "{socket}");
        }
    }

    #[test]
    fn docker_on_another_machine_or_an_unknown_socket_is_never_restarted() {
        for host in ["ssh://builder@build.example", "tcp://192.0.2.10:2376", "fd://"] {
            for os in [Os::Linux, Os::MacOs, Os::Windows] {
                let facts = Facts {
                    os,
                    user_unit: true,
                    system_unit: true,
                    pkexec: Some("/usr/bin/pkexec".into()),
                    desktop_cli: true,
                    ..linux(host)
                };
                let instructions = runs(&facts).expect_err(host);
                assert!(instructions.contains(host), "{os:?}: {instructions}");
            }
        }
        let mut facts = Facts {
            user_unit: true,
            system_unit: true,
            pkexec: Some("/usr/bin/pkexec".into()),
            ..linux("unix:///srv/podman/podman.sock")
        };
        assert_eq!(runs(&facts), Err(UNKNOWN.into()));
        facts.endpoint = None;
        assert_eq!(runs(&facts), Err(NO_ENDPOINT.into()));
    }

    #[test]
    fn docker_desktop_restarts_through_its_cli_on_every_system() {
        let cli = Ok(DESKTOP_CLI.join(" "));
        let mut facts = linux("unix:///home/person/.docker/desktop/docker.sock");
        facts.user_unit = true;
        assert_eq!(runs(&facts), Err(DESKTOP_MENU.into()));
        facts.desktop_cli = true;
        assert_eq!(runs(&facts), cli);

        let mut facts = Facts {
            os: Os::Windows,
            endpoint: Some(Endpoint::parse("npipe:////./pipe/dockerDesktopLinuxEngine")),
            ..Facts::default()
        };
        assert_eq!(runs(&facts), Err(WINDOWS_DESKTOP.into()));
        facts.desktop_cli = true;
        assert_eq!(runs(&facts), cli);
        // The generic engine pipe may be the Windows service, even beside Docker Desktop's CLI.
        facts.endpoint = Some(Endpoint::parse("npipe:////./pipe/docker_engine"));
        assert_eq!(runs(&facts), Err(WINDOWS_OTHER.into()));
        assert_eq!(plan(&facts).command(), Some("Restart-Service docker"));
    }

    #[test]
    fn docker_desktop_on_macos_is_found_by_its_socket_or_context() {
        let mut facts = Facts {
            os: Os::MacOs,
            endpoint: Some(Endpoint::Socket("/Users/person/.docker/run/docker.sock".into())),
            home: Some("/Users/person".into()),
            ..Facts::default()
        };
        assert_eq!(runs(&facts), Err(DESKTOP_MENU.into()));
        facts.desktop_cli = true;
        assert_eq!(runs(&facts), Ok(DESKTOP_CLI.join(" ")));
        let linked = "/Users/person/Library/Containers/com.docker.docker/Data/docker-cli.sock";
        facts.endpoint = Some(Endpoint::Socket(linked.into()));
        assert_eq!(runs(&facts), Ok(DESKTOP_CLI.join(" ")), "the target of the socket link");
        // Colima's socket is not Docker Desktop's, whatever is installed; its context is.
        facts.endpoint = Some(Endpoint::Socket("/Users/person/.colima/default/docker.sock".into()));
        assert_eq!(runs(&facts), Err(MAC_OTHER.into()));
        facts.context = Some(DESKTOP_CONTEXT.into());
        assert_eq!(runs(&facts), Ok(DESKTOP_CLI.join(" ")));
        // The restart reaches Docker Desktop with the configuration the probe used.
        facts.docker = ["/usr/local/bin/docker", "--config", "/state/docker"]
            .map(String::from)
            .into();
        let config = "/usr/local/bin/docker --config /state/docker desktop restart";
        assert_eq!(runs(&facts).as_deref(), Ok(config));
        assert_eq!(
            plan(&facts).command(),
            Some("docker desktop restart"),
            "a person types plain docker"
        );
    }
}
