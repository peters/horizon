use super::{SetupClaimError as Error, SetupIntent, SetupObservation, codec};
use crate::repository_overlay::{
    reader::{
        RepositoryReadError, SelectedRepositoryReader,
        linux::{RegularFileRead, Root},
    },
    storage,
};
use rustix::fs::{AtFlags, CWD, Mode, OFlags, ResolveFlags, linkat, openat2};
use std::{
    fs::File,
    io::{self, Write},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::{Path, PathBuf},
};

const CLAIM: &str = "setup-claim.json";
const CONFINED: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

pub(super) enum Admission {
    Fresh,
    Existing,
}

pub(super) struct Directory {
    reader: Root,
    handle: File,
    path: PathBuf,
}

impl Directory {
    pub(super) fn open(path: &Path) -> Result<Self, Error> {
        let reader = SelectedRepositoryReader::open(path).map_err(root_error)?.root;
        let directory = File::from(
            openat2(
                reader.handle(),
                ".",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                CONFINED,
            )
            .map_err(storage_error)?,
        );
        let result = Self {
            reader,
            handle: directory,
            path: path.to_owned(),
        };
        result.verify()?;
        result.qualify()?;
        Ok(result)
    }

    fn qualify(&self) -> Result<(), Error> {
        storage::qualify(&self.handle).map_err(|error| match error {
            storage::StorageQualificationError::Unsupported => Error::Unsupported,
            storage::StorageQualificationError::Storage => Error::Storage,
        })
    }

    fn verify(&self) -> Result<(), Error> {
        let metadata = self.handle.metadata().map_err(|_| Error::UnsafeRoot)?;
        if !metadata.is_dir()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o7777 != 0o700
            || metadata.nlink() == 0
        {
            return Err(Error::UnsafeRoot);
        }
        let current = SelectedRepositoryReader::open(&self.path).map_err(root_error)?;
        let bound = current.root.handle().metadata().map_err(|_| Error::UnsafeRoot)?;
        if (metadata.dev(), metadata.ino()) != (bound.dev(), bound.ino()) {
            return Err(Error::UnsafeRoot);
        }
        Ok(())
    }

    fn read(&self) -> Result<Option<RegularFileRead>, Error> {
        self.verify()?;
        match self.reader.read_private_file(CLAIM, codec::MAX_RECORD_BYTES) {
            Ok(record) => {
                codec::decode(&record.bytes)?;
                Ok(Some(record))
            }
            Err(RepositoryReadError::Missing) => Ok(None),
            Err(error) => Err(read_error(error)),
        }
    }

    pub(super) fn observe(&self, intent: &SetupIntent) -> Result<SetupObservation, Error> {
        match self.read()? {
            None => Ok(SetupObservation::Absent),
            Some(record) if record.bytes == codec::encode(intent)? => Ok(SetupObservation::ClaimedUnknown),
            Some(_) => Err(Error::Conflict),
        }
    }

    pub(super) fn admit(&self, intent: &SetupIntent) -> Result<Admission, Error> {
        self.qualify()?;
        self.admit_with(
            intent,
            &mut |file, bytes| file.write_all(bytes),
            &mut File::sync_all,
            &mut link,
        )
    }

    fn admit_with(
        &self,
        intent: &SetupIntent,
        write: &mut impl FnMut(&mut File, &[u8]) -> io::Result<()>,
        sync: &mut impl FnMut(&File) -> io::Result<()>,
        publish: &mut impl FnMut(&File, &File) -> Result<(), rustix::io::Errno>,
    ) -> Result<Admission, Error> {
        if self.observe(intent)? == SetupObservation::ClaimedUnknown {
            return Ok(Admission::Existing);
        }
        let bytes = codec::encode(intent)?;
        let mut file = self.anonymous()?;
        write(&mut file, &bytes).map_err(|_| Error::Storage)?;
        sync(&file).map_err(|_| Error::Storage)?;
        self.verify()?;
        match publish(&file, &self.handle) {
            Ok(()) => {
                sync(&file).map_err(|_| Error::Storage)?;
                sync(&self.handle).map_err(|_| Error::Storage)?;
                let record = self.read()?.ok_or(Error::Read)?;
                let actual = record.file.metadata().map_err(|_| Error::Read)?;
                let expected = file.metadata().map_err(|_| Error::Read)?;
                if record.bytes != bytes || (actual.dev(), actual.ino()) != (expected.dev(), expected.ino()) {
                    return Err(Error::Read);
                }
                Ok(Admission::Fresh)
            }
            Err(rustix::io::Errno::EXIST) => match self.observe(intent)? {
                SetupObservation::ClaimedUnknown => Ok(Admission::Existing),
                SetupObservation::Absent => Err(Error::Read),
            },
            Err(error) => Err(storage_error(error)),
        }
    }

    fn anonymous(&self) -> Result<File, Error> {
        self.verify()?;
        let file = File::from(
            openat2(
                &self.handle,
                ".",
                OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
                CONFINED,
            )
            .map_err(storage_error)?,
        );
        let metadata = file.metadata().map_err(|_| Error::Storage)?;
        if !metadata.is_file()
            || metadata.nlink() != 0
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o7777 != 0o600
        {
            return Err(Error::Storage);
        }
        Ok(file)
    }
}

fn link(file: &File, directory: &File) -> Result<(), rustix::io::Errno> {
    // Only this held anonymous regular inode is followed; the fixed destination
    // is never replaced. A post-link failure must retain that replay barrier.
    linkat(
        CWD,
        format!("/proc/self/fd/{}", file.as_raw_fd()),
        directory,
        CLAIM,
        AtFlags::SYMLINK_FOLLOW,
    )
}

fn root_error(error: RepositoryReadError) -> Error {
    match error {
        RepositoryReadError::Unsupported => Error::Unsupported,
        _ => Error::UnsafeRoot,
    }
}

fn read_error(error: RepositoryReadError) -> Error {
    match error {
        RepositoryReadError::Unsupported => Error::Unsupported,
        RepositoryReadError::TooLarge => Error::InvalidRecord,
        _ => Error::Read,
    }
}

fn storage_error(error: rustix::io::Errno) -> Error {
    match error {
        rustix::io::Errno::NOSYS | rustix::io::Errno::INVAL | rustix::io::Errno::OPNOTSUPP => Error::Unsupported,
        _ => Error::Storage,
    }
}

#[cfg(test)]
mod tests;
