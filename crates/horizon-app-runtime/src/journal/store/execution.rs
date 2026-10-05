use super::{Store, open_file};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    version: u32,
    owner: Uuid,
    root: PathBuf,
    nonce: Uuid,
    lock_identity: Identity,
    root_identity: Identity,
}

#[derive(Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(in crate::journal) struct Identity {
    device: u64,
    inode: u64,
}
impl Identity {
    pub(in crate::journal) fn capture(file: &File) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = file.metadata().map_err(|_| Error::JournalUnavailable)?;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = file;
            Err(Error::JournalUnavailable)
        }
    }
}

fn bounded(mut file: &File) -> Result<Vec<u8>> {
    file.rewind().map_err(|_| Error::JournalUnavailable)?;
    let mut bytes = Vec::new();
    file.take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::JournalUnavailable)?;
    if bytes.len() > 8192 {
        return Err(Error::JournalInvalid);
    }
    Ok(bytes)
}

impl Store {
    pub(in crate::journal) fn claim(&self, owner: Uuid, root: &Path, root_file: &File) -> Result<File> {
        let registry_lock = open_file(&self.registry, "journal.lock", true)?;
        registry_lock.lock().map_err(|_| Error::JournalUnavailable)?;
        let marker_name = format!("{}-owner-{owner}", self.namespace);
        let lock_name = format!("execution-{owner}.lock");
        let expected = match open_file(&self.registry, &marker_name, false) {
            Ok(marker) => Some(bounded(&marker)?),
            Err(Error::JournalMissing) => None,
            Err(error) => return Err(error),
        };
        let lock = if let Some(expected) = expected {
            let lock = open_file(&self.directory, &lock_name, false).map_err(|_| Error::JournalInvalid)?;
            if bounded(&lock)? != expected {
                return Err(Error::JournalInvalid);
            }
            let binding: Binding = serde_json::from_slice(&expected).map_err(|_| Error::JournalInvalid)?;
            if binding.lock_identity != Identity::capture(&lock)? {
                return Err(Error::JournalInvalid);
            }
            if binding.version != 1
                || binding.nonce.is_nil()
                || binding.owner != owner
                || binding.root != root
                || binding.root_identity != Identity::capture(root_file)?
            {
                return Err(Error::OwnershipRefused);
            }
            lock.try_lock().map_err(|_| Error::ExecutionBusy)?;
            lock
        } else {
            // Retained registry evidence precedes the lock: missing/replaced locks cannot initialize a new actor.
            let mut marker = open_file(&self.registry, &marker_name, true)?;
            marker
                .write_all(b"initializing")
                .and_then(|()| marker.sync_all())
                .map_err(|_| Error::JournalUnavailable)?;
            self.registry.sync_all().map_err(|_| Error::JournalUnavailable)?;
            let mut lock = open_file(&self.directory, &lock_name, true)?;
            lock.try_lock().map_err(|_| Error::ExecutionBusy)?;
            let bytes = serde_json::to_vec(&Binding {
                version: 1,
                owner,
                root: root.to_owned(),
                nonce: Uuid::new_v4(),
                lock_identity: Identity::capture(&lock)?,
                root_identity: Identity::capture(root_file)?,
            })
            .map_err(|_| Error::JournalInvalid)?;
            if bytes.len() > 8192 {
                return Err(Error::JournalInvalid);
            }
            lock.write_all(&bytes)
                .and_then(|()| lock.sync_all())
                .map_err(|_| Error::JournalUnavailable)?;
            self.directory.sync_all().map_err(|_| Error::JournalUnavailable)?;
            marker
                .rewind()
                .and_then(|()| marker.set_len(0))
                .and_then(|()| marker.write_all(&bytes))
                .and_then(|()| marker.sync_all())
                .map_err(|_| Error::JournalUnavailable)?;
            self.registry.sync_all().map_err(|_| Error::JournalUnavailable)?;
            lock
        };
        Ok(lock)
    }
}
