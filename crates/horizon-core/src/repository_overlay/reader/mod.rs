//! Bounded reads of individually selected nodes, not repository capture or export approval.
//! Linux confinement is required; unsupported platforms fail without a weaker fallback.

#[cfg(target_os = "linux")]
pub(super) mod linux;

use super::{OverlayPlanError, paths};
use std::{
    fmt,
    path::{Component, Path},
};

/// Maximum regular-file payload per read; larger files require a future streaming boundary.
pub const MAX_READ_BYTES: usize = 64 * 1024 * 1024;

/// Local bytes or a literal link target. Does not certify content safety or a coherent snapshot.
#[derive(Eq, PartialEq)]
pub enum SelectedRepositoryNode {
    File { bytes: Vec<u8>, executable: bool },
    Symlink { target: String },
}

impl fmt::Debug for SelectedRepositoryNode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File { bytes, executable } => formatter
                .debug_struct("File")
                .field("bytes", &bytes.len())
                .field("executable", executable)
                .finish_non_exhaustive(),
            Self::Symlink { .. } => formatter.debug_struct("Symlink").finish_non_exhaustive(),
        }
    }
}

/// Pins an explicitly selected directory object; renaming it does not select a replacement.
/// This does not discover or verify a Git root, enumerate files or authorize export.
pub struct SelectedRepositoryReader {
    #[cfg(target_os = "linux")]
    pub(super) root: linux::Root,
}

impl SelectedRepositoryReader {
    /// Open only the nominated absolute directory, with no symlinked ancestors.
    /// # Errors
    /// Rejects unsupported confinement, invalid roots and inaccessible directories.
    pub fn open(root: &Path) -> Result<Self, RepositoryReadError> {
        if !root.is_absolute() || root.parent().is_none() || root.components().any(|part| part == Component::ParentDir)
        {
            return Err(RepositoryReadError::InvalidRoot);
        }
        #[cfg(target_os = "linux")]
        {
            Ok(Self {
                root: linux::Root::open(root)?,
            })
        }
        #[cfg(not(target_os = "linux"))]
        Err(RepositoryReadError::Unsupported)
    }

    /// Read one allowed path without following its symlink target or linked parents.
    /// Byte bounds do not bound wall-clock time on a blocked filesystem. Run off the UI thread.
    /// # Errors
    /// Rejects invalid policy/limits, unsupported nodes, unsafe resolution and detected changes.
    pub fn read(&self, path: &str, byte_limit: usize) -> Result<SelectedRepositoryNode, RepositoryReadError> {
        paths::validate(path)?;
        if byte_limit > MAX_READ_BYTES {
            return Err(RepositoryReadError::InvalidLimit);
        }
        #[cfg(target_os = "linux")]
        {
            self.root.read(path, byte_limit)
        }
        #[cfg(not(target_os = "linux"))]
        Err(RepositoryReadError::Unsupported)
    }
}

/// Redacted errors never include filesystem paths or contents.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RepositoryReadError {
    #[error("safe repository reads are unsupported on this platform or kernel")]
    Unsupported,
    #[error("repository reader requires a selected absolute directory")]
    InvalidRoot,
    #[error("repository read limit exceeds the supported payload bound")]
    InvalidLimit,
    #[error(transparent)]
    Policy(#[from] OverlayPlanError),
    #[error("selected repository node is missing")]
    Missing,
    #[error("selected repository path cannot be safely confined")]
    UnsafePath,
    #[error("selected repository node type or hardlink is unsupported")]
    UnsupportedNode,
    #[error("selected repository node changed during the read")]
    Changed,
    #[error("selected repository node exceeds the read limit")]
    TooLarge,
    #[error("selected repository node could not be read")]
    ReadFailed,
}

#[cfg(all(test, target_os = "linux"))]
mod tests;

#[cfg(all(test, not(target_os = "linux")))]
mod unsupported_tests {
    use super::*;

    #[test]
    fn unsupported_platform_never_falls_back_to_unconfined_reads() {
        let directory = tempfile::tempdir().expect("fixture");
        assert!(matches!(
            SelectedRepositoryReader::open(directory.path()),
            Err(RepositoryReadError::Unsupported)
        ));
        let reader = SelectedRepositoryReader {};
        assert_eq!(reader.read("file", 10), Err(RepositoryReadError::Unsupported));
    }
}
