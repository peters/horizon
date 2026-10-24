//! Reads an uncommitted file without following a link, and lays the confirmed files over
//! a committed snapshot, checking each file's content against its digest as it is copied.
use super::{
    super::{Error, Result},
    Change, Content, MOVED, Refusal, Uncommitted, collect, hex,
};
use horizon_cloud::Build;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

const CHANGED: &str = "The uncommitted files in the image's build context changed after the rebuild was confirmed. Cancel the rebuild, then rebuild again to confirm the files as they are now";
const LINK_UNSUPPORTED: &str = "An uncommitted file in the image's build context is a link, which Horizon cannot copy; replace it with a file or add it to .gitignore before you build the image";
const BUFFER: usize = 64 * 1024;
const SNAPSHOT_LINK: &str =
    "The build snapshot has a link or a file where an uncommitted file needs a folder; try again";

/// Whether `error` says that the uncommitted files are not the confirmed ones, or changed
/// while they were copied.
#[must_use]
pub fn changed(error: &Error) -> bool {
    matches!(error, Error::Invalid(message) if *message == CHANGED)
}

/// Collects the uncommitted files of `repository` against `revision` for `build` again and
/// lays them over `snapshot`, the committed tree of `revision`, so the image builds from
/// the working copy. `confirmed` is the [`Uncommitted::hash`] a person confirmed, `None`
/// when there were no such files.
/// # Errors
/// Refuses when the files are not the confirmed ones, as [`collect`] refuses, and when a
/// file changes while it is copied.
pub fn overlay(
    repository: &Path,
    revision: &str,
    build: &Build,
    confirmed: Option<&str>,
    snapshot: &Path,
) -> Result<()> {
    let files = collect(repository, revision, build)?;
    if files.hash().as_deref() != confirmed {
        return Err(Error::Invalid(CHANGED));
    }
    files.apply(repository, snapshot)
}

impl Uncommitted {
    /// Deletes each deleted file from `snapshot`, then copies each other file from
    /// `repository` into it.
    fn apply(&self, repository: &Path, snapshot: &Path) -> Result<()> {
        let (deleted, written): (Vec<_>, Vec<_>) = self.files.iter().partition(|file| file.change == Change::Deleted);
        for file in deleted {
            let target = inside(snapshot, &file.path, false)?;
            remove(&target)?;
        }
        for file in written {
            let target = inside(snapshot, &file.path, true)?;
            remove(&target)?;
            match &file.content {
                Content::File { executable, digest } => {
                    let Found::File(reader) = find(repository, &file.path)? else {
                        return Err(Error::Invalid(CHANGED));
                    };
                    let mut reader = single(reader, &file.path)?;
                    let mut writer = fs::OpenOptions::new().write(true).create_new(true).open(&target)?;
                    if copy(&mut reader, Some(&mut writer))? != *digest {
                        return Err(Error::Invalid(CHANGED));
                    }
                    finish(&writer, *executable)?;
                }
                Content::Link { digest } => {
                    // Made from the bytes just checked, so a link swapped meanwhile is not copied.
                    let Found::Link(link) = find(repository, &file.path)? else {
                        return Err(Error::Invalid(CHANGED));
                    };
                    if hex(&Sha256::digest(&link)) != *digest {
                        return Err(Error::Invalid(CHANGED));
                    }
                    make_link(link, &target)?;
                }
                Content::Absent => return Err(Error::Invalid(CHANGED)),
            }
        }
        Ok(())
    }
}

/// What the uncommitted `path` of `repository` holds, read without following a link.
pub(super) fn content(repository: &Path, path: &str) -> Result<Content> {
    match find(repository, path)? {
        Found::Link(target) if cfg!(unix) => Ok(Content::Link {
            digest: hex(&Sha256::digest(target)),
        }),
        // Refused while the list is read, so a person never confirms a link that cannot
        // be copied.
        Found::Link(_) => Err(Error::Invalid(LINK_UNSUPPORTED)),
        Found::File(file) => {
            let mut file = single(file, path)?;
            Ok(Content::File {
                executable: executable(&file.metadata()?),
                digest: copy(&mut file, None)?,
            })
        }
        Found::Other => Err(Refusal::Special(path.to_owned()).into()),
    }
}

/// Whether the working copy has a file or a link at `path`, reached through folders only.
pub(super) fn on_disk(repository: &Path, path: &str) -> bool {
    matches!(find(repository, path), Ok(Found::File(_) | Found::Link(_)))
}

/// A working-copy entry as [`find`] reached it.
enum Found {
    /// A regular file, opened.
    File(fs::File),
    /// A symbolic link, with its target as bytes.
    Link(Vec<u8>),
    /// A folder or a special file.
    Other,
}

/// The entry `path` of `repository`, reached through folders that are not links. On Unix
/// each folder is opened in turn without following a link and the entry is read from the
/// last one, so a folder swapped for a link while Horizon reads is refused, not followed.
#[cfg(unix)]
fn find(repository: &Path, path: &str) -> Result<Found> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags};
    use rustix::io::Errno;
    let (folders, leaf) = parts(path)?;
    let refused = |error: Errno| -> Error {
        match error {
            Errno::LOOP | Errno::NOTDIR => Refusal::LinkedFolder(path.to_owned()).into(),
            Errno::NOENT => Error::Invalid(MOVED),
            other => io::Error::from(other).into(),
        }
    };
    let mut folder = rustix::fs::open(
        repository,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    for part in folders {
        folder = rustix::fs::openat(
            &folder,
            part,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(refused)?;
    }
    let stat = rustix::fs::statat(&folder, leaf, AtFlags::SYMLINK_NOFOLLOW).map_err(refused)?;
    match FileType::from_raw_mode(stat.st_mode) {
        FileType::Symlink => {
            let target = rustix::fs::readlinkat(&folder, leaf, Vec::new()).map_err(refused)?;
            Ok(Found::Link(target.into_bytes()))
        }
        FileType::RegularFile => {
            let file = rustix::fs::openat(
                &folder,
                leaf,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| match error {
                // Replaced by a link since it was looked at.
                Errno::LOOP => Error::Invalid(MOVED),
                other => refused(other),
            })?;
            Ok(Found::File(fs::File::from(file)))
        }
        _ => Ok(Found::Other),
    }
}

/// Whether `part` of a Git path is one plain name on this system: not empty, `.` or `..`,
/// and without a `\`, which on another system would make it several parts.
fn plain(part: &str) -> bool {
    let mut components = Path::new(part).components();
    matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(name)), None) if name == part
    )
}

/// The folders of the relative `path` and its last part, which Git names with `/`.
fn parts(path: &str) -> Result<(Vec<&str>, &str)> {
    let mut parts: Vec<_> = path.split('/').collect();
    if !parts.iter().all(|part| plain(part)) {
        return Err(Error::Invalid(MOVED));
    }
    let leaf = parts.pop().ok_or(Error::Invalid(MOVED))?;
    Ok((parts, leaf))
}

/// `snapshot` joined with `path`; with `create`, its missing folders are made. A link or a
/// file on the way, which only a deleted entry could leave, is refused.
fn inside(snapshot: &Path, path: &str, create: bool) -> Result<PathBuf> {
    let mut full = snapshot.to_path_buf();
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        if !plain(part) {
            return Err(Error::Invalid(MOVED));
        }
        full.push(part);
        if parts.peek().is_none() {
            break;
        }
        match fs::symlink_metadata(&full) {
            Ok(metadata) if metadata.is_dir() => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound && create => fs::create_dir(&full)?,
            // Nothing below a missing folder exists to delete.
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            _ => return Err(Error::Invalid(SNAPSHOT_LINK)),
        }
    }
    Ok(full)
}

/// Removes what `target` names in the snapshot, a folder with its contents; nothing to do
/// when it is missing.
fn remove(target: &Path) -> Result<()> {
    let removed = match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(target),
        Ok(_) => fs::remove_file(target),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    };
    Ok(removed?)
}

/// The opened regular `file` at `path`, refused when it is a hard link, whose content
/// another file outside the checkout can share.
fn single(file: fs::File, path: &str) -> Result<fs::File> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(Error::Invalid(MOVED));
    }
    if links(&metadata) > 1 {
        return Err(Refusal::HardLink(path.to_owned()).into());
    }
    Ok(file)
}

#[cfg(unix)]
fn links(metadata: &fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::nlink(metadata)
}

#[cfg(unix)]
fn executable(metadata: &fs::Metadata) -> bool {
    std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o111 != 0
}

/// Makes a link at `target` to `link`, a link's target as [`find`] read it.
#[cfg(unix)]
fn make_link(link: Vec<u8>, target: &Path) -> Result<()> {
    let link = PathBuf::from(<std::ffi::OsString as std::os::unix::ffi::OsStringExt>::from_vec(link));
    Ok(std::os::unix::fs::symlink(link, target)?)
}

/// The SHA-256 of all of `reader`, written to `writer` too when one is given.
fn copy(reader: &mut fs::File, mut writer: Option<&mut fs::File>) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; BUFFER];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        hasher.update(&buffer[..read]);
        if let Some(writer) = writer.as_deref_mut() {
            writer.write_all(&buffer[..read])?;
        }
    }
    Ok(hex(&hasher.finalize()))
}

/// Gives a copied file Git's mode and a fixed time, so equal files make an equal context.
fn finish(file: &fs::File, executable: bool) -> Result<()> {
    #[cfg(unix)]
    file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(if executable {
        0o755
    } else {
        0o644
    }))?;
    file.set_modified(SystemTime::UNIX_EPOCH)?;
    Ok(())
}
