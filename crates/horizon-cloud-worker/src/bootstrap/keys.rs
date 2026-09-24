//! Bound the exact private bytes used by sshd to their Ed25519 public key.
use super::store::{invalid, open_directory, regular, secret};
use rustix::fs::{MemfdFlags, Mode, OFlags, fchmod, memfd_create, openat};
use std::{
    fs::File,
    io::{self, Read, Seek, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};

pub(super) fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub(super) const HOST_KEY: &str = "ssh-host-key";

fn memory_file() -> io::Result<File> {
    let file = File::from(memfd_create("horizon-key-check", MemfdFlags::CLOEXEC)?);
    fchmod(&file, Mode::RUSR | Mode::WUSR)?;
    Ok(file)
}

fn execute(command: &mut Command) -> io::Result<Vec<u8>> {
    let mut output = memory_file()?;
    let mut child = command.stdout(output.try_clone()?).stderr(Stdio::null()).spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return Err(invalid()),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(invalid());
            }
        }
    }
    output.rewind()?;
    let mut bytes = Vec::new();
    output.take(8193).read_to_end(&mut bytes)?;
    if bytes.len() > 8192 {
        return Err(invalid());
    }
    Ok(bytes)
}

pub(super) fn public(bytes: &[u8]) -> io::Result<String> {
    let mut input = memory_file()?;
    input.write_all(bytes)?;
    input.rewind()?;
    let output = execute(
        Command::new("ssh-keygen")
            .args(["-y", "-P", "", "-f", "/dev/stdin"])
            .stdin(input),
    )?;
    let text = std::str::from_utf8(&output).map_err(|_| invalid())?;
    let fields: Vec<_> = text.split_whitespace().collect();
    if fields.len() < 2 || fields[0] != "ssh-ed25519" {
        return Err(invalid());
    }
    Ok(format!("{} {}", fields[0], fields[1]))
}

pub(super) fn current(directory: &Path) -> io::Result<zeroize::Zeroizing<Vec<u8>>> {
    secret(&mut regular(&open_directory(directory)?, HOST_KEY)?)
}

pub(super) fn install(directory: &Path, bytes: &[u8]) -> io::Result<()> {
    public(bytes)?;
    let root = open_directory(directory)?;
    match openat(
        &root,
        HOST_KEY,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    ) {
        Ok(fd) => {
            let mut file = File::from(fd);
            file.write_all(bytes)?;
            file.sync_all()?;
            root.sync_all()?;
        }
        Err(rustix::io::Errno::EXIST) => {}
        Err(error) => return Err(error.into()),
    }
    if current(directory)?.as_slice() != bytes {
        return Err(invalid());
    }
    regular(&root, HOST_KEY)?.sync_all()?;
    root.sync_all()
}

pub(super) fn fresh(directory: &Path) -> io::Result<()> {
    match current(directory) {
        Ok(bytes) => {
            public(&bytes)?;
            return Ok(());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    // Only a task-private run directory receives generated material. No workspace
    // path is written until the signed initialization command has been verified.
    let staging = directory.join(format!("key-{}", horizon_cloud_protocol::OperationId::generate()));
    std::fs::DirBuilder::new().mode(0o700).create(&staging)?;
    let path = staging.join(HOST_KEY);
    execute(
        Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&path)
            .stdin(Stdio::null()),
    )?;
    let bytes = current(&staging)?;
    install(directory, &bytes)?;
    std::fs::remove_file(&path)?;
    std::fs::remove_file(path.with_extension("pub"))?;
    std::fs::remove_dir(&staging)
}

use std::os::unix::fs::DirBuilderExt as _;

/// Seal the exact bytes prepared before sshd starts. Initialize checks this seal,
/// so a later private-seed change cannot hide behind an unchanged public field.
pub(super) fn seal(directory: &Path) -> io::Result<()> {
    let bytes = current(directory)?;
    let root = open_directory(directory)?;
    let digest = hash(&bytes);
    match openat(
        &root,
        "ssh-host-key.sha256",
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    ) {
        Ok(fd) => {
            let mut file = File::from(fd);
            file.write_all(&digest)?;
            file.sync_all()?;
            root.sync_all()?;
        }
        Err(rustix::io::Errno::EXIST) => {}
        Err(error) => return Err(error.into()),
    }
    captured(directory).map(|_| ())
}

pub(super) fn captured(directory: &Path) -> io::Result<zeroize::Zeroizing<Vec<u8>>> {
    let bytes = current(directory)?;
    let mut digest = Vec::new();
    regular(&open_directory(directory)?, "ssh-host-key.sha256")?
        .take(33)
        .read_to_end(&mut digest)?;
    if digest.as_slice() != hash(&bytes) {
        return Err(invalid());
    }
    Ok(bytes)
}
