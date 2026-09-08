use super::{PackedSourceLimits, SeedError as Error, staging};
use crate::repository_overlay::reader::SelectedRepositoryReader;
use std::{
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    process::Command,
};

pub(super) const MAX_NODES: usize = 262_144;
pub(super) const MAX_PATH_BYTES: usize = 16 * 1024 * 1024;

pub(super) fn validate(objects: &Path, parent: &Path, cancelled: &impl Fn() -> bool) -> Result<(), Error> {
    staging::check_cancel(cancelled)?;
    if !objects.is_absolute() {
        return Err(Error::UnsafeParent);
    }
    let root = SelectedRepositoryReader::open(objects).map_err(|_| Error::UnsafeParent)?;
    let scratch = SelectedRepositoryReader::open(parent).map_err(|_| Error::UnsafeParent)?;
    let scratch = scratch.root.handle().metadata().map_err(|_| Error::UnsafeParent)?;
    // Probe Git's actual lookup, including aliases on casefold filesystems.
    match rustix::fs::openat2(
        root.root.handle(),
        "info/alternates",
        rustix::fs::OFlags::PATH | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
        rustix::fs::ResolveFlags::BENEATH
            | rustix::fs::ResolveFlags::NO_SYMLINKS
            | rustix::fs::ResolveFlags::NO_MAGICLINKS
            | rustix::fs::ResolveFlags::NO_XDEV,
    ) {
        Err(rustix::io::Errno::NOENT) => {}
        _ => return Err(Error::UnsafeParent),
    }
    let device = root.root.handle().metadata().map_err(|_| Error::Source)?.dev();
    let mut pending = vec![PathBuf::new()];
    let (mut nodes, mut paths) = (0usize, 0usize);
    while let Some(relative) = pending.pop() {
        staging::check_cancel(cancelled)?;
        let path = objects.join(&relative);
        let metadata = fs::symlink_metadata(&path).map_err(|_| Error::Source)?;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.dev() != device
            || metadata.nlink() == 0
            || !(metadata.is_dir() || (metadata.is_file() && metadata.nlink() == 1))
        {
            return Err(Error::UnsafeParent);
        }
        if metadata.is_dir() {
            // Compare identities, not spelling: scratch must never modify the source.
            if metadata.dev() == scratch.dev() && metadata.ino() == scratch.ino() {
                return Err(Error::UnsafeParent);
            }
            if relative.components().count() >= 4 {
                return Err(Error::Limit);
            }
            for entry in fs::read_dir(path).map_err(|_| Error::Source)? {
                staging::check_cancel(cancelled)?;
                let child = relative.join(entry.map_err(|_| Error::Source)?.file_name());
                nodes = nodes.checked_add(1).filter(|n| *n <= MAX_NODES).ok_or(Error::Limit)?;
                paths = paths
                    .checked_add(child.as_os_str().len())
                    .filter(|n| *n <= MAX_PATH_BYTES)
                    .ok_or(Error::Limit)?;
                pending.push(child);
            }
        }
    }
    Ok(())
}

pub(super) fn command(path: &Path, objects: &Path, limits: PackedSourceLimits) -> Result<Command, Error> {
    for directory in ["objects", "objects/info", "objects/pack", "refs"] {
        fs::create_dir(path.join(directory)).map_err(|_| Error::Storage)?;
    }
    for (name, contents) in [
        ("HEAD", "ref: refs/heads/isolated\n"),
        ("config", "[core]\nrepositoryformatversion = 0\nbare = true\n"),
    ] {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path.join(name))
            .and_then(|mut file| file.write_all(contents.as_bytes()))
            .map_err(|_| Error::Storage)?;
    }
    let mut quoted = String::from("\"");
    for &byte in objects.as_os_str().as_bytes() {
        match byte {
            b'"' | b'\\' => {
                quoted.push('\\');
                quoted.push(char::from(byte));
            }
            0x20..=0x7e => quoted.push(char::from(byte)),
            _ => write!(quoted, "\\{byte:03o}").map_err(|_| Error::Source)?,
        }
    }
    quoted.push('"');
    let mut command = Command::new("/usr/bin/prlimit");
    command
        .args([
            format!("--as={0}:{0}", limits.address_space_bytes),
            format!("--cpu={0}:{0}", limits.cpu_seconds),
            "--nofile=64:64".into(),
            "--core=0:0".into(),
        ])
        .args(["--", "/usr/bin/git", "--no-replace-objects", "--git-dir"])
        .arg(path)
        .args([
            "-c",
            "core.multiPackIndex=false",
            "-c",
            "core.commitGraph=false",
            "-c",
            "core.packedGitLimit=64m",
            "-c",
            "core.packedGitWindowSize=16m",
            "-c",
            "core.deltaBaseCacheLimit=16m",
            "-c",
            "core.bigFileThreshold=1m",
            "cat-file",
            "--batch-command",
        ])
        .current_dir(path)
        .env_clear()
        .envs([
            ("LC_ALL", "C"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_SYSTEM", "/dev/null"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_ALLOW_PROTOCOL", ""),
            ("GIT_OPTIONAL_LOCKS", "0"),
            ("GIT_TERMINAL_PROMPT", "0"),
        ])
        .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", quoted);
    Ok(command)
}
