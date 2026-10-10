//! The lease file that marks a session as open in a running Horizon.
use std::fs;
use std::io::ErrorKind;

use super::{SESSION_LEASE_VERSION, SessionLease, SessionStore, atomic_write, current_unix_millis};
use crate::error::{Error, Result};

impl SessionStore {
    /// Create or replace the lease file for an active session.
    ///
    /// # Errors
    ///
    /// Returns an error if the lease directory cannot be created or if the
    /// lease file cannot be serialized and written.
    pub fn acquire_lease(&self, session_id: &str) -> Result<SessionLease> {
        let lease_path = self.home.session_lease_path(session_id);
        if let Some(parent) = lease_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let lease = SessionLease::new(session_id.to_string());
        let json = serde_json::to_vec_pretty(&lease).map_err(|error| Error::State(error.to_string()))?;
        atomic_write(&lease_path, &json)?;
        Ok(lease)
    }

    /// Update the heartbeat timestamp on an existing session lease.
    ///
    /// # Errors
    ///
    /// Returns an error if the refreshed lease cannot be serialized or written.
    pub fn refresh_lease(&self, lease: &mut SessionLease) -> Result<()> {
        lease.last_heartbeat_at = current_unix_millis();
        let json = serde_json::to_vec_pretty(lease).map_err(|error| Error::State(error.to_string()))?;
        atomic_write(&self.home.session_lease_path(&lease.session_id), &json)?;
        Ok(())
    }

    /// Remove a session lease file if it exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the lease file exists but cannot be removed.
    pub fn release_lease(&self, session_id: &str) -> Result<()> {
        let path = self.home.session_lease_path(session_id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn load_session_lease(&self, session_id: &str) -> Result<Option<SessionLease>> {
        let path = self.home.session_lease_path(session_id);
        if !path.exists() {
            return Ok(None);
        }

        let contents = fs::read_to_string(path)?;
        let mut lease =
            serde_json::from_str::<SessionLease>(&contents).map_err(|error| Error::State(error.to_string()))?;
        lease.version = SESSION_LEASE_VERSION;
        Ok(Some(lease))
    }
}
