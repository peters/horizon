use super::super::{SetupBoundaryError, SetupCompletion, SetupRecordError, outcome::codec};
use super::{Directory, RegularFileRead, RepositoryReadError, SetupIntent, SetupObservation, storage_error};
use rustix::fs::{AtFlags, CWD, linkat};
use std::{
    fs::File,
    io,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
};

const RESULT: &str = "setup-result.json";

fn claimed(directory: &Directory, intent: &SetupIntent) -> Result<(), SetupRecordError> {
    if directory.observe(intent)? != SetupObservation::ClaimedUnknown {
        return Err(SetupBoundaryError::InvalidClaim.into());
    }
    Ok(())
}

pub(super) fn read(directory: &Directory, intent: &SetupIntent) -> Result<Option<SetupCompletion>, SetupRecordError> {
    claimed(directory, intent)?;
    let result = read_file(directory)?
        .map(|record| codec::decode(&directory.path, intent, &record.bytes))
        .transpose()?;
    claimed(directory, intent)?;
    Ok(result)
}

fn read_file(directory: &Directory) -> Result<Option<RegularFileRead>, SetupRecordError> {
    match directory.reader.read_private_file(RESULT, codec::MAX_BYTES) {
        Ok(record) => Ok(Some(record)),
        Err(RepositoryReadError::Missing) => Ok(None),
        Err(RepositoryReadError::Unsupported) => Err(SetupBoundaryError::Unsupported.into()),
        Err(RepositoryReadError::TooLarge) => Err(SetupRecordError::InvalidRecord),
        Err(_) => Err(SetupRecordError::Read),
    }
}

pub(super) fn record(
    directory: &Directory,
    intent: &SetupIntent,
    completion: &SetupCompletion,
    write: &mut impl FnMut(&mut File, &[u8]) -> io::Result<()>,
    sync: &mut impl FnMut(&File) -> io::Result<()>,
    publish: &mut impl FnMut(&File, &File) -> Result<(), rustix::io::Errno>,
) -> Result<(), SetupRecordError> {
    if read(directory, intent)?.is_some() {
        return Err(SetupRecordError::Existing);
    }
    let bytes = codec::encode(&directory.path, intent, completion)?;
    let mut file = directory.anonymous()?;
    write(&mut file, &bytes).map_err(|_| SetupRecordError::Storage)?;
    sync(&file).map_err(|_| SetupRecordError::Storage)?;
    claimed(directory, intent)?;
    publish(&file, &directory.handle).map_err(|error| match error {
        rustix::io::Errno::EXIST => SetupRecordError::Existing,
        error => SetupRecordError::from(storage_error(error)),
    })?;
    sync(&file).map_err(|_| SetupRecordError::Storage)?;
    sync(&directory.handle).map_err(|_| SetupRecordError::Storage)?;
    claimed(directory, intent)?;
    let actual = read_file(directory)?.ok_or(SetupRecordError::Read)?;
    let observed = actual.file.metadata().map_err(|_| SetupRecordError::Read)?;
    let expected = file.metadata().map_err(|_| SetupRecordError::Read)?;
    if actual.bytes != bytes || (observed.dev(), observed.ino()) != (expected.dev(), expected.ino()) {
        return Err(SetupRecordError::Read);
    }
    claimed(directory, intent)?;
    Ok(())
}

pub(super) fn link(file: &File, directory: &File) -> Result<(), rustix::io::Errno> {
    // Follow only the held anonymous inode, never the fixed no-replace destination.
    linkat(
        CWD,
        format!("/proc/self/fd/{}", file.as_raw_fd()),
        directory,
        RESULT,
        AtFlags::SYMLINK_FOLLOW,
    )
}

#[cfg(test)]
mod tests;
