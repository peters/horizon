//! Reading what the setup agent can write: the status file and `.horizon`. Each file is
//! opened without following a link and without waiting on a pipe or a device, must be a
//! regular file, and is read only up to a limit. `.horizon` is read in one walk from
//! opened folder handles, never by path again, so the agent cannot swap a link in
//! between a check and a read; the walk refuses a linked root and anything that is not
//! a file, folder or link. So nothing the agent leaves makes Horizon block, follow a
//! link out of the repository or run out of memory.
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// The largest file `.horizon` may hold; a Dockerfile or cloud.yml is a few kilobytes.
pub(super) const SETUP_FILE_LIMIT: u64 = 16 * 1024 * 1024;

/// The most `.horizon` may hold in all its files, names and link targets together.
const SETUP_TOTAL_LIMIT: u64 = 64 * 1024 * 1024;

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
    read_open(options.open(path)?, limit)
}

/// Reads an opened file, which must be a regular one, at most `limit` bytes.
fn read_open(file: std::fs::File, limit: u64) -> std::io::Result<Vec<u8>> {
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

/// One entry of `.horizon`, with a file's content as the walk read it.
#[derive(Debug)]
pub(super) enum Entry {
    Folder,
    File { executable: bool, content: Vec<u8> },
    Link(PathBuf),
}

/// A link's target as git stores it: the raw bytes on Unix, so no two targets read as
/// one; elsewhere with `/` between parts.
pub(super) fn link_bytes(target: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        target.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        target.to_string_lossy().replace('\\', "/").into_bytes()
    }
}

/// What the walk has read so far, against its limits.
#[derive(Default)]
struct Budget {
    entries: usize,
    bytes: u64,
}

/// The bytes the walk keeps for `entry` at `relative`: its path and its content or link
/// target, so long names and deep folders count against the limit as file contents do.
fn retained(relative: &str, entry: &Entry) -> u64 {
    let kept = match entry {
        Entry::File { content, .. } => content.len(),
        Entry::Link(target) => link_bytes(target).len(),
        Entry::Folder => 0,
    };
    (relative.len() + kept) as u64
}

impl Budget {
    fn add(&mut self, bytes: u64) -> Result<(), String> {
        self.entries += 1;
        self.bytes += bytes;
        if self.entries > SETUP_ENTRY_LIMIT {
            return Err(format!(".horizon holds more than {SETUP_ENTRY_LIMIT} entries"));
        }
        if self.bytes > SETUP_TOTAL_LIMIT {
            return Err(format!(".horizon holds more than {SETUP_TOTAL_LIMIT} bytes"));
        }
        Ok(())
    }
}

/// Which folder a path names: its device and inode, read without following a link at
/// its end. A run keeps its repository's, so a folder renamed and replaced, by a link or
/// by another folder, is never taken for it; none where the platform has no such ids.
pub(super) type Identity = Option<(u64, u64)>;

/// The identity of the folder at `path`, never following a link at its end.
pub(super) fn identity(path: &Path) -> Identity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        std::fs::symlink_metadata(path)
            .ok()
            .map(|metadata| (metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// What a read says when the repository folder is no longer the run's.
pub(super) const OTHER_REPOSITORY: &str = "the repository folder is not the one the setup started in";

/// The entries of the repository's `.horizon`, sorted by their path relative to the
/// repository with `/` between parts. Links are listed, never followed. With an
/// `expected` identity the repository folder must still be that folder.
#[cfg(unix)]
pub(super) fn setup_entries(repository: &Path, expected: Identity) -> Result<Vec<(String, Entry)>, String> {
    use rustix::fs::{AtFlags, CWD, Dir, FileType, Mode, OFlags, openat, readlinkat, statat};
    use std::os::unix::ffi::OsStringExt as _;
    let folder_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    // The repository folder itself is opened without following a link at its end, and
    // the walk then goes on from that handle, so it reads the folder it checked.
    let repository_folder = openat(CWD, repository, folder_flags, Mode::empty())
        .map_err(|error| format!("{}: {error}", repository.display()))?;
    if let Some((device, inode)) = expected {
        let stat =
            rustix::fs::fstat(&repository_folder).map_err(|error| format!("{}: {error}", repository.display()))?;
        // The fields are u64 on Linux but not on every Unix, such as macOS.
        #[allow(clippy::useless_conversion)]
        let found = (u64::try_from(stat.st_dev).ok(), u64::try_from(stat.st_ino).ok());
        if found != (Some(device), Some(inode)) {
            return Err(OTHER_REPOSITORY.to_owned());
        }
    }
    let root = openat(&repository_folder, ".horizon", folder_flags, Mode::empty()).map_err(|error| {
        if matches!(error, rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR) {
            ".horizon must be a folder, not a link or a file".to_owned()
        } else {
            format!(".horizon: {error}")
        }
    })?;
    let mut entries = Vec::new();
    let mut budget = Budget::default();
    let mut pending = vec![(".horizon".to_owned(), root)];
    while let Some((prefix, folder)) = pending.pop() {
        let mut listing = Dir::read_from(&folder).map_err(|error| format!("{prefix}: {error}"))?;
        while let Some(item) = listing.read() {
            let item = item.map_err(|error| format!("{prefix}: {error}"))?;
            let name = item.file_name();
            if matches!(name.to_bytes(), b"." | b"..") {
                continue;
            }
            let Ok(text) = std::str::from_utf8(name.to_bytes()) else {
                return Err(format!("{prefix} holds a name that is not UTF-8"));
            };
            let relative = format!("{prefix}/{text}");
            let failed = |error: rustix::io::Errno| format!("{relative}: {error}");
            let stat = statat(&folder, name, AtFlags::SYMLINK_NOFOLLOW).map_err(failed)?;
            let entry = match FileType::from_raw_mode(stat.st_mode) {
                FileType::Symlink => {
                    let target = readlinkat(&folder, name, Vec::new()).map_err(failed)?;
                    Entry::Link(std::ffi::OsString::from_vec(target.into_bytes()).into())
                }
                FileType::Directory => {
                    // Opened without following, so a link swapped in fails here.
                    let child = openat(&folder, name, folder_flags, Mode::empty()).map_err(failed)?;
                    pending.push((relative.clone(), child));
                    Entry::Folder
                }
                FileType::RegularFile => {
                    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
                    let file = openat(&folder, name, flags, Mode::empty()).map_err(failed)?;
                    // The mode of the file opened, so content and mode are of one file
                    // even if another was swapped in after the look above.
                    let opened = rustix::fs::fstat(&file).map_err(failed)?;
                    if FileType::from_raw_mode(opened.st_mode) != FileType::RegularFile {
                        return Err(format!("{relative} changed while Horizon read it"));
                    }
                    let content = read_open(std::fs::File::from(file), SETUP_FILE_LIMIT)
                        .map_err(|error| format!("{relative}: {error}"))?;
                    Entry::File {
                        executable: Mode::from_raw_mode(opened.st_mode)
                            .intersects(Mode::XUSR | Mode::XGRP | Mode::XOTH),
                        content,
                    }
                }
                _ => return Err(format!("{relative} is not a file, folder or link")),
            };
            budget.add(retained(&relative, &entry))?;
            entries.push((relative, entry));
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries)
}

/// The entries of the repository's `.horizon`, by path; without folder handles this is
/// only as safe as the platform's path checks.
#[cfg(not(unix))]
pub(super) fn setup_entries(repository: &Path, _expected: Identity) -> Result<Vec<(String, Entry)>, String> {
    let root = repository.join(".horizon");
    let kind = std::fs::symlink_metadata(&root).map_err(|error| format!(".horizon: {error}"))?;
    if !kind.is_dir() {
        return Err(".horizon must be a folder, not a link or a file".to_owned());
    }
    let mut entries = Vec::new();
    let mut budget = Budget::default();
    let mut pending = vec![(".horizon".to_owned(), root)];
    while let Some((prefix, folder)) = pending.pop() {
        for item in std::fs::read_dir(&folder).map_err(|error| format!("{prefix}: {error}"))? {
            let item = item.map_err(|error| format!("{prefix}: {error}"))?;
            let relative = format!("{prefix}/{}", item.file_name().to_string_lossy());
            let path = item.path();
            let kind = std::fs::symlink_metadata(&path)
                .map_err(|error| format!("{relative}: {error}"))?
                .file_type();
            let entry = if kind.is_symlink() {
                Entry::Link(std::fs::read_link(&path).map_err(|error| format!("{relative}: {error}"))?)
            } else if kind.is_dir() {
                pending.push((relative.clone(), path));
                Entry::Folder
            } else if kind.is_file() {
                let content = read_regular(&path, SETUP_FILE_LIMIT).map_err(|error| format!("{relative}: {error}"))?;
                Entry::File {
                    executable: false,
                    content,
                }
            } else {
                return Err(format!("{relative} is not a file, folder or link"));
            };
            budget.add(retained(&relative, &entry))?;
            entries.push((relative, entry));
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn names_and_link_targets_count_against_the_limit() {
        let target = "t".repeat(4000);
        assert_eq!(
            retained(".horizon/link", &Entry::Link(PathBuf::from(&target))),
            (".horizon/link".len() + target.len()) as u64
        );
        assert_eq!(retained(".horizon/deep", &Entry::Folder), ".horizon/deep".len() as u64);
        let mut budget = Budget::default();
        let long = format!(".horizon/{}", "d/".repeat(40_000));
        let refused = (0..SETUP_ENTRY_LIMIT).find_map(|_| budget.add(retained(&long, &Entry::Folder)).err());
        assert!(
            refused.is_some_and(|error| error.contains("bytes")),
            "deep long names hit the byte limit"
        );
    }

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
    fn a_repository_folder_swapped_for_another_is_refused() {
        let parent = tempfile::tempdir().unwrap();
        let repository = parent.path().join("repository");
        std::fs::create_dir_all(repository.join(".horizon")).unwrap();
        let started = identity(&repository);
        assert!(
            setup_entries(&repository, started).is_ok(),
            "the folder the run started in"
        );
        std::fs::rename(&repository, parent.path().join("moved")).unwrap();
        std::fs::create_dir_all(repository.join(".horizon")).unwrap();
        assert_eq!(
            setup_entries(&repository, started).unwrap_err(),
            OTHER_REPOSITORY,
            "another folder"
        );
        std::fs::remove_dir_all(&repository).unwrap();
        std::os::unix::fs::symlink(parent.path().join("moved"), &repository).unwrap();
        assert!(
            setup_entries(&repository, started).is_err(),
            "a link in its place is not followed"
        );
    }

    #[test]
    fn a_linked_setup_or_a_pipe_in_it_is_refused() {
        let repository = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), repository.path().join(".horizon")).unwrap();
        assert!(
            setup_entries(repository.path(), None)
                .unwrap_err()
                .contains("not a link")
        );
        std::fs::remove_file(repository.path().join(".horizon")).unwrap();
        std::fs::create_dir_all(repository.path().join(".horizon/scripts")).unwrap();
        std::fs::write(repository.path().join(".horizon/cloud.yml"), "version: 1\n").unwrap();
        let names: Vec<String> = setup_entries(repository.path(), None)
            .unwrap()
            .into_iter()
            .map(|(name, _)| name)
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
            setup_entries(repository.path(), None)
                .unwrap_err()
                .contains("not a file, folder or link")
        );
    }
}
