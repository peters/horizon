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

/// The longest repository line kept, in characters.
pub const MAX_REPOSITORY_CHARS: usize = 60;

/// The longest tmux session name accepted, in characters.
pub const MAX_SESSION_CHARS: usize = 40;

/// The longest fact of a rented machine kept, in characters.
pub const MAX_FACT_CHARS: usize = 40;

/// Refusal code of a folder of a machine that is not one absolute path.
pub const INVALID_FOLDER: &str = "invalid_folder";

/// Refusal code of a size, a place or a price of a machine that breaks its rule.
pub const INVALID_MACHINE_FACT: &str = "invalid_machine_fact";

const MAX_FOLDER_BYTES: usize = 512;

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

/// Check the folder that the new terminals of a machine start in, which reaches
/// `tmux new-session -c '<folder>'` as one quoted word.
///
/// # Errors
/// Describes the rule with [`INVALID_FOLDER`].
pub fn check_folder(folder: &str) -> Result<&str, MachineRefusal> {
    let rule = format!("folder must be an absolute path of at most {MAX_FOLDER_BYTES} bytes with no control character");
    if folder.len() > MAX_FOLDER_BYTES
        || folder.chars().any(char::is_control)
        || !std::path::Path::new(folder).is_absolute()
    {
        return Err(MachineRefusal::new(INVALID_FOLDER, rule));
    }
    Ok(folder)
}

/// Check one fact of a rented machine: its `size`, its `place` or its
/// `price_per_hour`, such as `cpx32`, `Helsinki` or `0.03 EUR`.
///
/// The fact is text of the agent program that read it from its provider, so it is
/// one line of 1 to [`MAX_FACT_CHARS`] characters with no control character, and
/// a value outside that bound is refused with the name of its field. The empty
/// text is no fact: a caller that sends it clears the one that the record holds.
///
/// # Errors
/// Describes the rule of `field` with [`INVALID_MACHINE_FACT`].
pub fn check_fact(field: &str, text: &str) -> Result<String, MachineRefusal> {
    let rule = format!("{field} must be 1 to {MAX_FACT_CHARS} characters on one line with no control character");
    if text.chars().count() > MAX_FACT_CHARS || text.chars().any(char::is_control) || text.is_empty() {
        return Err(MachineRefusal::new(INVALID_MACHINE_FACT, rule));
    }
    Ok(text.to_owned())
}

/// The provider line and the repository line are short texts of a record that
/// Horizon only draws: one line of visible text, at most `max_chars` characters.
/// A longer or doubled line is cut instead of refused.
fn short_line(text: &str, max_chars: usize) -> String {
    let line: String = text
        .chars()
        .map(|character| if character.is_control() { ' ' } else { character })
        .collect();
    line.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

/// The provider line that a record keeps: one line of visible text, at most
/// [`MAX_PROVIDER_CHARS`] characters. A longer or doubled line is cut instead
/// of refused, because the text is a claim that Horizon only draws.
#[must_use]
pub fn provider_line(provider: &str) -> String {
    short_line(provider, MAX_PROVIDER_CHARS)
}

/// The repository line that a record keeps: one line of visible text, at most
/// [`MAX_REPOSITORY_CHARS`] characters, such as `example-org/shop`. It is the
/// rule of the provider line, because it is another short text of a record that
/// Horizon only draws.
#[must_use]
pub fn repository_line(repository: &str) -> String {
    short_line(repository, MAX_REPOSITORY_CHARS)
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
    fn a_repository_line_is_folded_and_cut_to_the_bound() {
        assert_eq!(repository_line("  example-org/shop  "), "example-org/shop");
        assert_eq!(repository_line("example-org / shop"), "example-org / shop");
        assert_eq!(repository_line(""), "");
        assert_eq!(
            repository_line(&"r".repeat(MAX_REPOSITORY_CHARS + 20)),
            "r".repeat(MAX_REPOSITORY_CHARS)
        );
        assert_eq!(repository_line("example-org\u{7}/shop"), "example-org /shop");
    }

    #[test]
    fn a_fact_of_a_rented_machine_is_one_line_within_the_bound() {
        for text in ["cpx32", "Helsinki", "0.03 EUR", "a"] {
            assert_eq!(
                check_fact("place", text).expect("one fact of a machine"),
                text,
                "a fact is kept as the agent program sent it"
            );
        }
        let longest = "p".repeat(MAX_FACT_CHARS);
        assert_eq!(check_fact("size", &longest).expect("a fact at the bound"), longest);
        for text in [
            String::new(),
            "p".repeat(MAX_FACT_CHARS + 1),
            "cpx32\nnl-1".to_string(),
            "2 vCPU\u{7}".to_string(),
            "Helsinki\r".to_string(),
        ] {
            let refusal = check_fact("place", &text).expect_err("the fact must be refused");
            assert_eq!(refusal.code, INVALID_MACHINE_FACT, "{text:?}");
            assert!(
                refusal.message.contains("place"),
                "{text:?} refuses without the name of its field: {}",
                refusal.message
            );
        }
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
        assert_eq!(MAX_REPOSITORY_CHARS, 60);
        assert_eq!(MAX_SESSION_CHARS, 40);
        assert_eq!(MAX_FACT_CHARS, 40);
        assert_eq!(INVALID_MACHINE_FACT, "invalid_machine_fact");
    }
}
