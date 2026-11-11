//! `secret-tool search --all --unlock service horizon-remote-browser`: one process
//! that prints every item of the service of an older Horizon.
//!
//! The format below is the one of `secret-tool` in `tool/secret-tool.c` of libsecret,
//! function `on_retrieve_secret` (read from the project on 2026-11-11): for each item,
//! stdout holds `[<last part of the item path>]`, `label = <label>`,
//! `secret = <the value>` (the value is written as its own bytes and then a
//! newline), `created = <date or empty>`, `modified = <date or empty>` and, when the
//! item has a schema, `schema = <schema>`. The attributes are **not** on stdout:
//! the same function prints one `attribute.<name> = <value>` line for each attribute
//! on stderr, because `g_print` writes the item and `g_printerr` writes its
//! attributes. Both streams are read, and the order of both is the order of the
//! items, because the tool retrieves one item at a time.
//!
//! A value that holds a newline would move the following line of its item onto a
//! line of its own, and the format has no escape for it: such an answer is
//! [`KeyringImportError::Unreadable`] rather than a guess.

use std::ffi::OsString;
use std::path::Path;

use zeroize::Zeroizing;

use super::super::KeyringImportError;
use super::{Bound, KeyringEntry, SERVICE, ended, failure, field, lines, mentions, text};
use crate::bounded_program::{ProgramEnvironment, ProgramOutput, run};

/// The program that reads the Secret Service.
pub(crate) const PROGRAM: &str = "secret-tool";

/// The fields of one item that this import reads as presence only.
const IGNORED: [&str; 4] = ["label = ", "created = ", "modified = ", "schema = "];

/// The arguments of the search: every item of the service, with its value, and the
/// unlock of an item that is locked, so the system asks the person for it.
fn arguments() -> Vec<OsString> {
    ["search", "--all", "--unlock", "service", SERVICE]
        .map(OsString::from)
        .to_vec()
}

/// The entries of the service, read by one process.
pub(crate) fn read(
    program: &Path,
    environment: &ProgramEnvironment,
    bound: Bound,
) -> Result<Vec<KeyringEntry>, KeyringImportError> {
    let output =
        run(program, &arguments(), environment, bound.program()).map_err(|failed| failure(PROGRAM, failed, bound))?;
    if !output.status.success() {
        return Err(refusal(&output));
    }
    parse(&output.stdout, &output.stderr)
}

/// The failure of the program, in the words of the rule that libsecret names.
///
/// libsecret prints the D-Bus error of the Secret Service on stderr, and two of
/// those names say what the person can do: unlock the keyring, or start or install
/// a keyring service. The names are matched as the D-Bus interface writes them.
fn refusal(output: &ProgramOutput) -> KeyringImportError {
    if mentions(&output.stderr, b"org.freedesktop.Secret.Error.IsLocked") {
        return KeyringImportError::Locked;
    }
    if mentions(&output.stderr, b"org.freedesktop.DBus.Error.ServiceUnknown")
        || mentions(&output.stderr, b"not provided by any .service files")
    {
        return KeyringImportError::Absent;
    }
    ended(PROGRAM, output)
}

/// Pair the value of every item of stdout with the account of its item on stderr.
fn parse(stdout: &[u8], stderr: &[u8]) -> Result<Vec<KeyringEntry>, KeyringImportError> {
    let values = secrets(stdout)?;
    let users = users(stderr)?;
    if values.len() != users.len() {
        return Err(KeyringImportError::Unreadable);
    }
    Ok(values
        .into_iter()
        .zip(users)
        .map(|(value, user)| KeyringEntry { user, value })
        .collect())
}

/// The value of every item that stdout names, in the order of the items.
fn secrets(stdout: &[u8]) -> Result<Vec<Zeroizing<Vec<u8>>>, KeyringImportError> {
    let mut values: Vec<Zeroizing<Vec<u8>>> = Vec::new();
    let mut current: Option<Zeroizing<Vec<u8>>> = None;
    let mut items = 0_usize;
    for line in lines(stdout)? {
        if line.starts_with(b"[") && line.ends_with(b"]") {
            items += 1;
            values.extend(current.take());
            continue;
        }
        if items == 0 {
            return Err(KeyringImportError::Unreadable);
        }
        if let Some(value) = field(line, "secret = ") {
            // A second value in one item means the first one held a newline.
            if current.replace(Zeroizing::new(value.to_vec())).is_some() {
                return Err(KeyringImportError::Unreadable);
            }
            continue;
        }
        if IGNORED.iter().any(|prefix| field(line, prefix).is_some()) {
            continue;
        }
        return Err(KeyringImportError::Unreadable);
    }
    values.extend(current);
    if values.len() != items {
        return Err(KeyringImportError::Unreadable);
    }
    Ok(values)
}

/// The account of every item, in the order of the items.
///
/// One item names one account, so an answer whose accounts and items differ in
/// number is refused: pairing an account with the wrong item would write the value
/// of one reference over the value of another.
fn users(stderr: &[u8]) -> Result<Vec<String>, KeyringImportError> {
    let mut users: Vec<String> = Vec::new();
    for line in lines(stderr)? {
        let Some(rest) = field(line, "attribute.") else {
            // A message of the library beside the items, which names no account.
            continue;
        };
        let (key, value) = split_field(rest).ok_or(KeyringImportError::Unreadable)?;
        if text(key)? == "username" {
            users.push(text(value)?.to_string());
        }
    }
    Ok(users)
}

/// Split `name = value` at its first separator.
fn split_field(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let at = line.windows(3).position(|window| window == b" = ")?;
    Some((&line[..at], &line[at + 3..]))
}

#[cfg(test)]
mod tests {
    use super::{parse, secrets, users};

    /// The output of `secret-tool search --all --unlock service
    /// horizon-remote-browser` for two items, as `on_retrieve_secret` of libsecret
    /// writes it: the items on stdout, the attributes on stderr. The dates of the
    /// second item are empty, which is what the tool prints for an item without
    /// them.
    const STDOUT: &str = "[/org/freedesktop/secrets/collection/login/7]\n\
label = keyring:https://grid.example.net|remote-browser/grid/user@horizon-remote-browser\n\
secret = alice-value\n\
created = 2026-01-02 03:04:05\n\
modified = 2026-01-02 03:04:05\n\
[/org/freedesktop/secrets/collection/login/8]\n\
label = keyring:https://grid.example.net|remote-browser/grid/key@horizon-remote-browser\n\
secret = token with = and spaces\n\
created = \n\
modified = \n";

    const STDERR: &str = "attribute.service = horizon-remote-browser\n\
attribute.username = https://grid.example.net|remote-browser/grid/user\n\
attribute.service = horizon-remote-browser\n\
attribute.username = https://grid.example.net|remote-browser/grid/key\n";

    #[test]
    fn two_items_hold_their_value_and_their_account() {
        let entries = parse(STDOUT.as_bytes(), STDERR.as_bytes()).expect("two entries");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].user, "https://grid.example.net|remote-browser/grid/user");
        assert_eq!(&*entries[0].value, b"alice-value");
        assert_eq!(entries[1].user, "https://grid.example.net|remote-browser/grid/key");
        assert_eq!(&*entries[1].value, b"token with = and spaces");
    }

    #[test]
    fn a_value_that_holds_a_line_break_is_refused() {
        let broken = STDOUT.replace("secret = alice-value", "secret = alice\nvalue");
        assert!(
            secrets(broken.as_bytes()).is_err(),
            "a value with a line break does not parse"
        );
    }

    #[test]
    fn an_item_without_a_value_is_refused() {
        let stdout = "[/one]\nlabel = a\ncreated = \nmodified = \n";
        assert!(
            parse(stdout.as_bytes(), b"attribute.username = one\n").is_err(),
            "an item must carry a value"
        );
    }

    #[test]
    fn an_item_that_names_no_account_is_refused() {
        assert!(
            users(b"attribute.service = horizon-remote-browser\n")
                .unwrap_or_default()
                .is_empty(),
            "an attribute that is not an account names no item"
        );
        assert!(
            parse(STDOUT.as_bytes(), b"").is_err(),
            "a value whose item names no account is not paired with a guess"
        );
    }
}
