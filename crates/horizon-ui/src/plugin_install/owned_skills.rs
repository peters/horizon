//! Whether a skill directory holds exactly the files that Horizon bundles.

use std::io;
use std::path::Path;

use super::EmbeddedFile;

/// Whether `dir` holds exactly the bundled `files` and nothing else: no
/// partial, extra or modified content that Horizon must not take over.
pub(super) fn validate_files(dir: &Path, files: &[EmbeddedFile]) -> io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(unowned()),
    }
    let mut found = 0;
    validate_entries(dir, dir, files, &mut found)?;
    if found == files.len() { Ok(()) } else { Err(unowned()) }
}

fn validate_entries(root: &Path, dir: &Path, files: &[EmbeddedFile], found: &mut usize) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(io::Error::other)?;
        let kind = entry.file_type()?;
        if kind.is_dir()
            && files
                .iter()
                .any(|file| Path::new(file.relative_path).starts_with(relative))
        {
            validate_entries(root, &path, files, found)?;
        } else if kind.is_file() {
            let Some(file) = files.iter().find(|file| relative == Path::new(file.relative_path)) else {
                return Err(unowned());
            };
            if std::fs::read_to_string(&path)? != file.content {
                return Err(unowned());
            }
            *found += 1;
        } else {
            return Err(unowned());
        }
    }
    Ok(())
}

fn unowned() -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Horizon skill differs from bundled files; preserving existing content",
    )
}

#[cfg(test)]
mod tests {
    use super::{EmbeddedFile, validate_files};

    const FILES: &[EmbeddedFile] = &[
        EmbeddedFile {
            relative_path: "SKILL.md",
            content: "skill",
        },
        EmbeddedFile {
            relative_path: "references/notes.md",
            content: "notes",
        },
    ];

    fn write(root: &std::path::Path, relative: &str, content: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("directory");
        std::fs::write(path, content).expect("file");
    }

    #[test]
    fn a_missing_directory_is_not_yet_owned() {
        let temp = tempfile::tempdir().expect("temp");
        assert!(validate_files(&temp.path().join("absent"), FILES).is_ok());
    }

    #[test]
    fn only_the_exact_bundled_tree_validates() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = temp.path().join("horizon-device");
        write(&dir, "SKILL.md", "skill");
        assert!(validate_files(&dir, FILES).is_err(), "a nested file is missing");

        write(&dir, "references/notes.md", "notes");
        assert!(validate_files(&dir, FILES).is_ok());

        write(&dir, "references/notes.md", "changed");
        assert!(validate_files(&dir, FILES).is_err(), "modified content is user content");

        write(&dir, "references/notes.md", "notes");
        write(&dir, "extra.md", "extra");
        assert!(validate_files(&dir, FILES).is_err(), "an extra file is user content");
    }
}
