//! The three programs of the import, run as stand-ins on a private `PATH`.

use std::time::{Duration, Instant};

use rustix::process::{Pid, test_kill_process};

use super::super::readers::{Bound, libsecret, platform, secret_tool, security};
use super::super::{KeyringImportError, ProgramEnvironment};
use super::stand_in::{StandIns, found};

/// One item of `secret-tool search`, as `on_retrieve_secret` of `tool/secret-tool.c`
/// of libsecret writes it: the item on stdout, its attributes on stderr.
///
/// A value of this helper holds no single quote, because a `/bin/sh` script quotes
/// its argument with one.
fn item(path: &str, value: &str, user: &str) -> String {
    format!(
        "printf '%s\\n' '[{path}]'\n\
         printf '%s\\n' 'label = keyring:{user}@horizon-remote-browser'\n\
         printf 'secret = %s\\n' '{value}'\n\
         printf '%s\\n' 'created = 2026-01-02 03:04:05'\n\
         printf '%s\\n' 'modified = 2026-01-02 03:04:05'\n\
         printf '%s\\n' 'attribute.service = horizon-remote-browser' >&2\n\
         printf '%s\\n' 'attribute.username = {user}' >&2\n"
    )
}

/// A `/bin/sh` script of `body`.
fn script(body: &str) -> String {
    format!("#!/bin/sh\n{body}")
}

fn bound() -> Bound {
    Bound::WholeImport(Duration::from_secs(10))
}

/// The value of every entry of one reading, as text, with its account.
fn answered(entries: &[super::super::readers::KeyringEntry]) -> Vec<(String, Vec<u8>)> {
    entries
        .iter()
        .map(|entry| (entry.user.clone(), entry.value.to_vec()))
        .collect()
}

#[test]
fn two_entries_of_the_secret_service_keep_their_accounts_and_values() {
    let stand_ins = StandIns::new();
    let program = stand_ins.program(
        "secret-tool",
        &script(&format!(
            "{}{}",
            item(
                "/org/freedesktop/secrets/collection/login/7",
                "alice-value",
                "https://grid.example.net|remote-browser/grid/user"
            ),
            item(
                "/org/freedesktop/secrets/collection/login/8",
                "token-value",
                "https://grid.example.net|remote-browser/grid/key"
            )
        )),
    );
    let entries = secret_tool::read(&program, &stand_ins.environment(), bound()).expect("two entries");

    assert_eq!(
        answered(&entries),
        vec![
            (
                "https://grid.example.net|remote-browser/grid/user".to_string(),
                b"alice-value".to_vec()
            ),
            (
                "https://grid.example.net|remote-browser/grid/key".to_string(),
                b"token-value".to_vec()
            ),
        ]
    );
}

#[test]
fn a_value_with_spaces_equals_and_non_ascii_is_kept_byte_for_byte() {
    let stand_ins = StandIns::new();
    let value = "  a=b  ·clé·  ";
    let program = stand_ins.program(
        "secret-tool",
        &script(&item(
            "/org/freedesktop/secrets/collection/login/9",
            value,
            "https://grid.example.net|remote-browser/grid/key",
        )),
    );
    let entries = secret_tool::read(&program, &stand_ins.environment(), bound()).expect("one entry");

    assert_eq!(
        &*entries[0].value,
        value.as_bytes(),
        "every byte of the value survives the reading"
    );
}

#[test]
fn a_value_with_a_line_break_is_one_typed_refusal() {
    let stand_ins = StandIns::new();
    // The format has no escape for a value that holds a newline: the tool would
    // write it as a line of its own.
    let body = item(
        "/org/freedesktop/secrets/collection/login/9",
        "first",
        "https://grid.example.net|remote-browser/grid/key",
    )
    .replace(
        "printf 'secret = %s\\n' 'first'",
        "printf 'secret = %s\\nsecond\\n' 'first'",
    );
    let program = stand_ins.program("secret-tool", &script(&body));

    assert_eq!(
        secret_tool::read(&program, &stand_ins.environment(), bound()).err(),
        Some(KeyringImportError::Unreadable)
    );
}

#[test]
fn a_failure_of_the_program_is_one_typed_refusal_without_its_output() {
    let stand_ins = StandIns::new();
    let program = stand_ins.program(
        "secret-tool",
        &script("printf '%s\\n' 'a value of the keyring' >&2\nexit 3\n"),
    );

    let error = secret_tool::read(&program, &stand_ins.environment(), bound()).expect_err("a refusal");
    assert_eq!(
        error,
        KeyringImportError::ProgramFailed {
            program: "secret-tool",
            code: 3
        }
    );
    assert!(
        !format!("{error} {error:?}").contains("a value of the keyring"),
        "the output of the program reaches no text: {error}"
    );
}

#[test]
fn a_locked_keyring_names_the_lock() {
    let stand_ins = StandIns::new();
    let program = stand_ins.program(
        "secret-tool",
        &script(
            "printf '%s\\n' 'secret-tool: GDBus.Error:org.freedesktop.Secret.Error.IsLocked: The secret \
             collection was not unlocked' >&2\nexit 1\n",
        ),
    );

    assert_eq!(
        secret_tool::read(&program, &stand_ins.environment(), bound()).err(),
        Some(KeyringImportError::Locked)
    );
}

#[test]
fn a_search_that_passes_the_bound_is_ended_and_its_pid_is_gone() {
    let stand_ins = StandIns::new();
    let pid_file = stand_ins.file("pid");
    let program = stand_ins.program(
        "secret-tool",
        &script(&format!(
            "printf '%s\\n' \"$$\" > '{}'\nexec /bin/sleep 30\n",
            pid_file.display()
        )),
    );
    let started = Instant::now();

    let error = secret_tool::read(
        &program,
        &stand_ins.environment(),
        Bound::WholeImport(Duration::from_millis(400)),
    )
    .expect_err("a refusal");

    assert!(error == KeyringImportError::TimedOut { seconds: 0 }, "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the bound of the import ends the program, it does not wait for it"
    );
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .expect("the stand-in wrote its pid")
        .trim()
        .parse()
        .expect("a pid");
    let pid = Pid::from_raw(i32::try_from(pid).expect("a pid")).expect("a pid");
    assert_eq!(
        test_kill_process(pid),
        Err(rustix::io::Errno::SRCH),
        "the child of this import is gone, so the bound ended this pid"
    );
}

#[test]
fn the_python_reading_answers_the_json_document() {
    let stand_ins = StandIns::new();
    // No `secret-tool` in this folder, so the reading of the Secret Service takes
    // the binding of libsecret through `python3`.
    stand_ins.program(
        "python3",
        &script(
            "printf '%s' '{\"entries\":[{\"attributes\":{\"service\":\"horizon-remote-browser\",\"username\":\
             \"https://grid.example.net|remote-browser/grid/key\"},\"secret\":\"c3ludGhldGljLXZhbHVl\"}]}'\n",
        ),
    );

    let entries = platform::secret_service(&stand_ins.environment(), bound()).expect("one entry");

    assert_eq!(
        answered(&entries),
        vec![(
            "https://grid.example.net|remote-browser/grid/key".to_string(),
            b"synthetic-value".to_vec()
        )]
    );
}

#[test]
fn a_python_without_the_binding_names_the_package() {
    let stand_ins = StandIns::new();
    stand_ins.program(
        "python3",
        &script(
            "printf '%s\\n' 'Traceback (most recent call last):' >&2\n\
             printf '%s\\n' \"ModuleNotFoundError: No module named 'gi'\" >&2\nexit 1\n",
        ),
    );

    assert_eq!(
        platform::secret_service(&stand_ins.environment(), bound()).err(),
        Some(KeyringImportError::ToolMissing)
    );
}

#[test]
fn a_computer_with_neither_program_names_the_package() {
    let stand_ins = StandIns::new();

    let error = platform::secret_service(&stand_ins.environment(), bound()).expect_err("a refusal");
    assert_eq!(error, KeyringImportError::ToolMissing);
    assert!(
        error.to_string().contains("libsecret-tools"),
        "the line names the package that reads a keyring: {error}"
    );
}

#[test]
fn the_python_reading_of_a_folder_without_python_names_the_package() {
    let stand_ins = StandIns::new();
    assert_eq!(
        libsecret::read(&stand_ins.environment(), bound()).err(),
        Some(KeyringImportError::ToolMissing)
    );
}

#[test]
fn the_keychain_answers_a_value() {
    let stand_ins = StandIns::new();
    let program = stand_ins.program("security", &script("printf '%s\\n' 'synthetic-token'\n"));

    let value = security::read_with(
        &program,
        &stand_ins.environment(),
        "https://grid.example.net|remote-browser/grid/key",
        bound(),
    )
    .expect("one reading");

    assert_eq!(value.as_deref().map(Vec::as_slice), Some(&b"synthetic-token"[..]));
}

#[test]
fn the_keychain_answers_no_item_with_status_44() {
    let stand_ins = StandIns::new();
    let program = stand_ins.program("security", &script("exit 44\n"));

    let value = security::read_with(&program, &stand_ins.environment(), "an-account", bound())
        .expect("no item is not a failure");

    assert!(value.is_none(), "this computer holds no value under that account");
}

#[test]
fn the_keychain_answers_a_failure_with_its_code() {
    let stand_ins = StandIns::new();
    let program = stand_ins.program(
        "security",
        &script("printf '%s\\n' 'security: SecKeychainSearchCopyNext: The user name or passphrase' >&2\nexit 51\n"),
    );

    let error = security::read_with(&program, &stand_ins.environment(), "an-account", bound()).expect_err("a refusal");

    assert_eq!(
        error,
        KeyringImportError::ProgramFailed {
            program: "security",
            code: 51
        }
    );
}

#[test]
fn the_program_of_a_reading_is_found_in_the_private_path_only() {
    let stand_ins = StandIns::new();
    stand_ins.program("security", &script("exit 44\n"));

    let environment: ProgramEnvironment = stand_ins.environment();
    assert!(environment.find("security").is_some());
    assert!(
        environment.find("secret-tool").is_none(),
        "the private path holds no program that this test did not write"
    );
    assert!(found(&stand_ins, "security").exists());
}
