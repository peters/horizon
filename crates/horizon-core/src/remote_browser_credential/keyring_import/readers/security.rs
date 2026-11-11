//! `security find-generic-password -s <service> -a <account> -w`: the Keychain of
//! macOS, one item at a time.
//!
//! `security` of the system prints the password of the item on stdout and ends with
//! status 0. An item that the keychain does not hold ends with status 44
//! (`errSecItemNotFound`), which is not a failure: this computer holds no value
//! under that account. Another status is a failure that names its code and never
//! its output.
//!
//! The program adds one newline after the password it prints. That newline is not
//! part of the value, so one trailing newline is removed; a value that still holds
//! a line break afterwards does not parse, because the import cannot tell a line
//! break inside a value from the newline of the program.
//!
//! macOS may ask the person for permission for each item, because the Keychain
//! belongs to that person and not to Horizon. The bound of the import covers that
//! question.

use std::ffi::OsString;
use std::path::Path;

use zeroize::Zeroizing;

use super::super::KeyringImportError;
use super::super::program::{ProgramEnvironment, run};
use super::{Bound, SERVICE, ended, failure};

/// The program that reads the Keychain.
pub(crate) const PROGRAM: &str = "security";

/// The status of "no such item", which is not an error.
const NO_ITEM: i32 = 44;

/// The arguments of one reading of `user`'s value.
fn arguments(user: &str) -> Vec<OsString> {
    ["find-generic-password", "-s", SERVICE, "-a", user, "-w"]
        .map(OsString::from)
        .to_vec()
}

/// The value of the item of `user`, or `None` when the keychain holds none.
pub(crate) fn read(
    environment: &ProgramEnvironment,
    user: &str,
    bound: Bound,
) -> Result<Option<Zeroizing<Vec<u8>>>, KeyringImportError> {
    let Some(program) = environment.find(PROGRAM) else {
        return Err(KeyringImportError::ToolMissing);
    };
    read_with(&program, environment, user, bound)
}

/// The same reading with the program that [`read`] found, so a test names its own.
pub(crate) fn read_with(
    program: &Path,
    environment: &ProgramEnvironment,
    user: &str,
    bound: Bound,
) -> Result<Option<Zeroizing<Vec<u8>>>, KeyringImportError> {
    let output = run(program, &arguments(user), environment, bound.program())
        .map_err(|failed| failure(PROGRAM, failed, bound))?;
    if output.code() == Some(NO_ITEM) {
        return Ok(None);
    }
    if !output.status.success() {
        return Err(ended(PROGRAM, &output));
    }
    value(&output.stdout).map(Some)
}

/// The value of one successful reading.
fn value(stdout: &[u8]) -> Result<Zeroizing<Vec<u8>>, KeyringImportError> {
    let value = stdout.strip_suffix(b"\n").unwrap_or(stdout);
    if value.is_empty() {
        return Err(KeyringImportError::RefusedValue);
    }
    if value.iter().any(|byte| matches!(byte, b'\n' | b'\r')) {
        return Err(KeyringImportError::Unreadable);
    }
    Ok(Zeroizing::new(value.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::{NO_ITEM, arguments, value};

    #[test]
    fn the_arguments_name_the_service_and_the_account() {
        let arguments = arguments("https://grid.example.net|remote-browser/grid/key");
        assert_eq!(
            arguments,
            [
                "find-generic-password",
                "-s",
                "horizon-remote-browser",
                "-a",
                "https://grid.example.net|remote-browser/grid/key",
                "-w"
            ]
            .map(std::ffi::OsString::from)
            .to_vec()
        );
        assert_eq!(NO_ITEM, 44, "no such item is the status that security documents");
    }

    #[test]
    fn one_newline_of_the_program_is_removed_and_a_line_break_inside_the_value_is_refused() {
        assert_eq!(&*value(b"synthetic-token\n").expect("a value"), b"synthetic-token");
        assert_eq!(&*value(b"synthetic-token").expect("a value"), b"synthetic-token");
        assert!(value(b"two\nlines\n").is_err(), "a value with a line break does not parse");
        assert!(value(b"\n").is_err(), "an empty item is not a value");
    }
}
