//! The identity of a folder that Horizon names by its path.
//!
//! A board names the folder of a workspace, the pickup list names the folder of
//! a session, and the command line names the folder of a start. Two of those
//! paths name the same folder when they resolve to the same place: a symbolic
//! link, a trailing slash and an interior `.` do not make a second folder.
//!
//! [`folder_identity`] is the one definition of that rule. A path that the
//! system can resolve answers its resolved form. A path that is gone keeps a
//! lexical form, so two names of one folder that is gone still match.

use std::path::{Component, Path, PathBuf};

/// The path that names the same folder as `path`, without a symbolic link, a
/// trailing separator or an interior `.`.
///
/// The answer is absolute. A folder that does not exist, or a path that carries
/// a part which is not text, keeps a lexical form instead: the parts fold, and
/// the folder keeps the name that the caller wrote.
#[must_use]
pub fn folder_identity(path: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return resolved;
    }
    lexical_identity(path)
}

/// Whether two paths name the same folder.
#[must_use]
pub fn same_folder(left: &Path, right: &Path) -> bool {
    folder_identity(left) == folder_identity(right)
}

/// The absolute form of `path` with `.` and repeated separators folded, and
/// with `..` kept as the caller wrote it.
///
/// A relative path resolves against the current directory, which is the folder
/// of the process.
fn lexical_identity(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |folder| folder.join(path))
    };
    let mut folded = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                folded.pop();
            }
            other => folded.push(other.as_os_str()),
        }
    }
    if folded.as_os_str().is_empty() {
        PathBuf::from(std::path::MAIN_SEPARATOR.to_string())
    } else {
        folded
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{folder_identity, same_folder};

    #[test]
    fn a_trailing_separator_and_an_interior_dot_name_the_same_folder() {
        let temp = tempfile::tempdir().expect("temp dir");
        let folder = temp.path().join("repo");
        std::fs::create_dir(&folder).expect("the folder");
        let with_separator = Path::new(&format!("{}/", folder.display())).to_path_buf();

        assert_eq!(folder_identity(&folder), folder_identity(&with_separator));
        assert!(same_folder(&folder, &with_separator));
        assert!(
            folder_identity(&folder).is_absolute() && !folder_identity(&folder).ends_with("."),
            "the resolved form of an existing folder carries no separator of its own"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_names_the_folder_of_its_target() {
        let temp = tempfile::tempdir().expect("temp dir");
        let folder = temp.path().join("repo");
        std::fs::create_dir(&folder).expect("the folder");
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&folder, &link).expect("the link");

        assert!(same_folder(&link, &folder), "the link names the folder of its target");
        assert!(
            !same_folder(&link, &temp.path().join("other")),
            "another folder stays another"
        );
    }

    #[test]
    fn a_folder_that_is_gone_keeps_a_lexical_identity() {
        let temp = tempfile::tempdir().expect("temp dir");
        let gone = temp.path().join("gone");
        let gone_with_separator = Path::new(&format!("{}/", gone.display())).to_path_buf();

        assert_eq!(folder_identity(&gone), folder_identity(&gone_with_separator));
        assert!(!same_folder(&gone, &temp.path().join("elsewhere")));
        assert!(folder_identity(&gone).is_absolute(), "an identity is absolute");
    }

    #[test]
    fn a_relative_path_resolves_against_the_current_directory() {
        let relative = Path::new("a-relative-folder");
        let current = std::env::current_dir().expect("the current dir");

        assert_eq!(folder_identity(relative), current.join(relative));
        assert_eq!(
            folder_identity(Path::new("a-relative-folder/")),
            current.join("a-relative-folder"),
            "a trailing separator folds in the lexical form too"
        );
    }
}
