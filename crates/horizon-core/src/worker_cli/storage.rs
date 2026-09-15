//! Private persistent identity and write-before-dispatch operation claims.

use super::{Error, Intent};
use horizon_core::{
    HorizonHome, SessionStore,
    cloud_run::{CloudWorkflowStore, StoredRemoteWorkspace},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    pub version: u8,
    // Atomically published with identity: dispatch may have occurred, never permission to replay.
    #[serde(default)]
    pub create_dispatch_claimed: bool,
    pub root: PathBuf,
    pub session: String,
    pub workspace: String,
    pub panel: String,
    pub intent: Intent,
}

pub(super) struct Context {
    pub home: HorizonHome,
    pub receipt: Receipt,
    _lock: File,
}

impl Context {
    pub fn open(root: &Path) -> Result<Self, Error> {
        let root = fs::canonicalize(root).map_err(|_| Error::Storage)?;
        private_directory(&root)?;
        let lock = lock(&root)?;
        let receipt: Receipt =
            serde_json::from_slice(&fs::read(root.join("receipt.json")).map_err(|_| Error::Storage)?)
                .map_err(|_| Error::Storage)?;
        if receipt.version != 1 || receipt.root != root {
            return Err(Error::Storage);
        }
        receipt.intent.config.validate().map_err(|_| Error::Input)?;
        let home = HorizonHome::from_root(root.join("home"));
        let sessions = SessionStore::new(home.clone(), root.join("profile.json"));
        let resolved = sessions.resume_session(&receipt.session).map_err(|_| Error::Storage)?;
        if resolved.session_id != receipt.session {
            return Err(Error::Storage);
        }
        Ok(Self {
            home,
            receipt,
            _lock: lock,
        })
    }

    pub fn new(root: &Path, receipt: Receipt, lock: File) -> Self {
        Self {
            home: HorizonHome::from_root(root.join("home")),
            receipt,
            _lock: lock,
        }
    }

    pub fn store(&self) -> Result<CloudWorkflowStore, Error> {
        CloudWorkflowStore::open_existing_without_migration(&self.home).map_err(|_| Error::Storage)
    }

    pub fn saved(&self) -> Result<StoredRemoteWorkspace, Error> {
        self.store()?
            .load_remote_workspace(&self.receipt.session, &self.receipt.workspace)
            .map_err(|_| Error::Storage)?
            .ok_or(Error::Operation)
    }

    pub fn claim(&self, operation: &str) -> Result<(), Error> {
        if operation == "create" && self.receipt.create_dispatch_claimed {
            return Err(Error::Claimed);
        }
        // Keep a failed/uncertain claim: an interrupted reply cannot authorize replay.
        write_new(
            &self.receipt.root.join(format!("{operation}.claimed")),
            b"dispatch may have occurred\n",
        )
        .map_err(|_| Error::Claimed)
    }
}

pub(super) fn private_directory(root: &Path) -> Result<(), Error> {
    let meta = fs::symlink_metadata(root).map_err(|_| Error::Storage)?;
    if !meta.is_dir() || meta.mode() & 0o077 != 0 || meta.uid() != rustix::process::geteuid().as_raw() {
        return Err(Error::Storage);
    }
    Ok(())
}

pub(super) fn create_root(root: &Path) -> Result<PathBuf, Error> {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(root)
        .map_err(|_| Error::Storage)?;
    let root = fs::canonicalize(root).map_err(|_| Error::Storage)?;
    File::open(root.parent().ok_or(Error::Storage)?)
        .and_then(|parent| parent.sync_all())
        .map_err(|_| Error::Storage)?;
    Ok(root)
}

pub(super) fn lock(root: &Path) -> Result<File, Error> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(root.join("controller.lock"))
        .map_err(|_| Error::Storage)?;
    file.try_lock().map_err(|_| Error::Storage)?;
    Ok(file)
}

pub(super) fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| Error::Storage)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| Error::Storage)?;
    File::open(path.parent().ok_or(Error::Storage)?)
        .and_then(|parent| parent.sync_all())
        .map_err(|_| Error::Storage)
}

/// Publish a complete recoverable intent without exposing a partial final file.
pub(super) fn publish_journal(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let parent = path.parent().ok_or(Error::Storage)?;
    let mut pending = tempfile::NamedTempFile::new_in(parent).map_err(|_| Error::Storage)?;
    pending
        .write_all(bytes)
        .and_then(|()| pending.as_file().sync_all())
        .map_err(|_| Error::Storage)?;
    pending.persist_noclobber(path).map_err(|_| Error::Storage)?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| Error::Storage)
}
