//! The server that holds the machine list of more than one computer.
//!
//! A person names one SSH target and, when the default does not fit, the command
//! that the link runs there. A server that runs on a machine that Horizon rented
//! is named by the machine of the list instead, and then the link uses the address,
//! the user, the key file and the known-hosts file of a terminal of that machine:
//! the machine holds the public key of Horizon and no key of the person. Every
//! computer of the person that names the same server then shares one machine list.
//! This module owns the entry and its rule; the link lives in `horizon_sync`, and
//! the process of the server is the same `horizon` binary with `--server`.
//!
//! A non-interactive `ssh` reads a short `PATH`, so a person can write the whole
//! path of the binary in the command instead of the bare name.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The command that the link runs on the server when the entry names none.
pub const DEFAULT_LINK_COMMAND: &str = "horizon --server-link";

/// Longest SSH target of a server, as one argument of `ssh` takes.
pub const MAX_SSH_TARGET_LEN: usize = 255;

/// Longest command that the link runs on the server.
pub const MAX_COMMAND_LEN: usize = 200;

/// The characters that a command may hold. Every one of them passes the remote
/// shell of `ssh` without a quote, so the entry needs no shell rule of its own
/// and no value can start a second command. The underscore is one of them
/// because a path such as `/Users/x/my_tools/horizon` is a normal path.
const COMMAND_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 ._:/=@+-";

/// The longest name of this computer for the other computers of the person.
pub const MAX_COMPUTER_NAME_CHARS: usize = 60;

/// What the Server tab and the first link of a computer say about its terminals.
pub const SHARED_TERMINALS_NOTICE: &str = "The other computers that you link to this server can see the terminals \
    of this computer and type in them. Anyone who gets into one of those computers, or into the server, can then \
    run commands here.";

/// The server that the machines of this Horizon follow.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ServerConfig {
    /// SSH target of the computer that runs `horizon --server`. It is the name
    /// that `ssh` resolves: an SSH config alias, a user and host, or an address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_target: Option<String>,
    /// The name of a machine of the machine list that runs `horizon --server`. The
    /// link then uses the address, the user, the port, the key file and the
    /// known-hosts file of a terminal of that machine, because a machine that
    /// Horizon rented holds the machine key of Horizon and no key of the person. It
    /// names a machine and never a route, so a machine whose address changes keeps
    /// one entry here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// Command that the link runs on that computer. Without one it is
    /// [`DEFAULT_LINK_COMMAND`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Address of the listener of the server, `host:port`. With an address and
    /// the token of a join, the link of this computer runs over TCP instead of
    /// `ssh`. A successful join writes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// The name of this computer for the other computers of the person. Empty
    /// means the host name.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub computer_name: String,
    /// Whether the other computers of the person see the terminals of this one.
    /// The store writes it only when it is off, as it does for the next field.
    #[serde(skip_serializing_if = "Clone::clone")]
    pub share_terminals: bool,
    /// Whether those computers type in the terminals that they see. It has no
    /// effect while this computer shares none.
    #[serde(skip_serializing_if = "Clone::clone")]
    pub remote_typing: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            ssh_target: None,
            machine: None,
            command: None,
            address: None,
            computer_name: String::new(),
            share_terminals: true,
            remote_typing: true,
        }
    }
}

/// The server of this Horizon process, read once from the settings store at
/// start.
///
/// The sidebar and the machine store read it on many frames, so it is held here
/// instead of reading the store again. A change of the entry applies at the next
/// start.
static SERVER: OnceLock<Option<ServerConfig>> = OnceLock::new();

/// Set the server that the settings name. The first call wins, and it is the
/// start of Horizon that makes it.
pub fn configure_server(server: Option<ServerConfig>) {
    let _ = SERVER.set(server);
}

/// The server that the settings named at start, or `None` when the machine list
/// of this computer stays on this computer.
#[must_use]
pub fn server() -> Option<&'static ServerConfig> {
    SERVER.get().and_then(Option::as_ref)
}

impl ServerConfig {
    /// Whether the entry names a server. No server means the machine list is one
    /// file of this computer, as before this entry existed.
    #[must_use]
    pub fn names_server(&self) -> bool {
        self.address().is_some()
            || self.ssh_target().is_some_and(|target| !target.is_empty())
            || self.machine().is_some()
    }

    /// The machine of the list that runs the server, without the surrounding spaces
    /// that a person can type.
    #[must_use]
    pub fn machine(&self) -> Option<&str> {
        self.machine
            .as_deref()
            .map(str::trim)
            .filter(|machine| !machine.is_empty())
    }

    /// The address of the listener of the server, without the surrounding spaces
    /// that a person can type.
    #[must_use]
    pub fn address(&self) -> Option<&str> {
        self.address
            .as_deref()
            .map(str::trim)
            .filter(|address| !address.is_empty())
    }

    /// The SSH target, without the surrounding spaces that a person can type.
    #[must_use]
    pub fn ssh_target(&self) -> Option<&str> {
        self.ssh_target
            .as_deref()
            .map(str::trim)
            .filter(|target| !target.is_empty())
    }

    /// The command that the link runs on the server, without the surrounding
    /// spaces, or [`DEFAULT_LINK_COMMAND`].
    #[must_use]
    pub fn command(&self) -> &str {
        self.command
            .as_deref()
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .unwrap_or(DEFAULT_LINK_COMMAND)
    }

    /// The name that the other computers of the person read for this one: the
    /// name of the entry, or the host name of this computer.
    #[must_use]
    pub fn computer_label(&self) -> String {
        match self.computer_name.trim() {
            "" => crate::session_store::hostname(),
            name => name.to_owned(),
        }
    }

    /// # Errors
    /// Rejects an SSH target that `ssh` would read as an option or that holds a
    /// control character, a command that holds a character outside the allowed set
    /// or is too long, a machine name that breaks the rule of a machine, an
    /// entry that names both a machine and an SSH target, and a computer name
    /// that is too long or holds a control character.
    pub fn validate(&self) -> Result<()> {
        let name = &self.computer_name;
        if name.chars().count() > MAX_COMPUTER_NAME_CHARS || name.chars().any(char::is_control) {
            return Err(Error::Config(format!(
                "server.computer_name must be one line of at most {MAX_COMPUTER_NAME_CHARS} characters"
            )));
        }
        if let Some(machine) = self.machine() {
            crate::machines::machine_name(machine)
                .map_err(|error| Error::Config(format!("server.machine: {error}")))?;
            if self.ssh_target().is_some() {
                return Err(Error::Config(
                    "server.machine and server.ssh_target name one server twice; keep the machine of the list or the SSH target"
                        .into(),
                ));
            }
        }
        if let Some(target) = self.ssh_target() {
            if target.len() > MAX_SSH_TARGET_LEN {
                return Err(Error::Config(format!(
                    "server.ssh_target must be at most {MAX_SSH_TARGET_LEN} characters"
                )));
            }
            // `ssh` takes the target as one argument; a target that starts with
            // `-` would become an option, and whitespace would split it.
            if !horizon_browser_control::ssh_forward::valid_target(target) {
                return Err(Error::Config(
                    "server.ssh_target must be one SSH name without a leading dash or a space".into(),
                ));
            }
        }
        if let Some(command) = self.command.as_deref().map(str::trim)
            && !command.is_empty()
            && !valid_command(command)
        {
            return Err(Error::Config(format!(
                "server.command must be 1 to {MAX_COMMAND_LEN} characters of letters, digits and `._:/=@+-`, with single spaces between words"
            )));
        }
        if let Some(address) = self.address() {
            crate::server_link::ServerAddress::parse(address)
                .map_err(|error| Error::Config(format!("server.address: {error}")))?;
        }
        Ok(())
    }
}

/// Whether `command` is one line that `ssh` passes to the remote shell as this
/// program alone: the allowed characters, no leading or trailing space and no
/// two spaces in a row.
fn valid_command(command: &str) -> bool {
    command.len() <= MAX_COMMAND_LEN
        && command.chars().all(|character| COMMAND_CHARS.contains(character))
        && !command.starts_with(' ')
        && !command.ends_with(' ')
        && !command.contains("  ")
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_LINK_COMMAND, MAX_COMMAND_LEN, ServerConfig, server};
    use crate::Config;

    fn entry(ssh_target: Option<&str>, command: Option<&str>) -> ServerConfig {
        ServerConfig {
            ssh_target: ssh_target.map(str::to_string),
            command: command.map(str::to_string),
            ..ServerConfig::default()
        }
    }

    fn on_machine(machine: &str) -> ServerConfig {
        ServerConfig {
            machine: Some(machine.to_owned()),
            ..ServerConfig::default()
        }
    }

    fn with_address(address: Option<&str>) -> ServerConfig {
        ServerConfig {
            address: address.map(str::to_string),
            ..ServerConfig::default()
        }
    }

    #[test]
    fn a_machine_of_the_list_names_the_server_with_the_login_of_that_machine() {
        let config = on_machine("  rented-1  ");
        assert!(config.names_server(), "a machine alone names a server");
        assert_eq!(config.machine(), Some("rented-1"));
        assert_eq!(config.ssh_target(), None, "the machine is the route, not a target");
        assert_eq!(config.command(), DEFAULT_LINK_COMMAND);
        assert!(config.validate().is_ok());

        // One entry names one route: both fields would decide the same thing.
        let twice = ServerConfig {
            ssh_target: Some("server-vm".to_owned()),
            ..on_machine("rented-1")
        };
        let error = twice.validate().expect_err("two routes");
        assert!(
            error.to_string().contains("server.machine") && error.to_string().contains("server.ssh_target"),
            "the refusal names both fields: {error}"
        );

        for bad in ["two words", "-dash", "a/b", ""] {
            let config = on_machine(bad);
            if bad.trim().is_empty() {
                assert_eq!(config.machine(), None, "a blank machine names none");
                assert!(!config.names_server());
                continue;
            }
            let error = config.validate().expect_err(bad);
            assert!(!error.to_string().is_empty(), "{bad} is refused with a sentence");
        }

        // The entry round-trips as the name of the machine alone.
        let config = Config {
            server: on_machine("rented-1"),
            ..Config::default()
        };
        let document = config.to_json().expect("serialize");
        assert!(document.contains("\"machine\": \"rented-1\""), "{document}");
        assert!(!document.contains("ssh_target"), "{document}");
        assert_eq!(Config::from_json(&document).expect("parse").server, config.server);
    }

    #[test]
    fn an_empty_entry_names_no_server() {
        let config = ServerConfig::default();
        assert!(!config.names_server());
        assert!(config.validate().is_ok());
        assert_eq!(config.command(), DEFAULT_LINK_COMMAND);
        let missing: ServerConfig = serde_json::from_str("{}").expect("an empty section");
        assert!(!missing.names_server());
        assert_eq!(missing, ServerConfig::default());
        assert!(server().is_none(), "a start that names no server holds none");
    }

    #[test]
    fn a_target_names_the_server_and_the_command_is_optional() {
        let config = entry(Some("server-vm"), None);
        assert!(config.names_server());
        assert_eq!(config.ssh_target(), Some("server-vm"));
        assert_eq!(config.command(), DEFAULT_LINK_COMMAND);
        assert!(config.validate().is_ok());

        let config = entry(Some("  peters@10.0.0.5  "), Some(" /opt/horizon --server-link "));
        assert_eq!(config.ssh_target(), Some("peters@10.0.0.5"));
        assert_eq!(config.command(), "/opt/horizon --server-link");
        assert!(config.validate().is_ok());
    }

    /// A path of a person can hold an underscore, and the remote shell reads it
    /// as one word of the same program.
    #[test]
    fn a_command_holds_the_path_of_a_person_with_an_underscore() {
        for command in [
            "/Users/x/my_tools/horizon --server-link",
            "/opt/horizon_candidate --server-link",
            "horizon_2 --server-link",
            "/home/x/tools/hz-prod_2/bin/horizon --server-link",
        ] {
            let config = entry(Some("server-vm"), Some(command));
            assert!(config.validate().is_ok(), "{command} is one command");
            assert_eq!(config.command(), command);
        }
    }

    #[test]
    fn a_target_that_ssh_would_read_as_an_option_or_split_is_rejected() {
        for target in ["-oProxyCommand=evil", "two hosts", "host\nname"] {
            let error = entry(Some(target), None).validate().expect_err(target);
            assert!(error.to_string().contains("server.ssh_target"), "{error}");
        }
        let long = "a".repeat(super::MAX_SSH_TARGET_LEN + 1);
        assert!(entry(Some(&long), None).validate().is_err());
    }

    #[test]
    fn a_command_that_the_remote_shell_could_read_as_two_is_rejected() {
        for command in [
            "horizon; rm -rf /",
            "horizon $(id)",
            "horizon && id",
            "  two  spaces",
            "quote'",
        ] {
            let error = entry(Some("server-vm"), Some(command)).validate().expect_err(command);
            assert!(error.to_string().contains("server.command"), "{error}");
        }
        let long = "a".repeat(MAX_COMMAND_LEN + 1);
        assert!(entry(Some("server-vm"), Some(&long)).validate().is_err());
        for command in ["", "   "] {
            assert!(
                entry(Some("server-vm"), Some(command)).validate().is_ok(),
                "a blank command means the default"
            );
        }
    }

    #[test]
    fn an_address_names_the_listener_of_the_server() {
        let config = with_address(Some("127.0.0.1:7000"));
        assert!(config.names_server(), "an address alone names a server");
        assert_eq!(config.address(), Some("127.0.0.1:7000"));
        assert!(config.validate().is_ok());

        let spaced = with_address(Some("  server-computer:7000  "));
        assert_eq!(spaced.address(), Some("server-computer:7000"));
        assert!(spaced.validate().is_ok());

        let both = ServerConfig {
            address: Some("server-computer:7000".to_owned()),
            ..entry(Some("server-computer"), None)
        };
        assert!(both.names_server());
        assert_eq!(
            both.ssh_target(),
            Some("server-computer"),
            "ssh stays a second transport"
        );

        for bad in [
            "",
            "   ",
            "server",
            "server:0",
            "[::1]",
            "server:7000/path",
            "person@server:7000",
        ] {
            let config = with_address(Some(bad));
            if bad.trim().is_empty() {
                assert_eq!(config.address(), None, "a blank address names no listener");
                assert!(!config.names_server());
                continue;
            }
            let error = config.validate().expect_err(bad);
            assert!(error.to_string().contains("server.address"), "{error}");
        }
    }

    #[test]
    fn the_sharing_fields_default_to_on_and_round_trip() {
        // An older store holds none of the three fields, and reads as the defaults.
        let older: ServerConfig = serde_json::from_str(r#"{"ssh_target": "server-vm"}"#).expect("an older section");
        assert!(older.share_terminals && older.remote_typing, "both switches start on");
        assert!(!older.computer_label().is_empty(), "an empty name means the host name");
        let document = serde_json::to_string(&older).expect("serialize");
        assert_eq!(document, r#"{"ssh_target":"server-vm"}"#, "the defaults write no field");

        let config = Config {
            server: ServerConfig {
                computer_name: " Study desk ".to_owned(),
                share_terminals: false,
                remote_typing: false,
                ..entry(Some("server-vm"), None)
            },
            ..Config::default()
        };
        assert!(config.server.validate().is_ok());
        assert_eq!(config.server.computer_label(), "Study desk");
        let document = config.to_json().expect("serialize");
        assert!(document.contains("\"share_terminals\": false"), "{document}");
        assert_eq!(Config::from_json(&document).expect("parse").server, config.server);

        for bad in ["a".repeat(super::MAX_COMPUTER_NAME_CHARS + 1), "two\nlines".to_owned()] {
            let config = ServerConfig {
                computer_name: bad.clone(),
                ..ServerConfig::default()
            };
            let error = config.validate().expect_err(&bad);
            assert!(error.to_string().contains("server.computer_name"), "{error}");
        }
    }

    #[test]
    fn the_section_round_trips_through_the_configuration() {
        let config = Config {
            server: entry(Some("server-vm"), Some("/opt/horizon --server-link")),
            ..Config::default()
        };
        let document = config.to_json().expect("serialize");
        assert!(
            document.contains("\"ssh_target\": \"server-vm\"")
                && document.contains("\"command\": \"/opt/horizon --server-link\""),
            "{document}"
        );
        assert_eq!(Config::from_json(&document).expect("parse").server, config.server);

        let document = Config::default().to_json().expect("serialize");
        assert!(
            !document.contains("ssh_target") && !document.contains("address"),
            "an entry that names no server writes no field: {document}"
        );

        let config = Config {
            server: with_address(Some("server-computer:7000")),
            ..Config::default()
        };
        let document = config.to_json().expect("serialize");
        assert!(document.contains("\"address\": \"server-computer:7000\""), "{document}");
        assert_eq!(Config::from_json(&document).expect("parse").server, config.server);
    }
}
