mod config_parser;
mod connection_status;
mod machine_keys;

use serde::{Deserialize, Serialize};

pub use config_parser::{DiscoveredSshHost, discover_ssh_hosts};
pub use connection_status::SshConnectionStatus;
pub use machine_keys::{MachineKey, MachineKeyError, MachineKeyRef, MachineKeys};

use horizon_browser_control::manifest::block_terminal::CheckedTerminal;

/// How long the connect of any ssh child of Horizon may take, in seconds.
///
/// `ServerAliveInterval` only covers a link whose ssh protocol is up. During the
/// TCP connect and the key exchange the system default decides, and it is about
/// two minutes on Linux and 75 seconds on macOS; the delays of the reconnect of a
/// lost link start at three seconds, so an attempt into a link that is gone must
/// end long before that.
const CONNECT_TIMEOUT_SECS: u16 = 10;

/// How often an open connection of a machine terminal asks the machine to answer,
/// in seconds.
const MACHINE_SERVER_ALIVE_INTERVAL_SECS: u16 = 5;

/// How many of those questions may stay unanswered before the connection ends.
const MACHINE_SERVER_ALIVE_COUNT_MAX: u16 = 3;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct SshConnection {
    pub host: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    pub remote_command: Option<String>,
    pub extra_args: Vec<String>,
    /// The machine key that Horizon keeps for this panel, when an agent opened
    /// it with one. It is Horizon's own pair under the Horizon home, so the
    /// person's own keys and ssh agent are not offered to the machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_key: Option<MachineKeyRef>,
    /// Whether this terminal is one of a machine of the machine list, so it
    /// holds the time bounds of a machine terminal. A click on a machine row, an
    /// agent's `open_terminal` of that machine and the reconnect of that panel all
    /// set it, and it is saved, so the attempt after a lost link is bounded the
    /// same way.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub machine_terminal: bool,
}

impl SshConnection {
    /// The connection of a terminal that an agent asked Horizon to open with
    /// the `open_terminal` operation of `block_panel`.
    /// Only the three checked route fields are used, and `-t` is added when a
    /// remote command needs a terminal. An agent supplies no option, no
    /// identity file, no proxy and no environment, so this is the whole input
    /// of the command line below.
    #[must_use]
    pub fn for_agent_terminal(terminal: &CheckedTerminal) -> Self {
        Self {
            host: terminal.host.clone(),
            port: terminal.port,
            user: terminal.user.clone(),
            remote_command: terminal.remote_command.clone(),
            extra_args: if terminal.needs_tty() {
                vec!["-t".to_string()]
            } else {
                Vec::new()
            },
            ..Self::default()
        }
    }

    /// The connection of a terminal that connects with one machine key of the
    /// caller's workspace. Horizon adds that key's own options to the command
    /// line: the key file, `IdentitiesOnly`, no ssh agent, the known-hosts file
    /// of this key, and `accept-new` so first contact is pinned for that key.
    #[must_use]
    pub fn for_agent_terminal_with_key(terminal: &CheckedTerminal, key: &MachineKeyRef) -> Self {
        let mut connection = Self::for_agent_terminal(terminal);
        connection.machine_key = Some(key.clone());
        connection
    }

    /// The connection as one of a machine of the machine list.
    ///
    /// A machine terminal holds the bounds of a machine: an attempt that the
    /// network drops ends in ten seconds, and an attempt whose machine stops
    /// answering ends after fifteen. Without them one attempt of a dead network
    /// hung in the connect for two minutes, so the delays of the reconnect of a
    /// lost link never ran.
    #[must_use]
    pub fn as_machine_terminal(mut self) -> Self {
        self.machine_terminal = true;
        self
    }

    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.host.trim().is_empty()
    }

    #[must_use]
    pub fn display_label(&self) -> String {
        self.user
            .as_deref()
            .filter(|user| !user.trim().is_empty())
            .map_or_else(|| self.host.clone(), |user| format!("{user}@{}", self.host))
    }

    #[must_use]
    pub fn to_command_args(&self) -> Vec<String> {
        let mut args = self.base_transport_args(false, CONNECT_TIMEOUT_SECS);
        args.push(self.transport_target());

        if let Some(remote_command) = non_empty(self.remote_command.as_deref()) {
            args.push(remote_command.to_string());
        }

        args
    }

    #[must_use]
    pub fn ssh_transport_args(&self) -> Vec<String> {
        let mut args = self.base_transport_args(true, CONNECT_TIMEOUT_SECS);
        args.push(self.transport_target());
        args
    }

    #[must_use]
    pub fn ssh_probe_transport_args(&self, connect_timeout_secs: u16) -> Vec<String> {
        let mut args = self.base_transport_args(true, connect_timeout_secs);
        args.push(self.transport_target());
        args
    }

    /// Arguments for `ssh -W`, which relays this process's stdio to
    /// `remote_endpoint` as seen from the SSH host. `-W` runs no remote
    /// command and allocates no TTY. Batch mode fails instead of prompting,
    /// and host-key verification is left to OpenSSH: an unknown or changed
    /// key refuses the forward, so a viewer that connects on its own never
    /// trusts a first-contact key. The key is trusted the usual way first,
    /// for example by opening the host over SSH once.
    ///
    /// The connect and the key exchange hold the same bound as a machine
    /// terminal, so a link whose address is gone ends in ten seconds and the
    /// viewer tries again instead of waiting for the system default of about
    /// two minutes.
    #[must_use]
    pub fn stdio_forward_args(&self, remote_endpoint: &str) -> Vec<String> {
        let mut args = vec!["-W".to_string(), remote_endpoint.to_string()];
        args.extend(self.ssh_transport_args());
        args
    }

    #[must_use]
    pub fn transport_target(&self) -> String {
        self.display_label()
    }

    fn base_transport_args(&self, batch_mode: bool, connect_timeout_secs: u16) -> Vec<String> {
        let mut args = Vec::new();

        if let Some(port) = self.port {
            args.extend(["-p".to_string(), port.to_string()]);
        }
        if let Some(identity_file) = self.identity_file.as_deref() {
            args.extend(["-i".to_string(), expand_tilde(identity_file)]);
        }
        if let Some(proxy_jump) = non_empty(self.proxy_jump.as_deref()) {
            args.extend(["-J".to_string(), proxy_jump.to_string()]);
        }
        if batch_mode {
            args.extend(["-o".to_string(), "BatchMode=yes".to_string()]);
        }
        if let Some(key) = self.machine_key.as_ref() {
            args.extend(key.ssh_option_args());
        }

        args.extend(keepalive_args(self.machine_terminal, connect_timeout_secs));
        args.extend(self.extra_args.iter().cloned());
        args
    }
}

/// The keepalive and connect options of one connection, as command-line options.
///
/// `ssh` reads the first value of an option that it meets, and a value of the
/// command line comes before the value of a configuration file, so these options
/// decide for the connection that Horizon starts, also when the ssh config of the
/// person names another value. A machine terminal is the case that needs them
/// most: a network that drops packets keeps a connect open until the system ends
/// it, and a person who presses a key in the panel then has nothing to hurry.
fn keepalive_args(machine_terminal: bool, connect_timeout_secs: u16) -> Vec<String> {
    if machine_terminal {
        return vec![
            "-o".to_string(),
            format!("ConnectTimeout={connect_timeout_secs}"),
            "-o".to_string(),
            format!("ServerAliveInterval={MACHINE_SERVER_ALIVE_INTERVAL_SECS}"),
            "-o".to_string(),
            format!("ServerAliveCountMax={MACHINE_SERVER_ALIVE_COUNT_MAX}"),
        ];
    }
    default_keepalive_args(connect_timeout_secs)
}

/// The options of a connection that is not a machine terminal: a plain SSH panel,
/// the terminal that an agent opens, the upload and the `-W` forward of a device.
/// None of them may wait for the system default of a connect.
fn default_keepalive_args(connect_timeout_secs: u16) -> Vec<String> {
    vec![
        "-o".to_string(),
        format!("ConnectTimeout={connect_timeout_secs}"),
        "-o".to_string(),
        "ServerAliveInterval=15".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=1".to_string(),
    ]
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.and_then(|item| {
        let trimmed = item.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    })
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        let home = std::path::PathBuf::from(home);
        horizon_browser_control::paths::refuse_real_home(&home);
        return home.join(rest).display().to_string();
    }

    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::SshConnection;
    use crate::ssh::MachineKeyRef;
    use horizon_browser_control::manifest::block_terminal::{self, TerminalRoute};

    fn key() -> MachineKeyRef {
        MachineKeyRef {
            directory: "/home/peters/.horizon/machine-keys/w666f6c64657231".to_string(),
            name: "build-1".to_string(),
        }
    }

    fn agent_terminal(host: &str, user: Option<&str>, port: Option<i64>, command: Option<&str>) -> Vec<String> {
        let route = TerminalRoute {
            host: host.into(),
            user: user.map(str::to_string),
            port,
        };
        let checked = block_terminal::check(&route, command).expect("the route and command are accepted");
        SshConnection::for_agent_terminal(&checked).to_command_args()
    }

    #[test]
    fn an_agent_terminal_with_a_machine_key_offers_that_key_alone() {
        let route = TerminalRoute {
            host: "203.0.113.10".into(),
            user: Some("root".into()),
            port: None,
        };
        let checked = block_terminal::check(&route, Some("tmux new -A -s build")).expect("the route is accepted");
        let connection = SshConnection::for_agent_terminal_with_key(&checked, &key());
        assert_eq!(
            connection.to_command_args(),
            vec![
                "-i".to_string(),
                "/home/peters/.horizon/machine-keys/w666f6c64657231/build-1".to_string(),
                "-o".to_string(),
                "IdentitiesOnly=yes".to_string(),
                "-o".to_string(),
                "IdentityAgent=none".to_string(),
                "-o".to_string(),
                "UserKnownHostsFile=/home/peters/.horizon/machine-keys/w666f6c64657231/build-1.known_hosts".to_string(),
                "-o".to_string(),
                "StrictHostKeyChecking=accept-new".to_string(),
                "-o".to_string(),
                "ConnectTimeout=10".to_string(),
                "-o".to_string(),
                "ServerAliveInterval=15".to_string(),
                "-o".to_string(),
                "ServerAliveCountMax=1".to_string(),
                "-t".to_string(),
                "root@203.0.113.10".to_string(),
                "tmux new -A -s build".to_string(),
            ],
            "the key options come before the destination and the command stays last"
        );
        assert_eq!(
            connection.identity_file, None,
            "the key is not also carried as an identity file"
        );
        assert_eq!(
            connection.machine_key.as_ref().map(|key| key.name.as_str()),
            Some("build-1"),
            "the panel carries the key name"
        );
    }

    #[test]
    fn a_terminal_without_a_key_is_the_command_line_of_today() {
        let route = TerminalRoute {
            host: "203.0.113.10".into(),
            user: Some("root".into()),
            port: None,
        };
        let checked = block_terminal::check(&route, Some("tmux new -A -s build")).expect("the route is accepted");
        let with_key = SshConnection::for_agent_terminal_with_key(&checked, &key());
        let without_key = SshConnection::for_agent_terminal(&checked);
        assert!(
            !without_key.to_command_args().iter().any(|argument| argument == "-i"),
            "no key, no identity option: {:?}",
            without_key.to_command_args()
        );
        assert!(
            with_key.to_command_args().len() > without_key.to_command_args().len(),
            "a key adds its own options"
        );
    }

    #[test]
    fn a_saved_panel_keeps_its_machine_key_and_an_older_one_still_parses() {
        let saved = serde_json::to_value(SshConnection {
            host: "lab-1".into(),
            machine_key: Some(key()),
            ..SshConnection::default()
        })
        .expect("serialize");
        assert_eq!(saved["machine_key"]["name"], serde_json::json!("build-1"));
        assert_eq!(
            saved["machine_key"]["directory"],
            serde_json::json!("/home/peters/.horizon/machine-keys/w666f6c64657231"),
            "the directory comes with the name, so a restored panel builds the same line"
        );
        let restored: SshConnection = serde_json::from_value(saved).expect("parse a saved connection");
        assert_eq!(restored.machine_key, Some(key()));
        let restored_args = restored.to_command_args();

        let older: SshConnection =
            serde_json::from_value(serde_json::json!({"host": "lab-1"})).expect("an older panel keeps parsing");
        assert_eq!(older.machine_key, None, "a panel of an older snapshot has no key");
        assert!(
            !restored_args.is_empty() && older.to_command_args().len() < restored_args.len(),
            "the restored panel builds the command line of its key"
        );
    }

    #[test]
    fn an_agent_terminal_runs_exactly_the_route_and_its_command() {
        assert_eq!(
            agent_terminal("lab-1", None, None, None),
            vec![
                "-o".to_string(),
                "ConnectTimeout=10".to_string(),
                "-o".to_string(),
                "ServerAliveInterval=15".to_string(),
                "-o".to_string(),
                "ServerAliveCountMax=1".to_string(),
                "lab-1".to_string(),
            ]
        );
        assert_eq!(
            agent_terminal("lab-1", Some("deploy"), Some(2222), None),
            vec![
                "-p".to_string(),
                "2222".to_string(),
                "-o".to_string(),
                "ConnectTimeout=10".to_string(),
                "-o".to_string(),
                "ServerAliveInterval=15".to_string(),
                "-o".to_string(),
                "ServerAliveCountMax=1".to_string(),
                "deploy@lab-1".to_string(),
            ],
            "the target is one argument, so no field can split into another"
        );
        let args = agent_terminal("lab-1", Some("deploy"), Some(2222), Some("tmux new -A -s build"));
        assert!(
            args.iter().any(|argument| argument == "-t"),
            "a remote command needs a terminal: {args:?}"
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some("tmux new -A -s build"),
            "the command is the last argument and stays one argument: {args:?}"
        );
        assert_eq!(
            args.iter().position(|argument| argument == "-t"),
            Some(args.len() - 3),
            "-t is an option of this connection, before the target: {args:?}"
        );
        assert_eq!(
            args.iter().filter(|argument| argument.starts_with("-o")).count(),
            3,
            "an agent adds no ssh option of its own: {args:?}"
        );
    }

    #[test]
    fn a_machine_terminal_bounds_its_attempt_with_its_own_options() {
        let route = TerminalRoute {
            host: "203.0.113.10".into(),
            user: Some("root".into()),
            port: None,
        };
        let checked = block_terminal::check(&route, Some("tmux new-session -A -s work")).expect("the route");
        let plain = SshConnection::for_agent_terminal(&checked).to_command_args();
        let machine = SshConnection::for_agent_terminal(&checked).as_machine_terminal();
        let args = machine.to_command_args();

        // The command line decides over the ssh config of the person: ssh reads
        // the first value of an option that it meets, and these come first.
        assert_eq!(
            args,
            vec![
                "-o".to_string(),
                "ConnectTimeout=10".to_string(),
                "-o".to_string(),
                "ServerAliveInterval=5".to_string(),
                "-o".to_string(),
                "ServerAliveCountMax=3".to_string(),
                "-t".to_string(),
                "root@203.0.113.10".to_string(),
                "tmux new-session -A -s work".to_string(),
            ],
            "a machine attempt ends in ten seconds and an open one in fifteen"
        );
        assert!(
            plain.iter().any(|argument| argument == "ConnectTimeout=10")
                && !plain.iter().any(|argument| argument == "ServerAliveCountMax=3"),
            "a plain ssh panel bounds its connect too, with its own keepalive: {plain:?}"
        );
        assert!(
            args.iter().position(|argument| argument == "ConnectTimeout=10")
                < args.iter().position(|argument| argument == "root@203.0.113.10"),
            "the options come before the destination: {args:?}"
        );

        // The saved panel keeps the machine bounds, so the attempt after a lost
        // link is bounded the same way.
        let saved = serde_json::to_value(&machine).expect("a saved connection");
        assert_eq!(saved["machine_terminal"], serde_json::json!(true));
        let restored: SshConnection = serde_json::from_value(saved).expect("a restored connection");
        assert_eq!(restored.to_command_args(), args);
        let older: SshConnection =
            serde_json::from_value(serde_json::json!({"host": "lab-1"})).expect("a connection of an older snapshot");
        assert!(
            !older.machine_terminal,
            "a plain ssh panel of an older snapshot stays plain"
        );
    }

    #[test]
    fn display_label_prefers_user_when_available() {
        let connection = SshConnection {
            host: "prod".to_string(),
            user: Some("deploy".to_string()),
            ..SshConnection::default()
        };

        assert_eq!(connection.display_label(), "deploy@prod");
    }

    #[test]
    fn to_command_args_includes_structured_ssh_options() {
        let connection = SshConnection {
            host: "prod".to_string(),
            port: Some(2222),
            user: Some("deploy".to_string()),
            identity_file: Some("/tmp/id_ed25519".to_string()),
            proxy_jump: Some("bastion".to_string()),
            remote_command: Some("tmux attach".to_string()),
            extra_args: vec!["-o".to_string(), "StrictHostKeyChecking=no".to_string()],
            ..SshConnection::default()
        };

        assert_eq!(
            connection.to_command_args(),
            vec![
                "-p".to_string(),
                "2222".to_string(),
                "-i".to_string(),
                "/tmp/id_ed25519".to_string(),
                "-J".to_string(),
                "bastion".to_string(),
                "-o".to_string(),
                "ConnectTimeout=10".to_string(),
                "-o".to_string(),
                "ServerAliveInterval=15".to_string(),
                "-o".to_string(),
                "ServerAliveCountMax=1".to_string(),
                "-o".to_string(),
                "StrictHostKeyChecking=no".to_string(),
                "deploy@prod".to_string(),
                "tmux attach".to_string(),
            ]
        );
    }

    #[test]
    fn stdio_forward_args_relay_the_remote_endpoint_in_batch_mode() {
        let connection = SshConnection {
            host: "lab".to_string(),
            port: Some(2222),
            user: Some("deploy".to_string()),
            remote_command: Some("tmux attach".to_string()),
            extra_args: vec!["-o".to_string(), "StrictHostKeyChecking=yes".to_string()],
            ..SshConnection::default()
        };

        let args = connection.stdio_forward_args("127.0.0.1:5900");
        assert_eq!(
            args,
            vec![
                "-W".to_string(),
                "127.0.0.1:5900".to_string(),
                "-p".to_string(),
                "2222".to_string(),
                "-o".to_string(),
                "BatchMode=yes".to_string(),
                "-o".to_string(),
                "ConnectTimeout=10".to_string(),
                "-o".to_string(),
                "ServerAliveInterval=15".to_string(),
                "-o".to_string(),
                "ServerAliveCountMax=1".to_string(),
                "-o".to_string(),
                "StrictHostKeyChecking=yes".to_string(),
                "deploy@lab".to_string(),
            ],
            "the remote command never runs behind a stdio forward"
        );
        assert!(
            !args.iter().any(|arg| arg.contains("accept-new")),
            "a viewer that connects on its own must not trust first-contact keys"
        );
    }

    #[test]
    fn to_command_args_places_user_extra_args_after_keepalive_defaults() {
        let connection = SshConnection {
            host: "prod".to_string(),
            extra_args: vec![
                "-o".to_string(),
                "ServerAliveInterval=60".to_string(),
                "-o".to_string(),
                "ServerAliveCountMax=4".to_string(),
            ],
            ..SshConnection::default()
        };

        assert_eq!(
            connection.to_command_args(),
            vec![
                "-o".to_string(),
                "ConnectTimeout=10".to_string(),
                "-o".to_string(),
                "ServerAliveInterval=15".to_string(),
                "-o".to_string(),
                "ServerAliveCountMax=1".to_string(),
                "-o".to_string(),
                "ServerAliveInterval=60".to_string(),
                "-o".to_string(),
                "ServerAliveCountMax=4".to_string(),
                "prod".to_string(),
            ]
        );
    }

    #[test]
    fn ssh_transport_args_force_batch_mode_without_remote_command() {
        let connection = SshConnection {
            host: "prod".to_string(),
            user: Some("deploy".to_string()),
            remote_command: Some("tmux attach".to_string()),
            ..SshConnection::default()
        };

        assert_eq!(
            connection.ssh_transport_args(),
            vec![
                "-o".to_string(),
                "BatchMode=yes".to_string(),
                "-o".to_string(),
                "ConnectTimeout=10".to_string(),
                "-o".to_string(),
                "ServerAliveInterval=15".to_string(),
                "-o".to_string(),
                "ServerAliveCountMax=1".to_string(),
                "deploy@prod".to_string(),
            ]
        );
    }

    #[test]
    fn ssh_probe_transport_args_add_connect_timeout_before_target() {
        let connection = SshConnection {
            host: "prod".to_string(),
            ..SshConnection::default()
        };

        assert_eq!(
            connection.ssh_probe_transport_args(5),
            vec![
                "-o".to_string(),
                "BatchMode=yes".to_string(),
                "-o".to_string(),
                "ConnectTimeout=5".to_string(),
                "-o".to_string(),
                "ServerAliveInterval=15".to_string(),
                "-o".to_string(),
                "ServerAliveCountMax=1".to_string(),
                "prod".to_string(),
            ],
            "the bound of the probe replaces the default one: ssh reads the first value"
        );
    }

    /// B6: every kind of ssh child that Horizon starts names a connect bound, so an
    /// attempt into a link that is gone ends in ten seconds instead of the two
    /// minutes of the system default. The four kinds are the plain panel, the
    /// terminal that an agent opens for a host outside the machine list, the upload
    /// and the `-W` forward of a device viewer.
    /// B13: multiplexing is a decision of the person. At a round trip of a third
    /// of a second a handshake costs seconds, so five panels cost five of them
    /// unless the person shares one connection. An `ssh` child of Horizon that
    /// named `ControlMaster`, `ControlPath` or `-S` would decide for all of the
    /// person's sessions, and `-S none` would switch a master of the person off
    /// for one panel. This test names the arguments of every child instead, so no
    /// later option list takes the choice away.
    #[test]
    fn no_ssh_child_of_horizon_switches_the_multiplexing_of_the_person_off() {
        let route = TerminalRoute {
            host: "lab-1".into(),
            user: Some("deploy".into()),
            port: None,
        };
        let checked = block_terminal::check(&route, None).expect("the route is accepted");
        let plain_panel = SshConnection {
            host: "lab-1".to_string(),
            remote_command: Some("tmux new -A -s work".to_string()),
            ..SshConnection::default()
        };
        let with_person_options = SshConnection {
            extra_args: vec![
                "-o".to_string(),
                "ControlMaster=auto".to_string(),
                "-o".to_string(),
                "ControlPath=~/.ssh/horizon-%C".to_string(),
            ],
            ..plain_panel.clone()
        };
        let children = [
            ("the plain SSH panel", plain_panel.to_command_args()),
            (
                "the terminal of an agent",
                SshConnection::for_agent_terminal(&checked).to_command_args(),
            ),
            ("the upload", plain_panel.ssh_transport_args()),
            ("the upload probe", plain_panel.ssh_probe_transport_args(5)),
            (
                "the stdio forward of a device",
                plain_panel.stdio_forward_args("127.0.0.1:5900"),
            ),
        ];
        for (kind, args) in children {
            for argument in &args {
                let lower = argument.to_ascii_lowercase();
                assert!(
                    !lower.contains("controlmaster")
                        && !lower.contains("controlpath")
                        && !lower.contains("controlpersist"),
                    "{kind} names no multiplexing option of its own: {args:?}"
                );
            }
            assert!(
                !args.iter().any(|argument| argument == "-S"),
                "{kind} passes no -S master path: {args:?}"
            );
        }
        // The options of the person come after the options of Horizon, so a panel
        // uses the master that they set. Every multiplexing word of the line is
        // theirs and none is Horizon's.
        let person_options = ["ControlMaster=auto", "ControlPath=~/.ssh/horizon-%C"];
        let with_person = with_person_options.to_command_args();
        assert!(
            with_person.iter().any(|argument| argument == person_options[0]),
            "an option of the person still reaches the child: {with_person:?}"
        );
        for argument in &with_person {
            let lower = argument.to_ascii_lowercase();
            if lower.contains("controlmaster") || lower.contains("controlpath") || lower.contains("controlpersist") {
                assert!(
                    person_options.contains(&argument.as_str()),
                    "Horizon adds no multiplexing option of its own: {argument}"
                );
            }
        }
    }

    #[test]
    fn every_ssh_child_of_horizon_bounds_its_connect() {
        let route = TerminalRoute {
            host: "lab-1".into(),
            user: Some("deploy".into()),
            port: None,
        };
        let checked = block_terminal::check(&route, None).expect("the route is accepted");
        let plain_panel = SshConnection {
            host: "lab-1".to_string(),
            remote_command: Some("tmux new -A -s work".to_string()),
            ..SshConnection::default()
        };
        let upload = plain_panel.clone();
        let forward = plain_panel.clone();
        let children = [
            (
                "the plain SSH panel",
                plain_panel.to_command_args(),
                "ConnectTimeout=10",
            ),
            (
                "the terminal of an agent",
                SshConnection::for_agent_terminal(&checked).to_command_args(),
                "ConnectTimeout=10",
            ),
            ("the upload", upload.ssh_transport_args(), "ConnectTimeout=10"),
            (
                "the upload probe",
                upload.ssh_probe_transport_args(5),
                "ConnectTimeout=5",
            ),
            (
                "the stdio forward of a device",
                forward.stdio_forward_args("127.0.0.1:5900"),
                "ConnectTimeout=10",
            ),
        ];
        for (kind, args, expected) in children {
            let bound = args
                .iter()
                .position(|argument| argument.starts_with("ConnectTimeout="))
                .map(|at| args[at].clone());
            assert_eq!(
                bound.as_deref(),
                Some(expected),
                "{kind} names its connect bound: {args:?}"
            );
            assert_eq!(
                args.iter()
                    .filter(|argument| argument.starts_with("ConnectTimeout="))
                    .count(),
                1,
                "{kind} names one connect bound, so the first value is the whole rule: {args:?}"
            );
        }
    }
}
