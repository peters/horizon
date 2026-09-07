use super::{Error, HOST_ALIAS, InteractiveWorkerSshEndpoint, RemoteSshIdentity};
use rustix::fs::{CWD, Mode, OFlags, ResolveFlags, openat2};
use std::{
    fs::File,
    io::Write,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::{Path, PathBuf},
};

/// The argument names this exact held descriptor in the owning process, not the SSH child.
/// No directory entry is created, including on failure or process exit without destructors.
pub(crate) struct KnownHosts {
    file: File,
    path: PathBuf,
}

impl KnownHosts {
    fn create(parent: &Path, host_key: &str) -> Result<Self, Error> {
        let directory = File::from(
            openat2(
                CWD,
                parent,
                OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
            )
            .map_err(|_| Error::TrustStorage)?,
        );
        let metadata = directory.metadata().map_err(|_| Error::TrustStorage)?;
        let uid = rustix::process::geteuid().as_raw();
        if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o7777 != 0o700 || metadata.nlink() == 0 {
            return Err(Error::TrustStorage);
        }
        // A named fallback would restore the process::exit leak this guard prevents.
        let mut file = File::from(
            openat2(
                &directory,
                ".",
                OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC | OFlags::EXCL,
                Mode::RUSR | Mode::WUSR,
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
            )
            .map_err(|_| Error::TrustStorage)?,
        );
        let metadata = file.metadata().map_err(|_| Error::TrustStorage)?;
        if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o7777 != 0o600 || metadata.nlink() != 0 {
            return Err(Error::TrustStorage);
        }
        writeln!(file, "{HOST_ALIAS} {host_key}").map_err(|_| Error::TrustStorage)?;
        let path = PathBuf::from(format!("/proc/{}/fd/{}", std::process::id(), file.as_raw_fd()));
        // Restricted procfs access must reject preparation, never choose alternate trust.
        let visible = std::fs::metadata(&path).map_err(|_| Error::TrustStorage)?;
        if visible.dev() != metadata.dev() || visible.ino() != metadata.ino() {
            return Err(Error::TrustStorage);
        }
        Ok(Self { file, path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn into_file(self) -> File {
        self.file
    }
}

pub(crate) fn known_hosts(
    identity: &RemoteSshIdentity,
    endpoint: &InteractiveWorkerSshEndpoint,
) -> Result<KnownHosts, Error> {
    let parent = identity.private_key_path().parent().ok_or(Error::UnsupportedPath)?;
    KnownHosts::create(parent, &endpoint.host_key)
}

#[cfg(test)]
mod tests;
