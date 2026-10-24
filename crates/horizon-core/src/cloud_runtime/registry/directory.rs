//! Recoverable private directory creation before a registry generation is pinned.
use super::Result;
use std::path::Path;

// Directory fsync and no-follow private coordination-file opens require Unix.
#[cfg(unix)]
mod unix;

pub(super) fn ensure_private_root(root: &Path) -> Result<()> {
    crate::session_store::require_directory_durability()?;
    #[cfg(unix)]
    {
        unix::ensure(root)
    }
}
