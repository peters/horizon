//! Cross-instance provider quota: every Horizon instance on this computer
//! that shares a provider identity (endpoint plus credential reference) draws
//! from the same `max_sessions` through slot files under the Horizon home. A
//! slot is an exclusively locked file; the lock is advisory, held for as long
//! as the lease lives, and freed by the operating system if the process dies.
//! Provider quotas stay authoritative; this only stops two instances from
//! believing they each own the whole quota.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use horizon_browser::remote::{RemoteAuthentication, RemoteProviderProfile};
use sha2::{Digest, Sha256};

/// Directory name under the Horizon home that holds the slot files.
pub const SLOTS_DIR: &str = "remote-slots";

/// The identity two instances must share to share a quota: the provider's
/// control endpoint and the name of the credential reference that
/// authenticates it (not the credential itself). Hashed so the key is a plain
/// path segment that never carries the endpoint or a reference name.
#[must_use]
pub fn quota_key(profile: &RemoteProviderProfile) -> String {
    let reference = match &profile.authentication {
        RemoteAuthentication::None {} => "none".to_string(),
        RemoteAuthentication::Basic { username_ref, .. } => username_ref.as_str().to_string(),
        RemoteAuthentication::Bearer { token_ref } => token_ref.as_str().to_string(),
    };
    let mut hasher = Sha256::new();
    hasher.update(profile.endpoint.as_str().as_bytes());
    hasher.update(b"\n");
    hasher.update(reference.as_bytes());
    let digest = hasher.finalize();
    let mut key = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(key, "{byte:02x}");
    }
    key
}

/// Why no slot could be leased.
#[derive(Debug)]
pub enum SlotError {
    /// Every slot up to `max_sessions` is held, by this or another instance.
    Busy { max_sessions: u32 },
    /// The slot directory or a slot file could not be used.
    Io(io::Error),
}

impl std::fmt::Display for SlotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy { max_sessions } => write!(
                formatter,
                "all {max_sessions} provider sessions are held by Horizon instances on this computer"
            ),
            Self::Io(error) => write!(formatter, "provider slot files unavailable: {error}"),
        }
    }
}

impl std::error::Error for SlotError {}

/// One held provider slot. Dropping it frees the slot for the next
/// allocation on this computer.
#[derive(Debug)]
pub struct SlotLease {
    _file: File,
    path: PathBuf,
}

impl SlotLease {
    /// The slot file this lease holds, for diagnostics.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Lease one of `max_sessions` slots for `key` under `root` (the Horizon
/// home), trying each slot in order and taking the first that is free. One
/// non-blocking pass: this runs on the host's frame, so it never waits. A
/// child process forked by this or another process inherits open lock
/// descriptors until it execs, so a slot freed a moment ago can read as
/// held for a few milliseconds; the caller's next attempt sees it free.
///
/// # Errors
/// [`SlotError::Busy`] when every slot is held; [`SlotError::Io`] when the
/// directory or a slot file cannot be created or opened.
pub fn acquire_slot(root: &Path, key: &str, max_sessions: u32) -> Result<SlotLease, SlotError> {
    let dir = root.join(SLOTS_DIR).join(key);
    fs::create_dir_all(&dir).map_err(SlotError::Io)?;
    for index in 0..max_sessions {
        let path = dir.join(format!("slot-{index}.lock"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(SlotError::Io)?;
        match file.try_lock() {
            Ok(()) => return Ok(SlotLease { _file: file, path }),
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(error)) => return Err(SlotError::Io(error)),
        }
    }
    Err(SlotError::Busy { max_sessions })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use horizon_browser::remote::{
        ControlEndpoint, CredentialReference, RemoteAdapterKind, RemoteAuthentication, RemoteProviderProfile,
        RemoteSessionLimits,
    };

    use super::{SlotError, acquire_slot, quota_key};

    fn profile(endpoint: &str, authentication: RemoteAuthentication) -> RemoteProviderProfile {
        RemoteProviderProfile {
            adapter: RemoteAdapterKind::Webdriver,
            endpoint: ControlEndpoint::parse(endpoint).expect("endpoint"),
            authentication,
            credential_bindings: BTreeMap::new(),
            limits: RemoteSessionLimits::default(),
        }
    }

    #[test]
    fn the_quota_key_follows_endpoint_and_reference_and_names_neither() {
        let basic = profile(
            "https://grid.example.net/wd/hub",
            RemoteAuthentication::Basic {
                username_ref: CredentialReference::from("user"),
                password_ref: CredentialReference::from("key"),
            },
        );
        let key = quota_key(&basic);
        assert_eq!(key.len(), 64);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!key.contains("grid") && !key.contains("user"));
        assert_eq!(quota_key(&basic), key, "stable");
        let other_reference = profile(
            "https://grid.example.net/wd/hub",
            RemoteAuthentication::Basic {
                username_ref: CredentialReference::from("other"),
                password_ref: CredentialReference::from("key"),
            },
        );
        assert_ne!(quota_key(&other_reference), key, "another credential is another quota");
        let other_endpoint = profile(
            "https://grid2.example.net/wd/hub",
            RemoteAuthentication::Basic {
                username_ref: CredentialReference::from("user"),
                password_ref: CredentialReference::from("key"),
            },
        );
        assert_ne!(quota_key(&other_endpoint), key, "another endpoint is another quota");
        let unauthenticated = profile("http://127.0.0.1:4444", RemoteAuthentication::None {});
        assert_ne!(quota_key(&unauthenticated), key);
    }

    #[test]
    fn slots_are_leased_up_to_the_limit_and_freed_on_drop() {
        let root = tempfile::tempdir().expect("tempdir");
        let first = acquire_slot(root.path(), "k", 2).expect("first slot");
        let second = acquire_slot(root.path(), "k", 2).expect("second slot");
        assert_ne!(first.path(), second.path());
        match acquire_slot(root.path(), "k", 2) {
            Err(SlotError::Busy { max_sessions: 2 }) => {}
            other => panic!("expected busy, got {other:?}"),
        }
        assert!(
            acquire_slot(root.path(), "other", 1).is_ok(),
            "another key is another quota"
        );
        drop(first);
        // Another test's forked child may hold the freed descriptor until it
        // execs (the fork-inheritance window); the caller's retry sees it free.
        let reused = (0..100)
            .find_map(|_| match acquire_slot(root.path(), "k", 2) {
                Ok(lease) => Some(lease),
                Err(SlotError::Busy { .. }) => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    None
                }
                Err(error) => panic!("{error}"),
            })
            .expect("freed slot");
        assert_eq!(reused.path().file_name().and_then(|n| n.to_str()), Some("slot-0.lock"));
        match acquire_slot(root.path(), "k", 0) {
            Err(SlotError::Busy { max_sessions: 0 }) => {}
            other => panic!("a zero quota leases nothing, got {other:?}"),
        }
    }
}
