//! Small shared helpers with no more specific module.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// A program that ends at once with success, for a test that must start no
/// shell, no ssh and no kill command.
///
/// The path exists on Linux and on macOS: macOS keeps `true` in `/usr/bin`, and
/// a smaller Linux keeps it in `/bin`. A test that names one of those paths
/// passes on one system and fails on the other, so it asks for this program.
#[doc(hidden)]
#[must_use]
pub fn stand_in_program() -> &'static str {
    ["/usr/bin/true", "/bin/true"]
        .into_iter()
        .find(|candidate| std::path::Path::new(candidate).exists())
        .unwrap_or("/usr/bin/true")
}

/// Writes `body` as the program `name` of `folder`, for a test that answers with
/// a stand-in program instead of a program of the system.
///
/// The file is a shell script with the permissions of a program, so the reader of
/// either system runs it on the computer that runs the tests.
///
/// The script is closed before this function answers. A caller that needs both
/// the script and a command line around it writes it with
/// [`stand_in_shell_in`] instead, because a file that this process still writes,
/// or that a fork of another test still holds open for writing, refuses an
/// `exec` with `ETXTBSY`.
///
/// # Errors
///
/// Returns the error of the write or of the permissions of the file.
#[doc(hidden)]
pub fn stand_in_program_in(folder: &Path, name: &str, body: &str) -> std::io::Result<PathBuf> {
    write_stand_in(folder, name, body, 0o755)
}

/// Writes `body` as the program `name` of `folder` and answers the command line
/// that runs it: `/bin/sh <path>` with the path quoted for the shell.
///
/// A test whose command line names a stand-in script uses this form, and the
/// kernel then never executes the script file: `/bin/sh` reads it as input. A
/// script file that another test thread forks over cannot refuse a read, so a
/// parallel run of these tests has no `ETXTBSY`. The file is not executable,
/// because nothing runs it as a program.
///
/// # Errors
///
/// Returns the error of the write of the script.
#[doc(hidden)]
pub fn stand_in_shell_in(folder: &Path, name: &str, body: &str) -> std::io::Result<String> {
    let path = write_stand_in(folder, name, body, 0o644)?;
    Ok(format!("/bin/sh '{}'", path.display()))
}

/// Writes one `/bin/sh` script of a test, closes it, and answers its path.
///
/// The file is opened once with `O_CLOEXEC`, which the standard library sets on
/// every file that it opens, so a program that this process starts never holds
/// the descriptor after its `exec`. A fork that copies the descriptor needs the
/// script within microseconds of the write, so the callers that read the script
/// instead of executing it, as [`stand_in_shell_in`] does, remove that window.
fn write_stand_in(folder: &Path, name: &str, body: &str, mode: u32) -> std::io::Result<PathBuf> {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    let path = folder.join(name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)?;
    let written = file
        .write_all(format!("#!/bin/sh\n{body}\n").as_bytes())
        .and_then(|()| file.set_permissions(std::fs::Permissions::from_mode(mode)));
    drop(file);
    written?;
    Ok(path)
}

/// Caps `value` at `max_chars` characters, ending a truncated result with an
/// ellipsis that counts toward the budget. Borrows when nothing is cut.
///
/// The cut is character-based: byte-indexed slicing would panic whenever a
/// multi-byte character straddling the cut point. This is the canonical
/// implementation; the UI re-exports it as `crate::text::truncate_chars`.
#[must_use]
pub fn truncate_chars(value: &str, max_chars: usize) -> Cow<'_, str> {
    if max_chars == 0 {
        return Cow::Borrowed(if value.is_empty() { value } else { "" });
    }

    let mut ellipsis_at = 0;
    for (char_index, (byte_index, _)) in value.char_indices().enumerate() {
        if char_index + 1 == max_chars {
            ellipsis_at = byte_index;
        } else if char_index + 1 > max_chars {
            let mut truncated = String::with_capacity(ellipsis_at + '…'.len_utf8());
            truncated.push_str(&value[..ellipsis_at]);
            truncated.push('…');
            return Cow::Owned(truncated);
        }
    }

    Cow::Borrowed(value)
}

#[cfg(test)]
mod tests {
    use super::{stand_in_program, stand_in_program_in, stand_in_shell_in, truncate_chars};

    #[test]
    fn stand_in_program_names_a_program_that_exists() {
        assert!(
            std::path::Path::new(stand_in_program()).exists(),
            "the stand-in program of a test exists on this system"
        );
    }

    #[test]
    fn stand_in_program_in_writes_a_program_that_runs() {
        let folder = tempfile::tempdir().expect("folder of a stand-in program");
        let program =
            stand_in_program_in(folder.path(), "answer", "printf 'yes\\n'").expect("the stand-in program of the test");

        let output = std::process::Command::new(&program)
            .env_clear()
            .output()
            .expect("the stand-in program runs");

        assert_eq!(output.status.code(), Some(0));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "yes\n");
    }

    #[test]
    fn stand_in_shell_in_writes_a_script_that_its_shell_reads() {
        use std::os::unix::fs::PermissionsExt as _;

        let folder = tempfile::tempdir().expect("folder of a stand-in script");
        let command = stand_in_shell_in(folder.path(), "answer", "printf 'yes %s\\n' \"$1\"")
            .expect("the stand-in script of the test");
        let script = folder.path().join("answer");

        assert_eq!(
            script.metadata().expect("the script").permissions().mode() & 0o111,
            0,
            "nothing executes the script file, so it is not executable"
        );
        let line = crate::speech::command::CommandLine::parse(&format!("{command} {{file}}"))
            .expect("a command line of the script");
        assert_eq!(line.program(), "/bin/sh", "the shell is the program of the line");

        let output = std::process::Command::new(line.program())
            .args(line.arguments_for(std::path::Path::new("one argument")))
            .env_clear()
            .output()
            .expect("the shell reads the script");
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "yes one argument\n");
    }

    #[test]
    fn truncate_chars_keeps_short_values_untouched() {
        assert_eq!(truncate_chars("build", 8), "build");
        assert_eq!(truncate_chars("exactly8", 8), "exactly8");
    }

    #[test]
    fn truncate_chars_caps_at_budget_including_ellipsis() {
        let truncated = truncate_chars("docker compose up", 8);

        assert_eq!(truncated, "docker …");
        assert_eq!(truncated.chars().count(), 8);
    }

    #[test]
    fn truncate_chars_honors_tiny_budgets() {
        assert_eq!(truncate_chars("abc", 0), "");
        assert_eq!(truncate_chars("abc", 1), "…");
        assert_eq!(truncate_chars("", 0), "");
    }

    #[test]
    fn truncate_chars_counts_characters_not_bytes() {
        assert_eq!(truncate_chars("blåbærsyltetøy", 6), "blåbæ…");
    }
}
