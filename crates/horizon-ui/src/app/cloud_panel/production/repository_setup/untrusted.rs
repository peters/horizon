//! Reading what the setup agent can write: the status file and `.horizon`. Each file is
//! opened without following a link and without waiting on a pipe or a device, must be a
//! regular file, and is read only up to a limit; a walk of `.horizon` refuses a linked
//! root and anything that is not a file, folder or link. So nothing the agent leaves in
//! their place makes Horizon block, loop or run out of memory.
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// The largest file `.horizon` may hold; a Dockerfile or cloud.yml is a few kilobytes.
pub(super) const SETUP_FILE_LIMIT: u64 = 16 * 1024 * 1024;

/// The most entries `.horizon` may hold.
const SETUP_ENTRY_LIMIT: usize = 4096;

/// Reads the regular file at `path`, at most `limit` bytes.
pub(super) fn read_regular(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, libc::O_NOFOLLOW | libc::O_NONBLOCK);
    #[cfg(not(unix))]
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(std::io::Error::other("is a link"));
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::other("is not a regular file"));
    }
    if metadata.len() > limit {
        return Err(std::io::Error::other(format!("is larger than {limit} bytes")));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(std::io::Error::other(format!("is larger than {limit} bytes")));
    }
    Ok(bytes)
}

/// One entry of `.horizon`.
#[derive(Debug)]
pub(super) enum Entry {
    Folder,
    File { executable: bool },
    Link(PathBuf),
}

/// The entries of the repository's `.horizon`, sorted by their path relative to the
/// repository with `/` between parts. Links are listed, never followed.
pub(super) fn setup_entries(repository: &Path) -> Result<Vec<(String, PathBuf, Entry)>, String> {
    let root = repository.join(".horizon");
    let kind = std::fs::symlink_metadata(&root).map_err(|error| format!(".horizon: {error}"))?;
    if !kind.is_dir() {
        return Err(".horizon must be a folder, not a link or a file".to_owned());
    }
    let mut entries = Vec::new();
    let mut pending = vec![root];
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            let relative = path
                .strip_prefix(repository)
                .map_err(|error| error.to_string())?
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| format!("{relative}: {error}"))?;
            let kind = metadata.file_type();
            let entry = if kind.is_symlink() {
                Entry::Link(std::fs::read_link(&path).map_err(|error| format!("{relative}: {error}"))?)
            } else if kind.is_dir() {
                pending.push(path.clone());
                Entry::Folder
            } else if kind.is_file() {
                Entry::File {
                    executable: executable(&metadata),
                }
            } else {
                return Err(format!("{relative} is not a file, folder or link"));
            };
            entries.push((relative, path, entry));
            if entries.len() > SETUP_ENTRY_LIMIT {
                return Err(format!(".horizon holds more than {SETUP_ENTRY_LIMIT} entries"));
            }
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries)
}

#[cfg(unix)]
fn executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_: &std::fs::Metadata) -> bool {
    false
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_pipe_a_link_or_a_large_file_is_refused_without_waiting() {
        let folder = tempfile::tempdir().unwrap();
        let pipe = folder.path().join("status.json");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&pipe)
                .status()
                .unwrap()
                .success()
        );
        assert!(read_regular(&pipe, 1024).is_err(), "a pipe is refused at once");
        let large = folder.path().join("large");
        std::fs::write(&large, vec![b'x'; 2048]).unwrap();
        assert!(read_regular(&large, 1024).is_err());
        let link = folder.path().join("link");
        std::os::unix::fs::symlink(&large, &link).unwrap();
        assert!(read_regular(&link, 4096).is_err(), "a link is not followed");
        assert_eq!(read_regular(&large, 4096).unwrap().len(), 2048);
    }

    #[test]
    fn a_linked_setup_or_a_pipe_in_it_is_refused() {
        let repository = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), repository.path().join(".horizon")).unwrap();
        assert!(setup_entries(repository.path()).unwrap_err().contains("not a link"));
        std::fs::remove_file(repository.path().join(".horizon")).unwrap();
        std::fs::create_dir_all(repository.path().join(".horizon/scripts")).unwrap();
        std::fs::write(repository.path().join(".horizon/cloud.yml"), "version: 1\n").unwrap();
        let names: Vec<String> = setup_entries(repository.path())
            .unwrap()
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        assert_eq!(names, [".horizon/cloud.yml", ".horizon/scripts"]);
        let pipe = repository.path().join(".horizon/scripts/pipe");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&pipe)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            setup_entries(repository.path())
                .unwrap_err()
                .contains("not a file, folder or link")
        );
    }
}
