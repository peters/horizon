use super::{CONFINED, Directory, root_error};
use crate::repository_overlay::{
    reader::SelectedRepositoryReader,
    retained_setup::{SCRATCH_NAME, SetupBoundaryError as Error},
};
use rustix::fs::{Mode, OFlags, mkdirat, openat2};
use std::{
    fs::File,
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// Both descriptors remain held during the path-based materializer's execution.
pub(in crate::repository_overlay::retained_setup) struct Scratch {
    handle: File,
    path: PathBuf,
}

impl Scratch {
    pub(in crate::repository_overlay::retained_setup) fn path(&self) -> &Path {
        &self.path
    }

    fn verify(&self, parent: &Directory) -> Result<(), Error> {
        parent.verify()?;
        let held = self.handle.metadata().map_err(|_| Error::Storage)?;
        let current = SelectedRepositoryReader::open(&self.path).map_err(root_error)?;
        let actual = current.root.handle().metadata().map_err(|_| Error::Storage)?;
        if !held.is_dir()
            || held.uid() != rustix::process::geteuid().as_raw()
            || held.mode() & 0o7777 != 0o700
            || held.nlink() == 0
            || (held.dev(), held.ino()) != (actual.dev(), actual.ino())
        {
            return Err(Error::UnsafeRoot);
        }
        Ok(())
    }
}

pub(super) fn create(parent: &Directory) -> Result<Scratch, Error> {
    create_with(parent, &mut reserve, &mut open, &mut File::sync_all)
}

fn create_with(
    parent: &Directory,
    reserve: &mut impl FnMut(&File) -> Result<(), rustix::io::Errno>,
    open: &mut impl FnMut(&File) -> Result<File, rustix::io::Errno>,
    sync: &mut impl FnMut(&File) -> io::Result<()>,
) -> Result<Scratch, Error> {
    parent.verify()?;
    reserve(&parent.handle).map_err(storage_error)?;
    let scratch = Scratch {
        handle: open(&parent.handle).map_err(storage_error)?,
        path: parent.path.join(SCRATCH_NAME),
    };
    scratch.verify(parent)?;
    sync(&scratch.handle).map_err(|_| Error::Storage)?;
    sync(&parent.handle).map_err(|_| Error::Storage)?;
    scratch.verify(parent)?;
    Ok(scratch)
}

fn reserve(parent: &File) -> Result<(), rustix::io::Errno> {
    mkdirat(parent, SCRATCH_NAME, Mode::RUSR | Mode::WUSR | Mode::XUSR)
}

fn open(parent: &File) -> Result<File, rustix::io::Errno> {
    openat2(
        parent,
        SCRATCH_NAME,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        CONFINED,
    )
    .map(File::from)
}

fn storage_error(error: rustix::io::Errno) -> Error {
    if error == rustix::io::Errno::EXIST {
        Error::ExistingScratch
    } else {
        super::storage_error(error).into()
    }
}

#[cfg(test)]
mod tests;
