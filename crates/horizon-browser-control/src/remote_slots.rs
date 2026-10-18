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

/// How long the per-key coordination lock is retried before contention is
/// reported. Another instance holds it for one directory scan, so this is
/// never reached in practice; it bounds what the host's frame can wait.
const COORDINATION_WINDOW: std::time::Duration = std::time::Duration::from_millis(20);
const COORDINATION_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1);

/// Why no slot could be leased.
#[derive(Debug)]
pub enum SlotError {
    /// Every slot up to `max_sessions` is held, by this or another instance.
    Busy { max_sessions: u32 },
    /// Another instance was checking the same quota for longer than the
    /// bounded wait; nothing was decided and the caller may try again.
    Contended,
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
            Self::Contended => formatter.write_str("another Horizon instance is checking the same provider quota"),
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
/// home). Every slot file in the key's directory counts, whatever its index:
/// a lease taken under a larger quota (`slot-3` when the quota was four)
/// still occupies one of the sessions after the quota is reduced to two.
/// The scan and the grant happen under a per-key coordination lock so two
/// acquirers cannot both count the same free slot; that lock is held for
/// the scan only, never while a session runs. Slot locks themselves are
/// tried without waiting: this runs on the host's frame. A child process
/// forked by this or another process inherits open lock descriptors until
/// it execs, so a slot freed a moment ago can read as held for a few
/// milliseconds; the caller's next attempt sees it free.
///
/// # Errors
/// [`SlotError::Busy`] when `max_sessions` slots are already held;
/// [`SlotError::Io`] when the directory or a slot file cannot be used.
pub fn acquire_slot(root: &Path, key: &str, max_sessions: u32) -> Result<SlotLease, SlotError> {
    let dir = root.join(SLOTS_DIR).join(key);
    fs::create_dir_all(&dir).map_err(SlotError::Io)?;
    let coordination = open_slot_file(&dir.join("coordination.lock"))?;
    let deadline = std::time::Instant::now() + COORDINATION_WINDOW;
    loop {
        match coordination.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(COORDINATION_INTERVAL);
            }
            Err(std::fs::TryLockError::WouldBlock) => return Err(SlotError::Contended),
            Err(std::fs::TryLockError::Error(error)) => return Err(SlotError::Io(error)),
        }
    }
    let mut held = 0u32;
    let mut free: Option<SlotLease> = None;
    let mut present = std::collections::BTreeSet::new();
    for entry in fs::read_dir(&dir).map_err(SlotError::Io)? {
        let path = entry.map_err(SlotError::Io)?.path();
        let Some(index) = slot_index(&path) else {
            continue;
        };
        present.insert(index);
        let file = open_slot_file(&path)?;
        match file.try_lock() {
            Ok(()) => {
                if free.is_none() {
                    free = Some(SlotLease { _file: file, path });
                }
            }
            Err(std::fs::TryLockError::WouldBlock) => held += 1,
            Err(std::fs::TryLockError::Error(error)) => return Err(SlotError::Io(error)),
        }
    }
    if held >= max_sessions {
        return Err(SlotError::Busy { max_sessions });
    }
    if let Some(lease) = free {
        return Ok(lease);
    }
    // At most `present.len()` indexes are taken, so the first free index is
    // at or below that count.
    let index = (0..=u32::try_from(present.len()).unwrap_or(u32::MAX))
        .find(|index| !present.contains(index))
        .unwrap_or_default();
    let path = dir.join(format!("slot-{index}.lock"));
    let file = open_slot_file(&path)?;
    match file.try_lock() {
        Ok(()) => Ok(SlotLease { _file: file, path }),
        Err(std::fs::TryLockError::WouldBlock) => Err(SlotError::Busy { max_sessions }),
        Err(std::fs::TryLockError::Error(error)) => Err(SlotError::Io(error)),
    }
}

fn open_slot_file(path: &Path) -> Result<File, SlotError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(SlotError::Io)
}

/// The index of a `slot-N.lock` file name, or `None` for anything else.
fn slot_index(path: &Path) -> Option<u32> {
    path.file_name()?
        .to_str()?
        .strip_prefix("slot-")?
        .strip_suffix(".lock")?
        .parse()
        .ok()
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

    fn acquire_after_drop(root: &std::path::Path, key: &str, max_sessions: u32) -> super::SlotLease {
        // Another test's forked child may hold the freed descriptor until exec.
        (0..100)
            .find_map(|_| match acquire_slot(root, key, max_sessions) {
                Ok(lease) => Some(lease),
                Err(SlotError::Busy { .. }) => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    None
                }
                Err(error) => panic!("{error}"),
            })
            .expect("freed slot within the fork-inheritance window")
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
        let reused = acquire_after_drop(root.path(), "k", 2);
        assert_eq!(reused.path().file_name().and_then(|n| n.to_str()), Some("slot-0.lock"));
        match acquire_slot(root.path(), "k", 0) {
            Err(SlotError::Busy { max_sessions: 0 }) => {}
            other => panic!("a zero quota leases nothing, got {other:?}"),
        }
    }

    #[test]
    fn a_lease_taken_under_a_larger_quota_still_counts_after_the_quota_shrinks() {
        let root = tempfile::tempdir().expect("tempdir");
        let first = acquire_slot(root.path(), "k", 4).expect("slot 0");
        let second = acquire_slot(root.path(), "k", 4).expect("slot 1");
        let high = acquire_slot(root.path(), "k", 4).expect("slot 2");
        assert!(high.path().ends_with("slot-2.lock"));
        drop(first);
        drop(second);
        // The quota is now two: the lease on slot 2 is one of them.
        let one_more = acquire_after_drop(root.path(), "k", 2);
        match acquire_slot(root.path(), "k", 2) {
            Err(SlotError::Busy { max_sessions: 2 }) => {}
            other => panic!("the high slot and the new lease fill the quota, got {other:?}"),
        }
        drop(one_more);
        drop(high);
        let _available = acquire_after_drop(root.path(), "k", 2);
    }

    #[test]
    fn a_held_coordination_lock_reports_contention_within_the_bound() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join(super::SLOTS_DIR).join("k");
        std::fs::create_dir_all(&dir).expect("dir");
        let coordination = super::open_slot_file(&dir.join("coordination.lock")).expect("file");
        coordination.lock().expect("hold the coordination lock");
        let started = std::time::Instant::now();
        match acquire_slot(root.path(), "k", 2) {
            Err(SlotError::Contended) => {}
            other => panic!("expected contention, got {other:?}"),
        }
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "the wait is bounded"
        );
        drop(coordination);
        assert!(acquire_slot(root.path(), "k", 2).is_ok());
    }
}
