//! Atomic files scoped to the worker's runtime: private to root, or readable by
//! the agent group and still writable only by root.
use std::{
    fs,
    io::{self, Write},
    path::Path,
};

pub(super) fn directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// A directory that this process owns and that `group` can only list and read.
/// A symlink or a directory that another account owns is refused, so nothing can
/// redirect the published copies.
pub(super) fn shared_directory(path: &Path, group: Option<u32>) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(path)?,
        Err(error) => return Err(error),
        Ok(metadata) if !metadata.is_dir() => {
            return Err(io::Error::other("Companion access path is not a directory"));
        }
        Ok(_) => {}
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if fs::symlink_metadata(path)?.uid() != rustix::process::geteuid().as_raw() {
            return Err(io::Error::other("Companion access directory has another owner"));
        }
        std::os::unix::fs::lchown(path, None, group)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o750))?;
    }
    #[cfg(not(unix))]
    let _ = group;
    Ok(())
}

pub(super) fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_with(path, bytes, 0o600, None)
}

/// Replaces `path` atomically with a file that has `mode` and, if given, `group`.
pub(super) fn write_with(path: &Path, bytes: &[u8], mode: u32, group: Option<u32>) -> io::Result<()> {
    let pending = path.with_extension("pending");
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&pending)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Restrict a reused pending file before writing; share it only when complete.
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(bytes)?;
        std::os::unix::fs::fchown(&file, None, group)?;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (mode, group);
        file.write_all(bytes)?;
    }
    file.sync_all()?;
    fs::rename(pending, path)?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub(super) fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

pub(super) fn remove_directory(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
