use std::fmt::Write as _;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::path::Component;
use std::path::Path;

use horizon_app_testing::contract::{Contract, Platform, project_path};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Immutable, host-private capture of an opened declared artifact. No path is reopened during upload.
pub struct Artifact {
    platform: Platform,
    sha256: String,
    bytes: u64,
    captured: File,
}

impl Artifact {
    /// # Errors
    /// Refuses undeclared files, symlinks, non-regular files, oversized files and changing input.
    pub fn capture(root: &Path, contract: &Contract, platform: Platform) -> Result<Self> {
        contract.validate().map_err(|_| Error::ArtifactRejected)?;
        let app = contract.apps.get(&platform).ok_or(Error::ArtifactRejected)?;
        project_path(root, &app.artifact).map_err(|_| Error::ArtifactRejected)?;
        let mut source = open_anchored(root, &app.artifact)?;
        let before = source.metadata().map_err(|_| Error::ArtifactRejected)?;
        if !before.is_file() || !(4..=MAX_BYTES).contains(&before.len()) {
            return Err(Error::ArtifactRejected);
        }
        let mut captured = tempfile::tempfile().map_err(|_| Error::ArtifactRejected)?;
        let mut hash = Sha256::new();
        let mut total = 0_u64;
        let mut buffer = [0_u8; 8192];
        let mut magic = Vec::with_capacity(4);
        loop {
            let count = source.read(&mut buffer).map_err(|_| Error::ArtifactChanged)?;
            if count == 0 {
                break;
            }
            total += u64::try_from(count).map_err(|_| Error::ArtifactRejected)?;
            if total > MAX_BYTES {
                return Err(Error::ArtifactRejected);
            }
            if magic.len() < 4 {
                magic.extend_from_slice(&buffer[..count.min(4 - magic.len())]);
            }
            hash.update(&buffer[..count]);
            captured
                .write_all(&buffer[..count])
                .map_err(|_| Error::ArtifactRejected)?;
        }
        let after = source.metadata().map_err(|_| Error::ArtifactChanged)?;
        if total != before.len() || before.len() != after.len() || before.modified().ok() != after.modified().ok() {
            return Err(Error::ArtifactChanged);
        }
        if magic != b"PK\x03\x04" {
            return Err(Error::ArtifactRejected);
        }
        captured.seek(SeekFrom::Start(0)).map_err(|_| Error::ArtifactRejected)?;
        let sha256 = hash
            .finalize()
            .iter()
            .fold(String::with_capacity(64), |mut text, byte| {
                let _ = write!(text, "{byte:02x}");
                text
            });
        Ok(Self {
            platform,
            sha256,
            bytes: total,
            captured,
        })
    }

    #[must_use]
    pub fn platform(&self) -> Platform {
        self.platform
    }
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub(crate) fn reader(&mut self) -> Result<&mut File> {
        self.captured.rewind().map_err(|_| Error::ArtifactRejected)?;
        Ok(&mut self.captured)
    }
}

#[cfg(unix)]
fn open_anchored(root: &Path, relative: &Path) -> Result<File> {
    use rustix::fs::{Mode, OFlags, open, openat};
    let root = root.canonicalize().map_err(|_| Error::ArtifactRejected)?;
    let mut directory = open(
        &root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| Error::ArtifactRejected)?;
    let mut parts = relative.components().peekable();
    while let Some(Component::Normal(part)) = parts.next() {
        let last = parts.peek().is_none();
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
        let fd = openat(
            &directory,
            part,
            if last { flags } else { flags | OFlags::DIRECTORY },
            Mode::empty(),
        )
        .map_err(|_| Error::ArtifactRejected)?;
        if last {
            return Ok(File::from(fd));
        }
        directory = fd;
    }
    Err(Error::ArtifactRejected)
}

#[cfg(not(unix))]
fn open_anchored(_root: &Path, _relative: &Path) -> Result<File> {
    Err(Error::ArtifactRejected)
}
