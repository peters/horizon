//! Release the session lock explicitly, even when a spawned child inherited a descriptor.
use std::{
    fs::File,
    path::{Path, PathBuf},
};

pub(crate) struct SessionLock {
    file: File,
    root: PathBuf,
}

impl SessionLock {
    pub(super) fn new(file: File, root: &Path) -> Self {
        Self {
            file,
            root: root.to_owned(),
        }
    }

    pub(super) fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        if let Err(error) = self.file.unlock() {
            tracing::warn!(%error, "could not explicitly release the sign-in session lock");
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn a_duplicate_descriptor_does_not_keep_a_completed_operation_locked() {
        let root = tempfile::tempdir().unwrap();
        let guard = super::super::session_lock(root.path()).unwrap();
        // A duplicate shares the same open-file description as an inherited Unix fd.
        let inherited = guard.file.try_clone().unwrap();
        drop(guard);
        let next = super::super::session_lock(root.path()).unwrap();
        assert!(inherited.metadata().is_ok(), "the duplicate is still open");
        drop(next);
        drop(inherited);
    }
}
