use super::{Error, Runtime, daemon, live};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};
use tokio::{
    net::{UnixListener, UnixStream},
    time::Instant,
};

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
        };
        fs::set_permissions(&result.path, fs::Permissions::from_mode(0o600))?;
        result.verify()?;
        Ok(result)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn accept(&self) -> Result<UnixStream, Error> {
        self.verify()?;
        let stream = self.listener.as_ref().ok_or(Error::Custody)?.accept().await?.0;
        self.verify()?;
        Ok(stream)
    }

    pub fn verify_stream(&self, stream: &UnixStream, until: Instant) -> Result<(), Error> {
        live(until)?;
        self.verify()?;
        if stream.local_addr()?.as_pathname() != Some(self.path.as_path()) {
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
    use tokio::io::AsyncWriteExt;

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
                .verify_stream(&unbound, Instant::now() + super::super::IDENTIFY_TIME)
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
