//! Transactional private files for local account setup; no network or registry access.
use super::{Error, Result, Settings};
use crate::cloud_runtime::{Cancellation, command::Runner};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub(super) struct Transaction {
    root: PathBuf,
    created: Vec<PathBuf>,
    directories: Vec<PathBuf>,
    committed: bool,
    lock: File,
}

impl Transaction {
    pub fn new(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("settings.lock"))?;
        lock.try_lock()
            .map_err(|_| Error::Invalid("Cloud settings are being saved by another process"))?;
        Ok(Self {
            root: root.into(),
            created: Vec::new(),
            directories: Vec::new(),
            committed: false,
            lock,
        })
    }

    pub fn verify_current(&self, expected: Option<&[u8]>) -> Result<()> {
        let observed = match std::fs::read(self.root.join("settings.json")) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if observed.as_deref() != expected {
            return Err(Error::Invalid("Cloud settings changed elsewhere; reload before saving"));
        }
        Ok(())
    }

    pub fn secret(&mut self, name: &str, value: &str) -> Result<PathBuf> {
        let directory = self.root.join("credentials");
        private_directory(&directory)?;
        let mut file = tempfile::Builder::new()
            .prefix(&format!("{name}-"))
            .tempfile_in(&directory)?;
        file.write_all(value.trim().as_bytes())?;
        file.flush()?;
        file.as_file().sync_all()?;
        let (_, path) = file.keep().map_err(|error| error.error)?;
        self.created.push(path.clone());
        sync_directory(&directory)?;
        Ok(path)
    }

    pub fn ssh_identity(&mut self) -> Result<PathBuf> {
        let directory = tempfile::Builder::new()
            .prefix("identity-")
            .tempdir_in(&self.root)?
            .keep();
        private_directory(&directory)?;
        self.directories.push(directory.clone());
        let path = directory.join("ed25519");
        let cancel = Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        runner.run(
            "Create dedicated SSH key",
            std::process::Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&path),
            std::time::Duration::from_secs(20),
        )?;
        OpenOptions::new().write(true).open(&path)?.sync_all()?;
        OpenOptions::new()
            .write(true)
            .open(path.with_extension("pub"))?
            .sync_all()?;
        sync_directory(&directory)?;
        sync_directory(&self.root)?;
        Ok(path)
    }

    pub fn commit(&mut self, settings: &Settings, expected: Option<&[u8]>) -> Result<()> {
        settings.validate()?;
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        serde_json::to_writer_pretty(&mut file, settings).map_err(|_| Error::Json)?;
        file.write_all(b"\n")?;
        file.flush()?;
        file.as_file().sync_all()?;
        self.verify_current(expected)?;
        file.persist(self.root.join("settings.json"))
            .map_err(|error| error.error)?;
        // Once the settings file refers to these secrets, retain them even if
        // directory durability cannot be confirmed. Never leave dangling bindings.
        self.committed = true;
        sync_directory(&self.root)
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if !self.committed {
            for path in &self.created {
                let _ = std::fs::remove_file(path);
            }
            for path in &self.directories {
                let _ = std::fs::remove_dir_all(path);
            }
        }
        // A concurrently forked child can retain the open file description.
        // Release our transaction lock explicitly, after rollback is complete.
        let _ = self.lock.unlock();
    }
}

fn private_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_transaction_releases_lock_while_a_duplicate_handle_remains() {
        for committed in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let mut transaction = Transaction::new(root.path()).unwrap();
            let secret = transaction.secret("compute", "synthetic-key").unwrap();
            transaction.committed = committed;
            let duplicate = transaction.lock.try_clone().unwrap();
            assert!(Transaction::new(root.path()).is_err());
            drop(transaction);
            assert_eq!(secret.exists(), committed);
            let next = Transaction::new(root.path()).unwrap();
            drop(next);
            drop(duplicate);
        }
    }
}
