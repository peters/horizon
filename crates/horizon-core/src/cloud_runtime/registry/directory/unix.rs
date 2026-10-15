use super::super::{Error, Result};
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

pub(super) fn ensure(root: &Path) -> Result<()> {
    ensure_with(root, true, &mut |_| Ok(()))
}

fn marker(parent: &Path) -> PathBuf {
    parent.join(format!(
        ".horizon-registry-directories-{}.lock",
        rustix::process::geteuid().as_raw()
    ))
}

fn metadata(path: &Path) -> Result<Option<std::fs::Metadata>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn ensure_with(root: &Path, private: bool, created: &mut impl FnMut(&Path) -> Result<()>) -> Result<()> {
    if metadata(root)?.is_some() {
        // Resolve ancestor aliases before finding their creation transaction.
        if private {
            validate_private(root)?;
        } else if !root.is_dir() {
            return Err(Error::Invalid("Registry directory ancestor is not a directory"));
        }
        let actual = root.canonicalize()?;
        if let Some(parent) = actual.parent()
            && metadata(&marker(parent))?.is_some()
        {
            let intent = lock(parent, false)?;
            recover(parent, &intent)?;
        }
        return Ok(());
    }
    let parent = root
        .parent()
        .ok_or(Error::Invalid("Registry directory has no parent"))?;
    // An existing parent may be another creator's unfinished directory too.
    ensure_with(parent, false, created)?;
    let parent = parent.canonicalize()?;
    let root = parent.join(root.file_name().ok_or(Error::Invalid("Invalid registry directory"))?);
    let intent = lock(&parent, true)?;
    recover(&parent, &intent)?;
    if metadata(&root)?.is_none() {
        // The stable parent lock has a durable pending bit before mkdir. It is
        // never unlinked: contenders must coordinate through the same inode.
        intent.set_len(1)?;
        intent.sync_all()?;
        File::open(&parent)?.sync_all()?;
        std::fs::DirBuilder::new().mode(0o700).create(&root)?;
        created(&root)?;
    }
    if private {
        validate_private(&root)?;
    } else if !root.is_dir() {
        return Err(Error::Invalid("Registry directory ancestor is not a directory"));
    }
    File::open(&root)?.sync_all()?;
    File::open(&parent)?.sync_all()?;
    clear(&intent)?;
    Ok(())
}

fn lock(parent: &Path, create: bool) -> Result<File> {
    let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK;
    let flags = i32::try_from(flags.bits()).map_err(|_| Error::Invalid("Unsupported registry directory flags"))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .truncate(false)
        .mode(0o600)
        .custom_flags(flags)
        .open(marker(parent))?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        return Err(Error::Invalid(
            "Registry directory coordination must be a private owned file",
        ));
    }
    file.lock()?;
    Ok(file)
}

fn recover(parent: &Path, intent: &File) -> Result<()> {
    match intent.metadata()?.len() {
        0 => Ok(()),
        1 => {
            File::open(parent)?.sync_all()?;
            clear(intent)
        }
        _ => Err(Error::Invalid("Invalid registry directory creation intent")),
    }
}

fn clear(intent: &File) -> Result<()> {
    intent.set_len(0)?;
    intent.sync_all()?;
    Ok(())
}

fn validate_private(root: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(root)?;
    if !metadata.is_dir() {
        return Err(Error::Invalid(
            "Registry state root must be a private directory, not a symlink",
        ));
    }
    if metadata.permissions().mode() & 0o077 != 0 || metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(Error::Invalid(
            "Registry state root must be owned by this user and private (0700)",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
