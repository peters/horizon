//! The SSH key that Horizon keeps for each machine an agent rents.
//!
//! An agent that rents a machine puts a public key on it at rent time and asks
//! Horizon to connect with the private half. The key pair is Horizon's, in the
//! Horizon home, so the person's own keys and ssh agent never reach a machine
//! that an agent rented, and the login does not fail because an agent offers
//! more keys than the server allows.
//!
//! An agent names a key and nothing else: it passes no path, no passphrase, no
//! ssh option and no key material. This module holds the name rule and the
//! refusal codes, which the MCP server and the UI both use; the files belong to
//! `MachineKeys` in `horizon-core`, which runs on the coordination worker.

/// The longest key name accepted, in characters.
pub const MAX_KEY_NAME_CHARS: usize = 40;

/// Most key pairs one workspace may hold at a time.
pub const MAX_MACHINE_KEYS_PER_WORKSPACE: usize = 32;

/// Refusal code of a name that is not one key label.
pub const INVALID_KEY_NAME: &str = "invalid_key_name";

/// Refusal code of a workspace that already holds the most keys.
pub const TOO_MANY_KEYS: &str = "too_many_keys";

/// Refusal code of a key that is not there, or that Horizon could not make.
pub const KEY_UNAVAILABLE: &str = "key_unavailable";

/// Why a key name is not one label that Horizon may keep. `code` is the stable
/// value that the tool reports, and `message` names the rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyRefusal {
    pub code: &'static str,
    pub message: String,
}

impl KeyRefusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Check one key name: one to [`MAX_KEY_NAME_CHARS`] characters of `a-z`, `0-9`
/// and `-`, and no `-` at either end.
///
/// The name becomes a file name under the Horizon home, so the rule keeps it
/// from a path, a leading `-` and a name that reads as another file such as
/// `<name>.known_hosts`.
///
/// # Errors
/// Describes the rule with [`INVALID_KEY_NAME`].
pub fn check_name(name: &str) -> Result<&str, KeyRefusal> {
    let refused = |message: String| Err(KeyRefusal::new(INVALID_KEY_NAME, message));
    if name.is_empty() {
        return refused("name must be 1 to 40 characters of 'a-z', '0-9' and '-'".into());
    }
    if name.chars().count() > MAX_KEY_NAME_CHARS {
        return refused(format!("name must be at most {MAX_KEY_NAME_CHARS} characters"));
    }
    if let Some(character) = name
        .chars()
        .find(|character| !(character.is_ascii_lowercase() || character.is_ascii_digit() || *character == '-'))
    {
        return refused(format!(
            "name may only contain 'a-z', '0-9' and '-'; it holds {character:?}"
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return refused("name cannot start or end with '-'".into());
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(name: &str) -> (String, String) {
        let refusal = check_name(name).expect_err("the name must be refused");
        (refusal.code.into(), refusal.message)
    }

    #[test]
    fn a_plain_name_is_kept_as_it_was_sent() {
        for name in ["build-1", "a", "0", "rented-machine-17", "a-b-c"] {
            assert_eq!(check_name(name).expect("a plain name"), name);
        }
        let longest = "a".repeat(MAX_KEY_NAME_CHARS);
        assert_eq!(
            check_name(&longest).expect("a name at the bound"),
            longest.as_str(),
            "a name at the bound is accepted"
        );
        let mut dashes_inside = "a".repeat(MAX_KEY_NAME_CHARS - 2);
        dashes_inside.insert(1, '-');
        dashes_inside.push('-');
        assert!(
            check_name(&dashes_inside).is_err(),
            "a name at the bound cannot end with '-'"
        );
    }

    #[test]
    fn every_name_refusal_names_the_rule() {
        for name in [
            String::new(),
            "a".repeat(MAX_KEY_NAME_CHARS + 1),
            "-build".to_string(),
            "build-".to_string(),
            "-".to_string(),
            "Build".to_string(),
            "build_1".to_string(),
            "build 1".to_string(),
            "build/1".to_string(),
            "build.1".to_string(),
            "../build".to_string(),
            "/home/peters/.ssh/id_ed25519".to_string(),
            "build-1\n".to_string(),
            "build-1\u{0}".to_string(),
            "k\u{00e5}re".to_string(),
        ] {
            let (code, message) = refusal(&name);
            assert_eq!(code, INVALID_KEY_NAME, "{name:?}");
            assert!(message.contains("name"), "{name:?} refuses without the rule: {message}");
        }
    }

    #[test]
    fn a_dot_never_reaches_a_name_because_the_pair_and_the_known_hosts_share_the_directory() {
        for name in ["build-1.pub", "build-1.known_hosts", "build-1.pub.bak"] {
            assert_eq!(
                refusal(name).0,
                INVALID_KEY_NAME,
                "{name} would read as another file of the same directory"
            );
        }
    }

    #[test]
    fn the_codes_are_the_ones_the_tool_reports() {
        assert_eq!(INVALID_KEY_NAME, "invalid_key_name");
        assert_eq!(TOO_MANY_KEYS, "too_many_keys");
        assert_eq!(KEY_UNAVAILABLE, "key_unavailable");
        assert_eq!(MAX_MACHINE_KEYS_PER_WORKSPACE, 32);
    }
}
