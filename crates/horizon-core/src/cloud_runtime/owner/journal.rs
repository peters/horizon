use super::{Error, Result};
use horizon_cloud_protocol::{AllocationId, ControllerId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::Path,
};

pub(super) const MARKER: &str = "owner.json";
pub(super) const JOURNAL: &str = "journal.json";
pub(super) const CANDIDATE: &str = "candidate.json";
const LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Marker {
    pub version: u32,
    pub allocation: AllocationId,
    pub controller: ControllerId,
    pub registration: uuid::Uuid,
    pub public_key_hash: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Anchor {
    pub generation: u64,
    pub hash: [u8; 32],
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    pub owner: Marker,
    pub generation: u64,
    pub payload: serde_json::Value,
}

pub(super) fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub(super) fn read(root: &Path, name: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(root.join(name))?.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err(Error::Journal);
    }
    Ok(bytes)
}

pub(super) fn write(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    if bytes.len() as u64 > LIMIT {
        return Err(Error::Journal);
    }
    horizon_browser_control::atomic_file::replace(&root.join(name), bytes)?;
    Ok(())
}

pub(super) fn decode(bytes: &[u8], marker: &Marker, anchor: &Anchor) -> Result<Journal> {
    if hash(bytes) != anchor.hash {
        return Err(Error::Journal);
    }
    let journal: Journal = serde_json::from_slice(bytes).map_err(|_| Error::Journal)?;
    if journal.owner != *marker || journal.generation != anchor.generation {
        return Err(Error::Journal);
    }
    Ok(journal)
}

pub(super) struct Lock(File);

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LockIdentity {
    device: u64,
    inode: u64,
}

impl LockIdentity {
    fn from_metadata(metadata: &std::fs::Metadata) -> Result<Self> {
        if !metadata.is_file() {
            return Err(Error::Ownership);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        Err(Error::Ownership)
    }

    pub(super) fn at_path(path: &Path) -> Result<Self> {
        Self::from_metadata(&std::fs::symlink_metadata(path)?)
    }
}

impl Lock {
    pub(super) fn acquire(path: &Path, create: bool) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(create)
            .open(path)?;
        file.try_lock().map_err(|_| Error::Busy)?;
        if create {
            file.sync_all()?;
            File::open(path.parent().ok_or(Error::Ownership)?)?.sync_all()?;
        }
        Ok(Self(file))
    }

    pub(super) fn identity(&self) -> Result<LockIdentity> {
        LockIdentity::from_metadata(&self.0.metadata()?)
    }

    pub(super) fn verify(&self, path: &Path, expected: LockIdentity) -> Result<()> {
        if self.identity()? != expected || LockIdentity::at_path(path)? != expected {
            return Err(Error::Ownership);
        }
        Ok(())
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

pub(super) fn create_root(root: &Path) -> Result<()> {
    crate::session_store::require_directory_durability()?;
    if !root.is_absolute() {
        return Err(Error::Ownership);
    }
    fs::create_dir(root)?;
    File::open(root.parent().ok_or(Error::Journal)?)?.sync_all()?;
    Ok(())
}
