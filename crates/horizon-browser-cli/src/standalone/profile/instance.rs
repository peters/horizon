//! The stable host instance of a profile host and the record of the host
//! that runs. `--connect` clients read the instance from the record, so it
//! stays the same over each restart of the host on the same runtime root.

use std::fs::File;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The record of the host under the runtime root, mode 600.
pub const RECORD: &str = "profile-host.json";
const LOCK: &str = "profile-host.lock";

/// What `profile-host.json` holds.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Record {
    pub host_instance: String,
    pub pid: u32,
    /// Unix milliseconds of the start of the host that wrote the record.
    pub started_at: i64,
}

impl Record {
    /// The record under `root`, if there is a valid one.
    #[must_use]
    pub fn read(root: &Path) -> Option<Self> {
        let record: Self = serde_json::from_slice(&std::fs::read(root.join(RECORD)).ok()?).ok()?;
        horizon_browser_control::manifest::valid_host_instance(&record.host_instance).then_some(record)
    }
}

/// The lock of the one host on a root, and the instance it keeps.
pub struct Claim {
    _lock: File,
    host_instance: String,
}

impl Claim {
    /// Lock the root for this host and take the instance of the record, or a
    /// new one when the root has none.
    ///
    /// # Errors
    /// Another host holds the lock, or the root cannot be prepared.
    pub fn take(root: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(root)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join(LOCK))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("another profile host runs on {}", root.display()),
                ));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
        let host_instance =
            Record::read(root).map_or_else(|| uuid::Uuid::new_v4().to_string(), |record| record.host_instance);
        Ok(Self {
            _lock: lock,
            host_instance,
        })
    }

    #[must_use]
    pub fn host_instance(&self) -> &str {
        &self.host_instance
    }

    /// Write the record of this host once it serves requests.
    ///
    /// # Errors
    /// The record cannot be written.
    pub fn ready(&self, root: &Path) -> io::Result<()> {
        let record = Record {
            host_instance: self.host_instance.clone(),
            pid: std::process::id(),
            started_at: horizon_browser_control::manifest::now_millis(),
        };
        let bytes = serde_json::to_vec_pretty(&record).map_err(io::Error::other)?;
        super::host::write_private(&root.join(RECORD), &bytes)
    }
}
