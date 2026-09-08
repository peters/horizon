//! Worker-owned setup admission, not a runner, retry policy or completion receipt.
//! The nominated root must be retained independently of runtime/client generations.
//! Its private ancestry and mount configuration must remain trusted and stable.

#[cfg(any(target_os = "linux", test))]
mod codec;
mod intent;
#[cfg(target_os = "linux")]
mod linux;

pub use intent::SetupIntent;
#[cfg(target_os = "linux")]
use std::sync::Arc;
use std::{fmt, path::Path};

const SCRATCH_NAME: &str = "setup-data";

/// One fixed claim slot in an existing, privately owned retained workspace root.
pub struct RetainedSetup {
    #[cfg(target_os = "linux")]
    directory: Arc<linux::Directory>,
}

/// Read-only observation cannot establish synchronization, liveness or completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupObservation {
    Absent,
    ClaimedUnknown,
}

#[derive(Debug)]
pub enum SetupAdmission {
    Fresh(SetupGrant),
    Existing,
}

/// Only a freshly synchronized admission can construct this non-cloneable grant.
/// Dropping it retains the claim. It cannot be reconstructed from stored bytes.
pub struct SetupGrant {
    intent: SetupIntent,
    #[cfg(target_os = "linux")]
    _directory: Arc<linux::Directory>,
}

impl SetupGrant {
    #[must_use]
    pub fn intent(&self) -> &SetupIntent {
        &self.intent
    }
}

impl fmt::Debug for SetupGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetupGrant").finish_non_exhaustive()
    }
}

impl RetainedSetup {
    /// Open existing private, qualified storage; never create or repair it.
    /// Run off the UI thread; filesystem byte bounds do not bound I/O latency.
    /// # Errors
    /// Rejects missing, unsupported, untrusted or invalid roots without fallback.
    pub fn open(root: &Path) -> Result<Self, SetupClaimError> {
        if !super::materialize::valid_path(&root.join(SCRATCH_NAME)) {
            return Err(SetupClaimError::InvalidIntent);
        }
        #[cfg(target_os = "linux")]
        return Ok(Self {
            directory: Arc::new(linux::Directory::open(root)?),
        });
        #[cfg(not(target_os = "linux"))]
        Err(SetupClaimError::Unsupported)
    }

    /// Observe only: no creation, synchronization, expiry, repair or execution.
    /// # Errors
    /// Read/parse failures never mean absence; changed intent is a conflict.
    pub fn observe(&self, intent: &SetupIntent) -> Result<SetupObservation, SetupClaimError> {
        #[cfg(target_os = "linux")]
        return self.directory.observe(intent);
        #[cfg(not(target_os = "linux"))]
        {
            let _ = intent;
            Err(SetupClaimError::Unsupported)
        }
    }

    /// Admit at most once per retained root, independent of retry/client identity.
    /// Existing identical claims never receive a new grant, even without a result.
    /// # Errors
    /// Any uncertain publication or synchronization returns no grant. Retain the
    /// root for inspection: an error is not permission to delete it or replay setup.
    pub fn admit(&self, intent: SetupIntent) -> Result<SetupAdmission, SetupClaimError> {
        #[cfg(target_os = "linux")]
        return match self.directory.admit(&intent)? {
            linux::Admission::Fresh => Ok(SetupAdmission::Fresh(SetupGrant {
                intent,
                _directory: Arc::clone(&self.directory),
            })),
            linux::Admission::Existing => Ok(SetupAdmission::Existing),
        };
        #[cfg(not(target_os = "linux"))]
        {
            let _ = intent;
            Err(SetupClaimError::Unsupported)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SetupClaimError {
    #[error("retained setup requires a valid immutable intent")]
    InvalidIntent,
    #[error("retained setup requires supported journaled storage and confinement")]
    Unsupported,
    #[error("retained setup root is missing, changed or not privately owned")]
    UnsafeRoot,
    #[error("retained setup claim cannot be safely read")]
    Read,
    #[error("retained setup claim is invalid or exceeds its bound")]
    InvalidRecord,
    #[error("retained setup intent conflicts with the existing claim")]
    Conflict,
    #[error("retained setup storage operation failed; no execution grant was issued")]
    Storage,
}

#[cfg(test)]
mod tests;
