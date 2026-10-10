//! How the Docker that Horizon builds with can be restarted, decided from what Horizon
//! observed rather than guessed. When the observations do not show one known setup,
//! the person gets instructions and Horizon runs nothing.
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
    Other,
}

impl Os {
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Other
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    pub os: Os,
    /// `None` when the Docker CLI could not say which daemon it uses.
    pub endpoint: Option<Endpoint>,
    /// The name of the Docker context in use, such as `rootless` or `desktop-linux`.
    pub context: Option<String>,
    /// `$XDG_RUNTIME_DIR`, where rootless Docker keeps its socket.
    pub runtime_dir: Option<PathBuf>,
    pub home: Option<PathBuf>,
    /// systemd loaded the user unit `docker.service`, which runs rootless Docker.
    pub user_unit: bool,
    /// systemd loaded the system unit `docker.service`.
    pub system_unit: bool,
    /// systemd loaded the user unit `docker-desktop.service` of Docker Desktop for Linux.
    pub desktop_unit: bool,
    /// `pkexec` is on `PATH` to ask for administrator rights.
    pub pkexec: bool,
    /// `docker desktop version` answered: Docker Desktop 4.37 or later with its CLI.
    pub desktop_cli: bool,
    /// macOS: `/Applications/Docker.app` exists.
    pub desktop_app: bool,
}

impl Facts {
    /// Facts that show nothing: no endpoint, no units, no tools.
    #[must_use]
    pub const fn none(os: Os) -> Self {
        Self {
            os,
            endpoint: None,
            context: None,
            runtime_dir: None,
            home: None,
            user_unit: false,
            system_unit: false,
            desktop_unit: false,
            pkexec: false,
            desktop_cli: false,
            desktop_app: false,
        }
    }
}

/// Where the Docker runs, as far as the facts show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setup {
    /// Rootless Docker of this user, run by a systemd user unit.
    Rootless,
    /// Docker as a system service, which needs administrator rights to restart.
    Service,
    Desktop,
    /// Docker on another machine.
    Remote,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Horizon restarts Docker by running `steps` in order once the person confirms.
    /// `shown` says what runs; `command` is what a person can type instead.
    Restart {
        setup: Setup,
        steps: Vec<Vec<String>>,
        shown: String,
        command: Option<String>,
    },
    /// Horizon runs nothing: the person restarts Docker as `instructions` say,
    /// with `command` to copy when there is one.
    Manual {
        setup: Setup,
        instructions: String,
        command: Option<String>,
    },
}

impl Plan {
    #[must_use]
    pub const fn setup(&self) -> Setup {
        match self {
            Self::Restart { setup, .. } | Self::Manual { setup, .. } => *setup,
        }
    }

    /// The command a person can run themselves, if Horizon knows one.
    #[must_use]
    pub fn command(&self) -> Option<&str> {
        match self {
            Self::Restart { command, .. } | Self::Manual { command, .. } => command.as_deref(),
        }
    }
}

const DESKTOP_CONTEXT: &str = "desktop-linux";
const DESKTOP_CLI: &str = "docker desktop restart";
const SERVICE_COMMAND: &str = "sudo systemctl restart docker";
/// Quits Docker Desktop, waits until it has quit, then opens it again.
const MAC_REOPEN: [&str; 11] = [
    "osascript",
    "-e",
    "quit app \"Docker\"",
    "-e",
    "repeat while application \"Docker\" is running",
    "-e",
    "delay 1",
    "-e",
    "end repeat",
    "-e",
    "tell application \"Docker\" to activate",
];

/// A restart a person could also type as `command`.
fn restart(setup: Setup, argv: &[&str], command: &str) -> Plan {
    Plan::Restart {
        setup,
        steps: vec![argv.iter().map(|&part| part.to_owned()).collect()],
        shown: command.to_owned(),
        command: Some(command.to_owned()),
    }
}

fn manual(setup: Setup, instructions: impl Into<String>, command: Option<&str>) -> Plan {
    Plan::Manual {
        setup,
        instructions: instructions.into(),
        command: command.map(str::to_owned),
    }
}

/// How to restart the Docker that `facts` describe.
#[must_use]
pub fn plan(facts: &Facts) -> Plan {
    match &facts.endpoint {
        None => manual(
            Setup::Unknown,
            "Horizon could not find out where this Docker runs. Restart Docker the way it was started on this computer, then retry.",
            None,
        ),
        Some(Endpoint::Remote(host)) => manual(
            Setup::Remote,
            format!(
                "This Docker runs at {host}, not on this computer, so Horizon does not restart it. Restart Docker on that machine, then retry."
            ),
            None,
        ),
        Some(Endpoint::Socket(path)) => match facts.os {
            Os::Linux => linux(facts, path),
            Os::MacOs => mac(facts, path),
            Os::Windows | Os::Other => unknown(),
        },
        Some(Endpoint::Pipe(pipe)) if facts.os == Os::Windows => windows(facts, pipe),
        Some(Endpoint::Pipe(_)) => unknown(),
    }
}

fn unknown() -> Plan {
    manual(
        Setup::Unknown,
        "Horizon does not know how this Docker was started. Restart it the way it was started on this computer, then retry.",
        None,
    )
}

fn linux(facts: &Facts, socket: &Path) -> Plan {
    let rootless = facts
        .runtime_dir
        .as_deref()
        .is_some_and(|directory| socket == directory.join("docker.sock"));
    if rootless {
        return if facts.user_unit {
            restart(
                Setup::Rootless,
                &["systemctl", "--user", "restart", "docker.service"],
                "systemctl --user restart docker",
            )
        } else {
            manual(
                Setup::Rootless,
                "This is rootless Docker, but no systemd user unit runs it. Stop dockerd and start it again the way it was started, for example with dockerd-rootless.sh.",
                None,
            )
        };
    }
    if is_desktop(facts, socket, ".docker/desktop/docker.sock") {
        return if facts.desktop_cli {
            restart(Setup::Desktop, &["docker", "desktop", "restart"], DESKTOP_CLI)
        } else if facts.desktop_unit {
            restart(
                Setup::Desktop,
                &["systemctl", "--user", "restart", "docker-desktop.service"],
                "systemctl --user restart docker-desktop",
            )
        } else {
            manual(
                Setup::Desktop,
                "Restart Docker Desktop from its menu, then retry.",
                None,
            )
        };
    }
    let system = [Path::new("/var/run/docker.sock"), Path::new("/run/docker.sock")].contains(&socket);
    if system && facts.system_unit {
        return if facts.pkexec {
            Plan::Restart {
                setup: Setup::Service,
                steps: vec![
                    ["pkexec", "systemctl", "restart", "docker.service"]
                        .map(str::to_owned)
                        .to_vec(),
                ],
                shown: "pkexec systemctl restart docker".to_owned(),
                command: Some(SERVICE_COMMAND.to_owned()),
            }
        } else {
            manual(
                Setup::Service,
                "Docker runs as a system service, and restarting it needs administrator rights. Run this command in a terminal, then retry.",
                Some(SERVICE_COMMAND),
            )
        };
    }
    unknown()
}

fn mac(facts: &Facts, socket: &Path) -> Plan {
    if !is_desktop(facts, socket, ".docker/run/docker.sock") {
        return manual(
            Setup::Unknown,
            "This Docker is not Docker Desktop. Restart the program that runs it, such as Colima or OrbStack, then retry.",
            None,
        );
    }
    if facts.desktop_cli {
        restart(Setup::Desktop, &["docker", "desktop", "restart"], DESKTOP_CLI)
    } else if facts.desktop_app {
        Plan::Restart {
            setup: Setup::Desktop,
            steps: vec![MAC_REOPEN.map(str::to_owned).to_vec()],
            shown: "quit Docker Desktop, wait until it has quit, then open it again".to_owned(),
            command: None,
        }
    } else {
        manual(
            Setup::Desktop,
            "Quit Docker Desktop from its menu bar icon, open it again, then retry.",
            None,
        )
    }
}

fn windows(facts: &Facts, pipe: &str) -> Plan {
    let desktop = facts.context.as_deref() == Some(DESKTOP_CONTEXT) || pipe.ends_with("dockerDesktopLinuxEngine");
    if facts.desktop_cli && (desktop || pipe.ends_with("docker_engine")) {
        return restart(Setup::Desktop, &["docker", "desktop", "restart"], DESKTOP_CLI);
    }
    if desktop {
        return manual(
            Setup::Desktop,
            "Right-click the Docker Desktop icon in the notification area, choose Quit Docker Desktop, start Docker Desktop again, then retry.",
            None,
        );
    }
    manual(
        Setup::Unknown,
        "Restart Docker Desktop from the icon in the notification area, or, for the Docker service, run this command in an administrator PowerShell. Then retry.",
        Some("Restart-Service docker"),
    )
}

/// Docker Desktop: its context, or its socket under the home directory at `relative`.
fn is_desktop(facts: &Facts, socket: &Path, relative: &str) -> bool {
    facts.context.as_deref() == Some(DESKTOP_CONTEXT)
        || facts.home.as_deref().is_some_and(|home| socket == home.join(relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux(endpoint: &str) -> Facts {
        Facts {
            endpoint: Some(Endpoint::parse(endpoint)),
            runtime_dir: Some("/run/user/1000".into()),
            home: Some("/home/person".into()),
            ..Facts::none(Os::Linux)
        }
    }

    fn steps(plan: &Plan) -> Vec<String> {
        match plan {
            Plan::Restart { steps, .. } => steps.iter().map(|step| step.join(" ")).collect(),
            Plan::Manual { .. } => Vec::new(),
        }
    }

    #[test]
    fn rootless_docker_restarts_through_its_user_unit_only_when_systemd_runs_it() {
        let mut facts = linux("unix:///run/user/1000/docker.sock");
        facts.user_unit = true;
        // A system unit and pkexec do not turn rootless Docker into the system service.
        facts.system_unit = true;
        facts.pkexec = true;
        let found = plan(&facts);
        assert_eq!(found.setup(), Setup::Rootless);
        assert_eq!(steps(&found), ["systemctl --user restart docker.service"]);
        assert_eq!(found.command(), Some("systemctl --user restart docker"));

        facts.user_unit = false;
        let found = plan(&facts);
        assert!(
            matches!(
                found,
                Plan::Manual {
                    setup: Setup::Rootless,
                    command: None,
                    ..
                }
            ),
            "{found:?}"
        );
    }

    #[test]
    fn the_system_service_asks_pkexec_for_rights_or_shows_the_command_to_copy() {
        for socket in ["unix:///var/run/docker.sock", "unix:///run/docker.sock"] {
            let mut facts = linux(socket);
            facts.system_unit = true;
            facts.pkexec = true;
            let found = plan(&facts);
            assert_eq!(steps(&found), ["pkexec systemctl restart docker.service"], "{socket}");
            assert_eq!(found.setup(), Setup::Service);
            assert_eq!(
                found.command(),
                Some(SERVICE_COMMAND),
                "a person types sudo, not pkexec"
            );

            facts.pkexec = false;
            let found = plan(&facts);
            assert!(
                steps(&found).is_empty(),
                "{socket}: without pkexec Horizon runs nothing"
            );
            assert_eq!(found.command(), Some(SERVICE_COMMAND));
        }
        // A system socket that no systemd unit runs is not restarted.
        let mut facts = linux("unix:///var/run/docker.sock");
        facts.pkexec = true;
        assert!(matches!(
            plan(&facts),
            Plan::Manual {
                setup: Setup::Unknown,
                ..
            }
        ));
    }

    #[test]
    fn docker_on_another_machine_is_never_restarted_from_here() {
        for host in ["ssh://builder@build.example", "tcp://192.0.2.10:2376", "fd://"] {
            for os in [Os::Linux, Os::MacOs, Os::Windows] {
                let mut facts = linux(host);
                facts.os = os;
                facts.user_unit = true;
                facts.system_unit = true;
                facts.pkexec = true;
                facts.desktop_cli = true;
                facts.desktop_app = true;
                let found = plan(&facts);
                assert_eq!(found.setup(), Setup::Remote, "{host} {os:?}");
                assert!(steps(&found).is_empty(), "{host} {os:?}");
                assert!(
                    matches!(&found, Plan::Manual { instructions, .. } if instructions.contains(host)),
                    "{found:?}"
                );
            }
        }
    }

    #[test]
    fn an_unknown_socket_or_no_endpoint_runs_nothing() {
        let mut facts = linux("unix:///srv/podman/podman.sock");
        facts.user_unit = true;
        facts.system_unit = true;
        facts.pkexec = true;
        assert!(steps(&plan(&facts)).is_empty());
        facts.endpoint = None;
        assert!(matches!(
            plan(&facts),
            Plan::Manual {
                setup: Setup::Unknown,
                ..
            }
        ));
    }

    #[test]
    fn docker_desktop_for_linux_prefers_its_cli_then_its_user_unit() {
        let mut facts = linux("unix:///home/person/.docker/desktop/docker.sock");
        facts.user_unit = true;
        facts.desktop_unit = true;
        assert_eq!(
            steps(&plan(&facts)),
            ["systemctl --user restart docker-desktop.service"]
        );
        facts.desktop_cli = true;
        assert_eq!(steps(&plan(&facts)), ["docker desktop restart"]);
        facts.desktop_cli = false;
        facts.desktop_unit = false;
        assert!(matches!(
            plan(&facts),
            Plan::Manual {
                setup: Setup::Desktop,
                ..
            }
        ));
    }

    #[test]
    fn docker_desktop_on_macos_uses_its_cli_or_quits_and_reopens_the_app() {
        let mut facts = Facts {
            endpoint: Some(Endpoint::Socket("/Users/person/.docker/run/docker.sock".into())),
            home: Some("/Users/person".into()),
            ..Facts::none(Os::MacOs)
        };
        assert!(matches!(
            plan(&facts),
            Plan::Manual {
                setup: Setup::Desktop,
                ..
            }
        ));
        facts.desktop_app = true;
        let reopen = plan(&facts);
        assert_eq!(steps(&reopen), [MAC_REOPEN.join(" ")]);
        assert_eq!(reopen.command(), None, "the script is not a command to copy");
        facts.desktop_cli = true;
        assert_eq!(steps(&plan(&facts)), ["docker desktop restart"]);

        // Colima's socket is not Docker Desktop's, whatever is installed.
        facts.endpoint = Some(Endpoint::Socket("/Users/person/.colima/default/docker.sock".into()));
        assert!(matches!(
            plan(&facts),
            Plan::Manual {
                setup: Setup::Unknown,
                ..
            }
        ));
        // The desktop context is Docker Desktop's, wherever its socket is.
        facts.context = Some(DESKTOP_CONTEXT.into());
        assert_eq!(steps(&plan(&facts)), ["docker desktop restart"]);
    }

    #[test]
    fn docker_desktop_on_windows_restarts_only_through_its_cli() {
        let mut facts = Facts {
            endpoint: Some(Endpoint::parse("npipe:////./pipe/dockerDesktopLinuxEngine")),
            ..Facts::none(Os::Windows)
        };
        assert!(matches!(
            plan(&facts),
            Plan::Manual {
                setup: Setup::Desktop,
                command: None,
                ..
            }
        ));
        facts.desktop_cli = true;
        assert_eq!(steps(&plan(&facts)), ["docker desktop restart"]);

        // The engine pipe without Docker Desktop's CLI may be the Windows service.
        facts.endpoint = Some(Endpoint::parse("npipe:////./pipe/docker_engine"));
        facts.desktop_cli = false;
        let found = plan(&facts);
        assert!(steps(&found).is_empty());
        assert_eq!(found.command(), Some("Restart-Service docker"));
    }

    #[test]
    fn endpoints_parse_by_scheme() {
        assert_eq!(
            Endpoint::parse(" unix:///run/user/1000/docker.sock\n"),
            Endpoint::Socket("/run/user/1000/docker.sock".into())
        );
        assert_eq!(
            Endpoint::parse("npipe:////./pipe/docker_engine"),
            Endpoint::Pipe("//./pipe/docker_engine".into())
        );
        assert_eq!(
            Endpoint::parse("ssh://build.example"),
            Endpoint::Remote("ssh://build.example".into())
        );
    }
}
