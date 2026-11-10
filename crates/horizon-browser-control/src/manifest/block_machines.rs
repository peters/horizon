//! The machines that an agent registers with Horizon.
//!
//! A machine is a saved SSH route with a name, the key that Horizon keeps for
//! it, an optional tmux session and the provider line that the agent read from
//! its CLI. The records live in the Horizon home, because the machines are the
//! person's and not a workspace's; `horizon-core` writes the file on the
//! coordination worker. This module holds the rules and the refusal codes that
//! the MCP server and the UI both use, as [`super::block_keys`] does for keys.
//!
//! The provider line is free text from an agent: it is the agent's claim, drawn
//! in a low-emphasis style, and no program reads it.

/// Most machines one Horizon home keeps at a time.
pub const MAX_MACHINES: usize = 64;

/// The longest provider line kept, in characters.
pub const MAX_PROVIDER_CHARS: usize = 60;

/// The longest tmux session name accepted, in characters.
pub const MAX_SESSION_CHARS: usize = 40;

/// Refusal code of a machine name that is not one key label.
pub const INVALID_MACHINE_NAME: &str = "invalid_machine_name";

/// Refusal code of a tmux session name that is not one plain name.
pub const INVALID_SESSION_NAME: &str = "invalid_session_name";

/// Refusal code of a machine that this Horizon home does not keep.
pub const MACHINE_UNAVAILABLE: &str = "machine_unavailable";

/// Refusal code of a home that already keeps the most machines.
pub const TOO_MANY_MACHINES: &str = "too_many_machines";

/// Refusal code of an operation whose fields contradict each other, such as an
/// `open_terminal` that names both a route and a machine, or neither.
pub const INVALID_REQUEST: &str = "invalid_request";

/// Why a machine record is not one that Horizon may keep. `code` is the stable
/// value that the tool reports, and `message` names the field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineRefusal {
    pub code: &'static str,
    pub message: String,
}

impl MachineRefusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Check one machine name. It is the rule of a machine key name, because the
/// record and the key pair carry one name: 1 to 40 characters of `a-z`, `0-9`
/// and `-`, with no `-` at either end.
///
/// # Errors
/// Describes the rule with [`INVALID_MACHINE_NAME`].
pub fn check_machine_name(name: &str) -> Result<&str, MachineRefusal> {
    super::block_keys::check_name(name).map_err(|refusal| {
        MachineRefusal::new(
            INVALID_MACHINE_NAME,
            format!(
                "machine name {}",
                refusal.message.trim_start_matches("name ").trim_end_matches("name ")
            ),
        )
    })
}

/// Check one tmux session name: one to [`MAX_SESSION_CHARS`] characters of
/// `A-Z`, `a-z`, `0-9`, `_` and `-`, and no `-` in front.
///
/// The name reaches `tmux new -A -s <session>` as one word, so the rule keeps a
/// space, a quote, a `;` and a leading `-` out of that command.
///
/// # Errors
/// Describes the rule with [`INVALID_SESSION_NAME`].
pub fn check_session_name(session: &str) -> Result<&str, MachineRefusal> {
    let refused = |message: String| Err(MachineRefusal::new(INVALID_SESSION_NAME, message));
    if session.is_empty() {
        return refused(format!(
            "session must be 1 to {MAX_SESSION_CHARS} characters of 'A-Z', 'a-z', '0-9', '_' and '-'"
        ));
    }
    if session.chars().count() > MAX_SESSION_CHARS {
        return refused(format!("session must be at most {MAX_SESSION_CHARS} characters"));
    }
    if let Some(character) = session
        .chars()
        .find(|character| !(character.is_ascii_alphanumeric() || *character == '_' || *character == '-'))
    {
        return refused(format!(
            "session may only contain 'A-Z', 'a-z', '0-9', '_' and '-'; it holds {character:?}"
        ));
    }
    if session.starts_with('-') {
        return refused("session cannot start with '-', because tmux would read it as an option".into());
    }
    Ok(session)
}

/// The provider line that a record keeps: one line of visible text, at most
/// [`MAX_PROVIDER_CHARS`] characters. A longer or doubled line is cut instead
/// of refused, because the text is a claim that Horizon only draws.
#[must_use]
pub fn provider_line(provider: &str) -> String {
    let line: String = provider
        .chars()
        .map(|character| if character.is_control() { ' ' } else { character })
        .collect();
    line.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_PROVIDER_CHARS)
        .collect()
}

/// Check that an `open_terminal` names exactly one target: a route, or a
/// machine of this Horizon home. A caller that sends both, or neither, learns
/// the name of the field it must leave out or add.
///
/// # Errors
/// Names the offending field with [`INVALID_REQUEST`].
pub fn check_single_target(has_route: bool, has_machine: bool) -> Result<(), MachineRefusal> {
    match (has_route, has_machine) {
        (true, true) => Err(MachineRefusal::new(
            INVALID_REQUEST,
            "open_terminal names either ssh or machine, not both: leave out ssh when you pass machine, because Horizon takes the route from the record",
        )),
        (false, false) => Err(MachineRefusal::new(
            INVALID_REQUEST,
            "open_terminal needs ssh or machine: pass ssh with a host and an optional user and port, or machine with the name of a machine in this Horizon",
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(name: &str) -> (String, String) {
        let refusal = check_machine_name(name).expect_err("the name must be refused");
        (refusal.code.into(), refusal.message)
    }

    #[test]
    fn a_machine_name_follows_the_key_rule_and_says_so() {
        for name in ["build-1", "a", "0", "rented-machine-17", "a-b-c"] {
            assert_eq!(check_machine_name(name).expect("a plain name"), name);
        }
        let longest = "a".repeat(super::super::block_keys::MAX_KEY_NAME_CHARS);
        assert_eq!(
            check_machine_name(&longest).expect("a name at the bound"),
            longest.as_str()
        );
        for name in [
            String::new(),
            "a".repeat(super::super::block_keys::MAX_KEY_NAME_CHARS + 1),
            "-build".to_string(),
            "build-".to_string(),
            "Build".to_string(),
            "build_1".to_string(),
            "build 1".to_string(),
            "build/1".to_string(),
            "build.1".to_string(),
            "../build".to_string(),
            "build-1\n".to_string(),
        ] {
            let (code, message) = refusal(&name);
            assert_eq!(code, INVALID_MACHINE_NAME, "{name:?}");
            assert!(message.contains("name"), "{name:?} refuses without the rule: {message}");
        }
    }

    #[test]
    fn a_session_name_is_one_word_of_tmux_characters() {
        for session in ["build", "build-1", "Build_1-x", "a"] {
            assert_eq!(check_session_name(session).expect("a plain session"), session);
        }
        let longest = "a".repeat(MAX_SESSION_CHARS);
        assert_eq!(
            check_session_name(&longest).expect("a session at the bound"),
            longest.as_str()
        );
        for session in [
            String::new(),
            "a".repeat(MAX_SESSION_CHARS + 1),
            "-rf".to_string(),
            "build session".to_string(),
            "build:1".to_string(),
            "build;id".to_string(),
            "build$(id)".to_string(),
            "build\n".to_string(),
            "k\u{00e5}re".to_string(),
        ] {
            let refusal = check_session_name(&session).expect_err("the session must be refused");
            assert_eq!(refusal.code, INVALID_SESSION_NAME, "{session:?}");
            assert!(
                refusal.message.contains("session"),
                "{session:?} refuses without the field: {}",
                refusal.message
            );
        }
    }

    #[test]
    fn a_provider_line_is_folded_and_cut_to_the_bound() {
        assert_eq!(provider_line("  Hetzner,   hel1\nrunning  "), "Hetzner, hel1 running");
        assert_eq!(provider_line(""), "");
        assert_eq!(
            provider_line(&"a".repeat(MAX_PROVIDER_CHARS + 20)),
            "a".repeat(MAX_PROVIDER_CHARS)
        );
        assert_eq!(provider_line("Hetzner\u{7}hel1"), "Hetzner hel1");
    }

    #[test]
    fn one_terminal_names_one_target() {
        assert!(check_single_target(true, false).is_ok(), "a route alone is a terminal");
        assert!(
            check_single_target(false, true).is_ok(),
            "a machine alone is a terminal"
        );
        for (has_route, has_machine, field) in [(true, true, "ssh"), (false, false, "ssh or machine")] {
            let refusal = check_single_target(has_route, has_machine).expect_err("no single target");
            assert_eq!(refusal.code, INVALID_REQUEST);
            assert!(refusal.message.contains(field), "{}", refusal.message);
        }
    }

    #[test]
    fn the_codes_are_the_ones_the_tool_reports() {
        assert_eq!(INVALID_MACHINE_NAME, "invalid_machine_name");
        assert_eq!(INVALID_SESSION_NAME, "invalid_session_name");
        assert_eq!(MACHINE_UNAVAILABLE, "machine_unavailable");
        assert_eq!(TOO_MANY_MACHINES, "too_many_machines");
        assert_eq!(INVALID_REQUEST, "invalid_request");
        assert_eq!(MAX_MACHINES, 64);
        assert_eq!(MAX_PROVIDER_CHARS, 60);
        assert_eq!(MAX_SESSION_CHARS, 40);
    }
}
