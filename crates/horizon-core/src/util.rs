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
/// # Errors
///
/// Returns the error of the write or of the permissions of the file.
#[doc(hidden)]
pub fn stand_in_program_in(folder: &Path, name: &str, body: &str) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;

    let path = folder.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
    let mut permissions = std::fs::metadata(&path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions)?;
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
    use super::{stand_in_program, stand_in_program_in, truncate_chars};

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
