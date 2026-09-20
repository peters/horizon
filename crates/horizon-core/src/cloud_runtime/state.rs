//! Durable allocation fence and per-cloud ownership lock.
use super::{Error, Result, Stage};
use horizon_cloud::{CreateState, Worker, WorkerSpec};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Session {
    pub panel_id: String,
    pub agent: String,
    pub tmux: String,
    pub branch: String,
    pub worktree: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Deployment {
    pub version: u32,
    pub cloud_id: String,
    pub repository: PathBuf,
    pub revision: String,
    pub profile: horizon_cloud::Profile,
    pub stage: Stage,
    pub operation: CreateState,
    pub spec: Option<WorkerSpec>,
    pub worker: Option<Worker>,
    pub sessions: Vec<Session>,
    #[serde(default)]
    pub source_ready: bool,
    #[serde(default)]
    pub stop_requested: bool,
}
pub struct Store {
    root: PathBuf,
    lock_file: File,
}
impl Store {
    /// # Errors
    /// Refuses simultaneous controllers. OS locks release after a crash.
    pub fn lock(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("operation.lock"))?;
        file.try_lock().map_err(|_| Error::Busy)?;
        Ok(Self {
            root: root.into(),
            lock_file: file,
        })
    }
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// # Errors
    /// Returns read/parse errors without resetting or ignoring corrupt state.
    pub fn load(&self) -> Result<Option<Deployment>> {
        let path = self.root.join("deployment.json");
        match std::fs::read(path) {
            Ok(bytes) => {
                let state: Deployment = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
                if state.version != 1 {
                    return Err(Error::Invalid("Unsupported cloud state"));
                }
                Ok(Some(state))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    /// # Errors
    /// Syncs file contents and parent directory before returning to the provider.
    pub fn save(&self, state: &Deployment) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(state).map_err(|_| Error::Json)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(self.root.join("deployment.json")).map_err(|e| e.error)?;
        #[cfg(unix)]
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = self.lock_file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn competing_controllers_cannot_both_hold_operation_lock() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::lock(temp.path()).unwrap();
        assert!(matches!(Store::lock(temp.path()), Err(Error::Busy)));
        drop(store);
        assert!(Store::lock(temp.path()).is_ok());
    }
}
