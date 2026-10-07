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
        let source = open_anchored(root, &app.artifact)?;
        Self::capture_file(source, platform)
    }

    /// # Errors
    /// Trusted host capture relative to its retained workspace directory, never a reopened path.
    /// This prevents root replacement between workspace admission and artifact capture.
    pub fn capture_directory(root: &File, contract: &Contract, platform: Platform) -> Result<Self> {
        contract.validate().map_err(|_| Error::ArtifactRejected)?;
        let app = contract.apps.get(&platform).ok_or(Error::ArtifactRejected)?;
        Self::capture_file(open_directory(root, &app.artifact)?, platform)
    }

    fn capture_file(source: File, platform: Platform) -> Result<Self> {
        Self::capture_with(source, platform, |_| ())
    }

    fn capture_with(mut source: File, platform: Platform, mut after_chunk: impl FnMut(u64)) -> Result<Self> {
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
            after_chunk(total);
        }
        let after = source.metadata().map_err(|_| Error::ArtifactChanged)?;
        if total != before.len() || !unchanged(&before, &after) {
            return Err(Error::ArtifactChanged);
        }
        // Re-read the retained source; path replacement never changes the descriptor.
        let digest = hash.finalize();
        source.rewind().map_err(|_| Error::ArtifactChanged)?;
        let mut verify = Sha256::new();
        let mut verified = 0_u64;
        loop {
            let count = source.read(&mut buffer).map_err(|_| Error::ArtifactChanged)?;
            if count == 0 {
                break;
            }
            verified += count as u64;
            if verified > MAX_BYTES {
                return Err(Error::ArtifactChanged);
            }
            verify.update(&buffer[..count]);
        }
        let final_metadata = source.metadata().map_err(|_| Error::ArtifactChanged)?;
        if verified != total || verify.finalize() != digest || !unchanged(&before, &final_metadata) {
            return Err(Error::ArtifactChanged);
        }
        if magic != b"PK\x03\x04" {
            return Err(Error::ArtifactRejected);
        }
        captured.seek(SeekFrom::Start(0)).map_err(|_| Error::ArtifactRejected)?;
        let sha256 = digest.iter().fold(String::with_capacity(64), |mut text, byte| {
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
    use rustix::fs::{Mode, OFlags, open};
    let root = root.canonicalize().map_err(|_| Error::ArtifactRejected)?;
    let directory = open(
        &root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| Error::ArtifactRejected)?;
    open_directory(&File::from(directory), relative)
}

#[cfg(unix)]
fn open_directory(root: &File, relative: &Path) -> Result<File> {
    use rustix::fs::{Mode, OFlags, openat};
    if !root.metadata().map_err(|_| Error::ArtifactRejected)?.is_dir()
        || relative.components().any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(Error::ArtifactRejected);
    }
    let mut directory = root.try_clone().map_err(|_| Error::ArtifactRejected)?;
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
        directory = File::from(fd);
    }
    Err(Error::ArtifactRejected)
}

#[cfg(not(unix))]
fn open_anchored(_root: &Path, _relative: &Path) -> Result<File> {
    Err(Error::ArtifactRejected)
}

#[cfg(not(unix))]
fn open_directory(_root: &File, _relative: &Path) -> Result<File> {
    Err(Error::ArtifactRejected)
}

fn unchanged(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Unlike mtime, an unprivileged writer cannot restore the inode change timestamp.
        (before.dev(), before.ino(), before.ctime(), before.ctime_nsec())
            == (after.dev(), after.ino(), after.ctime(), after.ctime_nsec())
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(all(test, unix))]
mod mutation_tests {
    use super::*;
    #[test]
    fn same_size_rewrite_with_restored_mtime_cannot_publish_mixed_bytes() {
        let mut source = tempfile::NamedTempFile::new().unwrap();
        let mut bytes = vec![b'A'; 16384];
        bytes[..4].copy_from_slice(b"PK\x03\x04");
        source.write_all(&bytes).unwrap();
        source.flush().unwrap();
        let original = source.as_file().metadata().unwrap().modified().unwrap();
        let path = source.path().to_owned();
        let opened = File::open(&path).unwrap();
        let mut changed = false;
        let result = Artifact::capture_with(opened, Platform::Android, |total| {
            if total == 8192 && !changed {
                changed = true;
                let mut writer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
                writer.seek(SeekFrom::Start(8192)).unwrap();
                writer.write_all(&vec![b'B'; 8192]).unwrap();
                writer
                    .set_times(std::fs::FileTimes::new().set_modified(original))
                    .unwrap();
                assert_eq!(writer.metadata().unwrap().modified().unwrap(), original);
            }
        });
        assert!(changed);
        assert!(matches!(result, Err(Error::ArtifactChanged)));
    }
    #[test]
    fn verified_capture_remains_immutable_after_the_source_is_rewritten() {
        let mut source = tempfile::NamedTempFile::new().unwrap();
        source.write_all(b"PK\x03\x04stable").unwrap();
        source.flush().unwrap();
        let mut artifact = Artifact::capture_file(File::open(source.path()).unwrap(), Platform::Ios).unwrap();
        std::fs::write(source.path(), b"PK\x03\x04other!").unwrap();
        let mut captured = Vec::new();
        artifact.reader().unwrap().read_to_end(&mut captured).unwrap();
        assert_eq!(captured, b"PK\x03\x04stable");
    }
}
