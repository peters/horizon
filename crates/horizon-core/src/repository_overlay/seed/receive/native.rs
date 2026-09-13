use super::{PackedSourceLimits, SeedError, source_error, view};
use crate::repository_overlay::seed::packed::process::Session;
use git2::Oid;
use rustix::fs::{CWD, RenameFlags, renameat_with};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    process::Command,
};

pub(super) fn index(
    command: Command,
    path: &Path,
    objects: u32,
    limits: PackedSourceLimits,
    cancelled: &impl Fn() -> bool,
) -> Result<String, SeedError> {
    let mut session = Session::spawn(command, limits.object_timeout, Box::new(cancelled))?;
    session.begin_without_input();
    let hash = line(&mut session)?.ok_or(SeedError::Object)?;
    if line(&mut session)?.is_some() {
        return Err(SeedError::Object);
    }
    session.finish().map_err(|error| source_error(error.kind()))?;
    let hash = std::str::from_utf8(&hash).map_err(|_| SeedError::Object)?;
    let index = path.join("objects/pack/received.idx");
    let metadata = fs::symlink_metadata(&index).map_err(|_| SeedError::Storage)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.len() > u64::from(objects) * 40 + 2048
    {
        return Err(SeedError::Object);
    }
    fs::set_permissions(&index, fs::Permissions::from_mode(0o600)).map_err(|_| SeedError::Storage)?;
    Ok(hash.to_owned())
}

pub(super) fn relocate(path: &Path, hash: &str) -> Result<(), SeedError> {
    for extension in ["pack", "idx"] {
        renameat_with(
            CWD,
            path.join(format!("objects/pack/received.{extension}")),
            CWD,
            path.join(format!("objects/pack/pack-{hash}.{extension}")),
            RenameFlags::NOREPLACE,
        )
        .map_err(|_| SeedError::Storage)?;
    }
    Ok(())
}

pub(super) fn closure(
    metadata: &Path,
    objects_directory: &Path,
    commit: Oid,
    objects: u32,
    limits: PackedSourceLimits,
    cancelled: &impl Fn() -> bool,
) -> Result<(), SeedError> {
    let command = view::command(metadata, objects_directory, limits, view::Operation::Closure(commit))?;
    enumerate(command, commit, objects, limits, cancelled)
}

pub(super) fn enumerate(
    command: Command,
    commit: Oid,
    objects: u32,
    limits: PackedSourceLimits,
    cancelled: &impl Fn() -> bool,
) -> Result<(), SeedError> {
    let mut session = Session::spawn(command, limits.object_timeout, Box::new(cancelled))?;
    session
        .begin_commit(commit)
        .map_err(|error| source_error(error.kind()))?;
    let first = line(&mut session)?.ok_or(SeedError::Object)?;
    if first != commit.to_string().as_bytes() {
        return Err(SeedError::Object);
    }
    // The isolated strict pack is the only object source. Native traversal deduplicates
    // its complete shallow closure, so equality excludes all unselected packed objects.
    let mut count = 1;
    while line(&mut session)?.is_some() {
        count += 1;
        if count > objects {
            return Err(SeedError::Object);
        }
    }
    session.finish().map_err(|error| source_error(error.kind()))?;
    if count != objects {
        return Err(SeedError::Object);
    }
    Ok(())
}

pub(super) fn verify(
    command: Command,
    limits: PackedSourceLimits,
    cancelled: &impl Fn() -> bool,
) -> Result<(), SeedError> {
    let mut session = Session::spawn(command, limits.object_timeout, Box::new(cancelled))?;
    session.begin_without_input();
    if session.read(&mut [0]).map_err(|error| source_error(error.kind()))? != 0 {
        return Err(SeedError::Object);
    }
    session.finish().map_err(|error| source_error(error.kind()))
}

fn line(session: &mut Session<'_>) -> Result<Option<[u8; 40]>, SeedError> {
    let mut bytes = [0; 41];
    let mut position = 0;
    while position < bytes.len() {
        let count = session
            .read(&mut bytes[position..])
            .map_err(|error| source_error(error.kind()))?;
        if count == 0 {
            return if position == 0 {
                Ok(None)
            } else {
                Err(SeedError::Object)
            };
        }
        position += count;
    }
    if bytes[40] != b'\n'
        || !bytes[..40]
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(SeedError::Object);
    }
    Ok(Some(bytes[..40].try_into().map_err(|_| SeedError::Object)?))
}
