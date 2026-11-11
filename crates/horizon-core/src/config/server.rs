//! The server that holds the machine list of more than one computer.
//!
//! A person names one SSH target and, when the default does not fit, the command
//! that the link runs there. Every computer of the person that names the same
//! server then shares one machine list. This module owns the entry and its rule;
//! the link lives in `horizon_sync`, and the process of the server is the same
//! `horizon` binary with `--server`.
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

/// The server that the machines of this Horizon follow.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ServerConfig {
    /// SSH target of the computer that runs `horizon --server`. It is the name
    /// that `ssh` resolves: an SSH config alias, a user and host, or an address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_target: Option<String>,
    /// Command that the link runs on that computer. Without one it is
    /// [`DEFAULT_LINK_COMMAND`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
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
        self.ssh_target().is_some_and(|target| !target.is_empty())
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

    /// # Errors
    /// Rejects an SSH target that `ssh` would read as an option or that holds a
    /// control character, and a command that holds a character outside the
    /// allowed set or is too long.
    pub fn validate(&self) -> Result<()> {
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
        }
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
    fn the_section_round_trips_through_the_configuration() {
        let config = Config {
            server: entry(Some("server-vm"), Some("/opt/horizon --server-link")),
            ..Config::default()
        };
        let yaml = config.to_yaml().expect("yaml");
        assert!(
            yaml.contains("server:\n  ssh_target: server-vm\n  command: /opt/horizon --server-link\n"),
            "{yaml}"
        );
        assert_eq!(Config::from_yaml(&yaml).expect("parse").server, config.server);

        let yaml = Config::default().to_yaml().expect("yaml");
        assert!(
            !yaml.contains("server:") || !yaml.contains("ssh_target"),
            "an entry that names no server writes no field: {yaml}"
        );
    }
}
