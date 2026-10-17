use super::{Error, PendingStream, Runtime, daemon, live, pending::Origin};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{net::UnixListener, time::Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    device: u64,
    inode: u64,
}
impl Key {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

/// A private original listener, never a TCP endpoint reachable through generic proxies.
pub(super) struct Backend {
    listener: Option<UnixListener>,
    path: PathBuf,
    original: Key,
    origin: Arc<Origin>,
}
impl Backend {
    pub fn bind(runtime: Runtime, port: u16) -> Result<Self, Error> {
        let directory = Path::new(match runtime {
            Runtime::Worker => "/run/horizon-tailnet/producer",
            Runtime::Kernel => "/run/horizon-kernel-producer",
        });
        Self::bind_in(directory, port)
    }

    fn bind_in(directory: &Path, port: u16) -> Result<Self, Error> {
        daemon::ancestors(directory)?;
        match fs::DirBuilder::new().mode(0o700).create(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        private_directory(directory)?;
        let path = directory.join(format!("{port}.sock"));
        // Never unlink an existing path to make a new candidate start.
        let listener = UnixListener::bind(&path)?;
        let original = Key::of(&fs::symlink_metadata(&path)?);
        let result = Self {
            listener: Some(listener),
            path,
            original,
            origin: Arc::new(Origin),
        };
        fs::set_permissions(&result.path, fs::Permissions::from_mode(0o600))?;
        result.verify()?;
        Ok(result)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn accept(&self) -> Result<PendingStream, Error> {
        self.verify()?;
        let stream = self.listener.as_ref().ok_or(Error::Custody)?.accept().await?.0;
        self.verify()?;
        Ok(PendingStream::new(stream, Arc::clone(&self.origin)))
    }

    pub fn verify_stream(&self, pending: &PendingStream, until: Instant) -> Result<(), Error> {
        live(until)?;
        if !pending.has_origin(&self.origin) {
            return Err(Error::Custody);
        }
        self.verify()?;
        if pending.payload()?.local_addr()?.as_pathname() != Some(self.path.as_path()) {
            return Err(Error::Custody);
        }
        live(until)
    }

    fn verify(&self) -> Result<(), Error> {
        daemon::ancestors(&self.path)?;
        private_directory(self.path.parent().ok_or(Error::Custody)?)?;
        let metadata = fs::symlink_metadata(&self.path)?;
        if !metadata.file_type().is_socket()
            || metadata.uid() != 0
            || metadata.mode() & 0o777 != 0o600
            || Key::of(&metadata) != self.original
        {
            return Err(Error::Custody);
        }
        Ok(())
    }

    pub fn retire(mut self) -> Result<(), Error> {
        self.verify()?;
        drop(self.listener.take());
        fs::remove_file(&self.path)?;
        Ok(())
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        drop(self.listener.take());
        // A replacement path belongs to another owner and is never removed.
        if daemon::ancestors(&self.path).is_ok()
            && fs::symlink_metadata(&self.path).is_ok_and(|metadata| Key::of(&metadata) == self.original)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn private_directory(path: &Path) -> Result<(), Error> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o777 != 0o700 {
        return Err(Error::Custody);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::AsyncWriteExt, net::UnixStream};

    #[tokio::test]
    #[ignore = "Requires the separate isolated root backend fixture; no tailnet enrollment"]
    async fn retired_backend_acceptance_cannot_transfer_to_a_successor_at_the_same_path() {
        assert!(rustix::process::geteuid().is_root());
        let directory = tempfile::tempdir_in("/run").unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let original = Backend::bind_in(directory.path(), 20032).unwrap();
        let path = original.path().to_owned();
        let _peer = UnixStream::connect(&path).await.unwrap();
        let pending = original.accept().await.unwrap();
        original
            .verify_stream(&pending, Instant::now() + super::super::IDENTIFY_TIME)
            .unwrap();
        original.retire().unwrap();
        let successor = Backend::bind_in(directory.path(), 20032).unwrap();
        // The old stream still has exactly the new listener's pathname, while
        // the successor has valid current inode/mode/ancestor checks.
        assert_eq!(
            pending.payload().unwrap().local_addr().unwrap().as_pathname(),
            Some(path.as_path())
        );
        successor.verify().unwrap();
        assert!(matches!(
            successor.verify_stream(&pending, Instant::now() + super::super::IDENTIFY_TIME),
            Err(Error::Custody)
        ));
        let _new_peer = UnixStream::connect(&path).await.unwrap();
        let current = successor.accept().await.unwrap();
        successor
            .verify_stream(&current, Instant::now() + super::super::IDENTIFY_TIME)
            .unwrap();
        drop(current);
        drop(pending);
        successor.retire().unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    #[ignore = "Requires the separate isolated root backend fixture; no tailnet enrollment"]
    async fn original_private_listener_refuses_replacement_and_cleans_only_its_own_path() {
        assert!(rustix::process::geteuid().is_root());
        let directory = tempfile::tempdir_in("/run").unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let backend = Backend::bind_in(directory.path(), 20032).unwrap();
        let original_path = backend.path().to_owned();
        assert!(Backend::bind_in(directory.path(), 20032).is_err());
        let mut client = UnixStream::connect(&original_path).await.unwrap();
        let accepted = backend.accept().await.unwrap();
        backend
            .verify_stream(&accepted, Instant::now() + super::super::IDENTIFY_TIME)
            .unwrap();
        let (unbound, _) = UnixStream::pair().unwrap();
        assert!(
            backend
                .verify_stream(
                    &super::super::pending_fixture(unbound),
                    Instant::now() + super::super::IDENTIFY_TIME
                )
                .is_err()
        );
        client.write_all(b"PROXY forged").await.unwrap();
        fs::remove_file(&original_path).unwrap();
        let replacement = UnixListener::bind(&original_path).unwrap();
        fs::set_permissions(&original_path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            backend
                .verify_stream(&accepted, Instant::now() + super::super::IDENTIFY_TIME)
                .is_err()
        );
        assert!(backend.retire().is_err());
        assert!(original_path.exists());
        drop(replacement);
        fs::remove_file(&original_path).unwrap();

        let backend = Backend::bind_in(directory.path(), 20033).unwrap();
        let path = backend.path().to_owned();
        backend.retire().unwrap();
        assert!(!path.exists());
        let backend = Backend::bind_in(directory.path(), 20034).unwrap();
        let path = backend.path().to_owned();
        drop(backend);
        assert!(!path.exists());
    }
}
