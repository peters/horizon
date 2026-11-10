mod config_parser;
mod connection_status;
mod machine_keys;

use serde::{Deserialize, Serialize};

pub use config_parser::{DiscoveredSshHost, discover_ssh_hosts};
pub use connection_status::SshConnectionStatus;
pub use machine_keys::{MachineKey, MachineKeyError, MachineKeyRef, MachineKeys};

use horizon_browser_control::manifest::block_terminal::CheckedTerminal;

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
        let mut args = self.base_transport_args(false);
        args.push(self.transport_target());

        if let Some(remote_command) = non_empty(self.remote_command.as_deref()) {
            args.push(remote_command.to_string());
        }

        args
    }

    #[must_use]
    pub fn ssh_transport_args(&self) -> Vec<String> {
        let mut args = self.base_transport_args(true);
        args.push(self.transport_target());
        args
    }

    #[must_use]
    pub fn ssh_probe_transport_args(&self, connect_timeout_secs: u16) -> Vec<String> {
        let mut args = self.base_transport_args(true);
        args.extend(["-o".to_string(), format!("ConnectTimeout={connect_timeout_secs}")]);
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

    fn base_transport_args(&self, batch_mode: bool) -> Vec<String> {
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

        args.extend(default_keepalive_args());
        args.extend(self.extra_args.iter().cloned());
        args
    }
}

fn default_keepalive_args() -> impl IntoIterator<Item = String> {
    [
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
        return std::path::PathBuf::from(home).join(rest).display().to_string();
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
            2,
            "an agent adds no ssh option: {args:?}"
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
                "ServerAliveInterval=15".to_string(),
                "-o".to_string(),
                "ServerAliveCountMax=1".to_string(),
                "-o".to_string(),
                "ConnectTimeout=5".to_string(),
                "prod".to_string(),
            ]
        );
    }
}
