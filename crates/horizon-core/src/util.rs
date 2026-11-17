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
/// either system runs it on the computer that runs the tests. A body that starts
/// with `#!` keeps its own first line, so a test of a `bash` program writes one.
///
/// No descriptor of the script is open in this process at any time, so a fork of
/// another test thread cannot copy one, and both this process and any production
/// code under test can execute it at once. A caller that only needs a command
/// line around the script writes it with [`stand_in_shell_in`] instead.
///
/// # Errors
///
/// Returns the error of the write or of the permissions of the file.
#[doc(hidden)]
pub fn stand_in_program_in(folder: &Path, name: &str, body: &str) -> std::io::Result<PathBuf> {
    write_stand_in(folder, name, body, 0o755)
}

/// Starts one program that a test wrote, and starts it again while the kernel
/// answers `ExecutableFileBusy`.
///
/// The tests of this workspace run in parallel threads of one process. A thread
/// that writes a script holds it open for writing for a few microseconds, and a
/// fork of another thread in that window copies the descriptor: the child holds
/// the script open for writing until its own `exec`, and the kernel then refuses
/// to execute that script with `ETXTBSY` (`Text file busy`). No write can remove
/// that window, so a test that starts a script of its own retries the start
/// instead. Every other answer of a start is the answer of the first try.
///
/// `start` performs one try and builds its command line again for each try, so a
/// retry can never reuse a command that a child already took.
///
/// # Errors
///
/// Returns the error of the last try: the error of `start`, or
/// `ExecutableFileBusy` when the attempts end first.
#[doc(hidden)]
pub fn spawn_stand_in<T>(mut start: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut attempt = 1;
    loop {
        match start() {
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy && attempt < STAND_IN_ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(STAND_IN_RETRY);
            }
            answer => return answer,
        }
    }
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

/// The most times that [`spawn_stand_in`] starts a script of a test, and how long it
/// waits between two tries. A fork that copied the descriptor of the script ends its
/// own `exec` within a millisecond, so the bounded time is the whole window of a
/// script that a busy test process wrote.
const STAND_IN_ATTEMPTS: usize = 200;
const STAND_IN_RETRY: std::time::Duration = std::time::Duration::from_millis(10);

/// Writes one script of a test and answers its path.
///
/// A `/bin/sh` of the system writes the file. A write from this process keeps a
/// descriptor of the script open for a few microseconds, and a fork of another
/// test thread in that window copies it: the child then holds the script open for
/// writing until its own `exec`, and the kernel refuses to execute the script with
/// `ETXTBSY` (`Text file busy`). The descriptor of a child belongs to that child,
/// so nothing of this process can hold the script and the window does not exist.
///
/// A body that starts with `#!` keeps its own first line, so a test of a `bash`
/// program writes the shell that its script needs.
fn write_stand_in(folder: &Path, name: &str, body: &str, mode: u32) -> std::io::Result<PathBuf> {
    use std::io::Write as _;

    let path = folder.join(name);
    let first_line = if body.starts_with("#!") { "" } else { "#!/bin/sh\n" };
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "cat > \"$1\" && chmod \"$2\" \"$1\"", "sh"])
        .arg(&path)
        .arg(format!("{mode:o}"))
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    let written = match child.stdin.take() {
        Some(mut stdin) => stdin.write_all(format!("{first_line}{body}\n").as_bytes()),
        None => Ok(()),
    };
    let status = child.wait()?;
    written?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "the shell that wrote `{}` ended with {status}",
            path.display()
        )));
    }
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

/// One `sh` word that keeps the value of the person literal: one word, with no
/// expansion.
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::{quote, spawn_stand_in, stand_in_program, stand_in_program_in, stand_in_shell_in, truncate_chars};

    #[test]
    fn quote_keeps_every_value_one_word_of_the_shell() {
        let table = [
            ("", "''"),
            ("plain", "'plain'"),
            ("two words", "'two words'"),
            ("it's", "'it'\\''s'"),
            ("a;'b", "'a;'\\''b'"),
            ("say \"hi\"", "'say \"hi\"'"),
            ("$HOME", "'$HOME'"),
            ("back`tick", "'back`tick'"),
            ("back\\slash", "'back\\slash'"),
            ("one\ntwo", "'one\ntwo'"),
            ("~", "'~'"),
            ("*", "'*'"),
            ("naïve-ærlig", "'naïve-ærlig'"),
            ("-x", "'-x'"),
        ];
        for (input, expected) in table {
            assert_eq!(quote(input), expected, "the word of {input:?}");
        }
    }

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

        let output = spawn_stand_in(|| std::process::Command::new(&program).env_clear().output())
            .expect("the stand-in program runs");

        assert_eq!(output.status.code(), Some(0));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "yes\n");
    }

    /// A test thread of this process writes a script while another thread forks, and the
    /// child of that fork then holds the script open for writing: Linux refuses to
    /// execute it with `ETXTBSY` until the child ends its own `exec`. This test holds the
    /// descriptor itself, which is the same refusal on Linux, and proves that the helper
    /// starts the program on both systems.
    ///
    /// macOS runs a program that a descriptor still writes, so the refusal below is a
    /// rule of Linux and the program of this test starts under the held descriptor there.
    /// The helper starts the program on both systems, which is the part that both prove.
    #[test]
    fn spawn_stand_in_starts_a_program_that_a_fork_still_holds_open() {
        let folder = tempfile::tempdir().expect("folder of a stand-in program");
        let program =
            stand_in_program_in(folder.path(), "answer", "printf 'yes\\n'").expect("the stand-in program of the test");
        let held = std::fs::OpenOptions::new()
            .write(true)
            .open(&program)
            .expect("an open descriptor of the script");
        #[cfg(target_os = "linux")]
        {
            let refused = std::process::Command::new(&program)
                .env_clear()
                .output()
                .expect_err("the kernel of Linux refuses a program that a descriptor still writes");
            assert_eq!(
                refused.kind(),
                std::io::ErrorKind::ExecutableFileBusy,
                "the refusal of this test is the refusal of a fork: {refused}"
            );
        }

        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            drop(held);
        });
        let output = spawn_stand_in(|| std::process::Command::new(&program).env_clear().output())
            .expect("the helper starts the program after the window");
        release.join().expect("the release of the descriptor");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "yes\n");
    }

    #[test]
    fn a_body_with_its_own_first_line_keeps_the_shell_it_names() {
        let folder = tempfile::tempdir().expect("folder of a stand-in program");
        let program = stand_in_program_in(folder.path(), "answer", "#!/bin/sh\nprintf 'own\\n'")
            .expect("the stand-in program of the test");
        let body = std::fs::read_to_string(&program).expect("the script of the test");

        assert!(
            body.starts_with("#!/bin/sh\n"),
            "the body keeps its own shell: {body:?}"
        );
        assert_eq!(body.matches("#!").count(), 1, "one first line, and not two: {body:?}");
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
