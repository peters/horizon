//! The SSH terminal that an agent asks Horizon to open for a person.
//!
//! `block_panel open_terminal` reaches a machine that a person uses, so the
//! input is one route, one optional command and one optional title. Horizon
//! builds the `ssh` command line itself: an agent contributes no option, no
//! identity file, no extra argument and no environment, so it cannot turn its
//! input into a different program or an ssh option.
//!
//! A host, a user and a remote command are untrusted. This module holds their
//! only check, and both the MCP server and the UI call it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::device::SshRoute;

/// The longest remote command kept, in characters.
pub const MAX_REMOTE_COMMAND_CHARS: usize = 512;

/// Refusal code of a host, user or port that is not one SSH label and port.
pub const INVALID_SSH_ROUTE: &str = "invalid_ssh_route";

/// Refusal code of a remote command that is not one plain line.
pub const INVALID_REMOTE_COMMAND: &str = "invalid_remote_command";

/// Characters that a remote command may hold besides letters and digits: the
/// words, paths, ports and options of one command line, and nothing that a
/// remote shell reads as a second command or a substitution.
const PLAIN_COMMAND_EXTRA: [char; 10] = [' ', '.', '_', ':', '/', ',', '=', '@', '+', '-'];

/// The route of an `open_terminal` request, as the agent sends it.
///
/// `port` is wider than the SSH port on purpose: a value outside 1..=65535 is
/// then refused with [`INVALID_SSH_ROUTE`] instead of a JSON error that names
/// no field.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TerminalRoute {
    /// Host name, address or SSH config alias. Required.
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// SSH port from 1 to 65535; omitted means the SSH configuration's port or
    /// 22.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i64>,
}

/// A route, a command and a title that passed [`check`]: the only values that
/// reach an `ssh` command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedTerminal {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub remote_command: Option<String>,
}

impl CheckedTerminal {
    /// Whether `ssh` must allocate a terminal: only a remote command runs on
    /// one, because `ssh host command` uses no TTY by default.
    #[must_use]
    pub fn needs_tty(&self) -> bool {
        self.remote_command.is_some()
    }
}

/// Why a route or a command is not one terminal that Horizon may open. `code`
/// is the stable value that the tool reports, and `message` names the field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalRefusal {
    pub code: &'static str,
    pub message: String,
}

impl TerminalRefusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Check one route and one remote command.
///
/// The host and the user are normalised with the check of a Device panel route,
/// so a label that starts with `-`, holds whitespace or a control character
/// never reaches an `ssh` option, a `ProxyCommand` or a shell. A remote command
/// is one plain line from [`PLAIN_COMMAND_EXTRA`], so a remote shell cannot read
/// it as a second command.
///
/// # Errors
/// Describes the first refused field with [`INVALID_SSH_ROUTE`] or
/// [`INVALID_REMOTE_COMMAND`].
pub fn check(route: &TerminalRoute, remote_command: Option<&str>) -> Result<CheckedTerminal, TerminalRefusal> {
    let port = match route.port {
        None => None,
        Some(port) => {
            let port = u16::try_from(port)
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| TerminalRefusal::new(INVALID_SSH_ROUTE, "ssh.port must be from 1 to 65535"))?;
            Some(port)
        }
    };
    let mut label = SshRoute {
        host: route.host.clone(),
        user: route.user.clone(),
        port,
    };
    label
        .normalize()
        .map_err(|message| TerminalRefusal::new(INVALID_SSH_ROUTE, message))?;
    let remote_command = match remote_command {
        None => None,
        Some(command) => Some(plain_command(command)?),
    };
    Ok(CheckedTerminal {
        host: label.host,
        user: label.user,
        port: label.port,
        remote_command,
    })
}

/// One checked remote command: trimmed, 1 to [`MAX_REMOTE_COMMAND_CHARS`]
/// characters, no leading `-` and only the characters of a plain command line.
fn plain_command(command: &str) -> Result<String, TerminalRefusal> {
    let command = command.trim();
    let refused = |message: String| TerminalRefusal::new(INVALID_REMOTE_COMMAND, message);
    if command.is_empty() {
        return Err(refused(
            "remote_command must hold one command; leave it out to open a plain shell".into(),
        ));
    }
    if command.chars().count() > MAX_REMOTE_COMMAND_CHARS {
        return Err(refused(format!(
            "remote_command must be at most {MAX_REMOTE_COMMAND_CHARS} characters"
        )));
    }
    if command.starts_with('-') {
        return Err(refused(
            "remote_command cannot start with '-', because it would read as an ssh option".into(),
        ));
    }
    if let Some(character) = command.chars().find(|character| !plain(*character)) {
        return Err(refused(format!(
            "remote_command may only contain letters, digits, spaces and the characters '.', '_', ':', '/', ',', '=', '@', '+' and '-'; it holds {character:?}"
        )));
    }
    Ok(command.to_owned())
}

fn plain(character: char) -> bool {
    character.is_ascii_alphanumeric() || PLAIN_COMMAND_EXTRA.contains(&character)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(host: &str, user: Option<&str>, port: Option<i64>) -> TerminalRoute {
        TerminalRoute {
            host: host.into(),
            user: user.map(str::to_string),
            port,
        }
    }

    fn refused(route: &TerminalRoute, command: Option<&str>) -> (String, String) {
        let refusal = check(route, command).expect_err("the terminal must be refused");
        (refusal.code.into(), refusal.message)
    }

    fn checked(route: &TerminalRoute, command: Option<&str>) -> CheckedTerminal {
        check(route, command).expect("the terminal is accepted")
    }

    #[test]
    fn a_plain_route_and_command_keep_exactly_the_fields_the_agent_sent() {
        let free = checked(&route(" lab-1 ", Some(" deploy "), Some(2222)), None);
        assert_eq!(free.host, "lab-1");
        assert_eq!(free.user.as_deref(), Some("deploy"));
        assert_eq!(free.port, Some(2222));
        assert_eq!(free.remote_command, None);
        assert!(!free.needs_tty(), "a plain shell allocates its own terminal");

        let command = checked(&route("203.0.113.10", None, None), Some("  tmux new -A -s build  "));
        assert_eq!(command.remote_command.as_deref(), Some("tmux new -A -s build"));
        assert!(command.needs_tty(), "a remote command needs -t");
        assert_eq!(command.user, None);
        assert_eq!(command.port, None);
    }

    #[test]
    fn an_unknown_route_field_is_refused_before_the_check() {
        assert!(
            serde_json::from_value::<TerminalRoute>(serde_json::json!({
                "host": "lab-1", "identity_file": "/home/peters/.ssh/id_ed25519"
            }))
            .is_err(),
            "an agent can pass no identity file"
        );
        assert!(
            serde_json::from_value::<TerminalRoute>(serde_json::json!({
                "host": "lab-1", "options": ["-oProxyCommand=touch /tmp/x"]
            }))
            .is_err(),
            "an agent can pass no ssh option"
        );
    }

    #[test]
    fn every_route_refusal_names_its_field() {
        for (route, field) in [
            (route("", None, None), "ssh.host"),
            (route("   ", None, None), "ssh.host"),
            (route("-oProxyCommand=id", None, None), "ssh.host"),
            (route("lab 1", None, None), "ssh.host"),
            (route("lab\t1", None, None), "ssh.host"),
            (route("lab\u{7}", None, None), "ssh.host"),
            (route(&"a".repeat(SshRoute::MAX_HOST_CHARS + 1), None, None), "ssh.host"),
            (route("lab-1", Some("-oProxyCommand=id"), None), "ssh.user"),
            (route("lab-1", Some("de ploy"), None), "ssh.user"),
            (route("lab-1", Some(""), None), "ssh.user"),
            (route("lab-1", None, Some(0)), "ssh.port"),
            (route("lab-1", None, Some(-1)), "ssh.port"),
            (route("lab-1", None, Some(65_536)), "ssh.port"),
        ] {
            let (code, message) = refused(&route, None);
            assert_eq!(code, INVALID_SSH_ROUTE, "{route:?}");
            assert!(message.contains(field), "{route:?} refuses without {field}: {message}");
        }
    }

    #[test]
    fn every_command_refusal_names_the_field() {
        for command in [
            String::new(),
            "   ".to_string(),
            "-oProxyCommand=id".to_string(),
            "tmux new; id".to_string(),
            "tmux new && id".to_string(),
            "tmux new | id".to_string(),
            "echo $HOME".to_string(),
            "echo `id`".to_string(),
            "cat < /etc/passwd".to_string(),
            "echo hi > /tmp/x".to_string(),
            "$(id)".to_string(),
            "echo \\n".to_string(),
            "tmux new\nid".to_string(),
            "tmux new\rid".to_string(),
            "tmux new\u{0}".to_string(),
            "\"tmux new\"".to_string(),
            "'tmux new'".to_string(),
            "a".repeat(MAX_REMOTE_COMMAND_CHARS + 1),
        ] {
            let (code, message) = refused(&route("lab-1", None, None), Some(&command));
            assert_eq!(code, INVALID_REMOTE_COMMAND, "{command:?}");
            assert!(
                message.contains("remote_command"),
                "{command:?} refuses without the field: {message}"
            );
        }
        let longest = "a".repeat(MAX_REMOTE_COMMAND_CHARS);
        assert_eq!(
            checked(&route("lab-1", None, None), Some(&longest))
                .remote_command
                .as_deref(),
            Some(longest.as_str()),
            "a command at the bound is accepted"
        );
    }

    #[test]
    fn option_injection_through_any_field_is_refused() {
        // Each attempt is a value that `ssh` would read as an option if it
        // reached the command line as its own argument.
        let attempts = ["-oProxyCommand=touch /tmp/horizon-c5", "-L", "-i/tmp/key"];
        for attempt in attempts {
            assert_eq!(
                refused(&route(attempt, None, None), None).0,
                INVALID_SSH_ROUTE,
                "{attempt} as a host"
            );
            assert_eq!(
                refused(&route("lab-1", Some(attempt), None), None).0,
                INVALID_SSH_ROUTE,
                "{attempt} as a user"
            );
            assert_eq!(
                refused(&route("lab-1", None, None), Some(attempt)).0,
                INVALID_REMOTE_COMMAND,
                "{attempt} as a command"
            );
        }
    }
}
