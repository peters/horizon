//! Exclusive host actor ownership, shared by CLI, MCP and live native panels.
use super::store::Initialization;
use super::store::execution::Identity;
use super::{Journal, Kind, Operation, Phase};
use crate::{Error, Result};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Trusted workspace lease. Neither paths nor provider references are agent-selected values.
/// Keep this lease alive for the entire actor, including all concurrent device lanes.
pub struct Workspace {
    journal: Arc<Journal>,
    owner: Uuid,
    root: PathBuf,
    _lease: Lease,
    _root_lease: Lease,
    root_file: File,
    reconcile: bool,
}

// Explicit unlock also releases the lease when a concurrently spawned child briefly
// inherits the open file description before exec closes its descriptors.
struct Lease(File);
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl Workspace {
    /// # Errors
    /// The owner/root come from the host's persisted workspace, never tool input.
    /// A different actor, missing lock, root drift or credential drift refuses execution.
    pub fn open(journal: Arc<Journal>, owner: Uuid, root: &Path) -> Result<Self> {
        Self::open_mode(journal, owner, root, Initialization::Allowed)
    }

    /// # Errors
    /// Requires an existing original-owner binding and unchanged root and credential realm.
    /// Holds the normal exclusive leases without creating or rewriting ownership state.
    pub fn open_existing(journal: Arc<Journal>, owner: Uuid, root: &Path) -> Result<Self> {
        Self::open_mode(journal, owner, root, Initialization::Forbidden)
    }

    fn open_mode(journal: Arc<Journal>, owner: Uuid, root: &Path, initialization: Initialization) -> Result<Self> {
        if owner.is_nil() {
            return Err(Error::OwnershipRefused);
        }
        let root = root.canonicalize().map_err(|_| Error::OwnershipRefused)?;
        if !root.is_dir() {
            return Err(Error::OwnershipRefused);
        }
        let root_file = File::open(&root).map_err(|_| Error::OwnershipRefused)?;
        if root.canonicalize().map_err(|_| Error::OwnershipRefused)? != root {
            return Err(Error::OwnershipRefused);
        }
        let lease = Lease(if initialization == Initialization::Allowed {
            journal.store.claim(owner, &root, &root_file)?
        } else {
            journal.store.claim_existing(owner, &root, &root_file)?
        });
        // The owner binding protects recovery identity; the directory lease also
        // serializes different owners, accounts and state directories on one root.
        let root_lease = Lease(root_file.try_clone().map_err(|_| Error::JournalUnavailable)?);
        root_lease.0.try_lock().map_err(|_| Error::ExecutionBusy)?;
        let reconcile = journal.execution_pending(owner, &root)?;
        Ok(Self {
            journal,
            owner,
            root,
            _lease: lease,
            _root_lease: root_lease,
            root_file,
            reconcile,
        })
    }

    /// # Errors
    /// A restarted actor cannot execute new work until every retained owned operation is reconciled.
    pub fn start(&self, kind: Kind, lifetime: Duration) -> Result<Operation> {
        if self.reconcile {
            return Err(Error::ReconciliationRequired);
        }
        if self.root.canonicalize().map_err(|_| Error::OwnershipRefused)? != self.root {
            return Err(Error::OwnershipRefused);
        }
        let current = File::open(&self.root).map_err(|_| Error::OwnershipRefused)?;
        if Identity::capture(&current)? != Identity::capture(&self.root_file)? {
            return Err(Error::OwnershipRefused);
        }
        self.journal.start(self.owner, &self.root, kind, lifetime)
    }

    /// # Errors
    /// This opens admission only after confirmed recovery has removed all pending operations.
    /// The recovery callbacks must hold this workspace lease throughout their bounded cleanup.
    pub fn finish_reconciliation(&mut self) -> Result<()> {
        if self.journal.execution_pending(self.owner, &self.root)? {
            return Err(Error::ReconciliationRequired);
        }
        self.reconcile = false;
        Ok(())
    }

    /// Trusted host binding: the provider must match the lease's captured credential realm.
    /// # Errors
    /// A different configured account cannot control this workspace's journaled resources.
    pub fn provider(&self, account: &crate::account::Account) -> Result<Arc<horizon_app_provider::api::BrowserStack>> {
        if self.journal.realm != account.realm() {
            return Err(Error::OwnershipRefused);
        }
        Ok(account.provider())
    }

    /// # Errors
    /// Trusted directory capability for artifact capture; never reopen a tool-selected root.
    pub fn root_directory(&self) -> Result<File> {
        self.root_file.try_clone().map_err(|_| Error::OwnershipRefused)
    }

    /// Host-only lifecycle access; public interfaces receive operation snapshots, never this journal.
    #[must_use]
    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    #[must_use]
    pub fn owner(&self) -> Uuid {
        self.owner
    }
}

impl Journal {
    fn execution_pending(&self, owner: Uuid, root: &Path) -> Result<bool> {
        self.store.access(false, |ledger| {
            let mut pending = false;
            for record in ledger.records.values() {
                if record.owner == owner && record.root != root {
                    return Err(Error::OwnershipRefused);
                }
                if record.root == root && record.phase != Phase::Complete {
                    // A new owner cannot reconcile or bypass another owner's work.
                    if record.owner != owner || record.realm != self.realm {
                        return Err(Error::OwnershipRefused);
                    }
                    pending = true;
                }
            }
            Ok(pending)
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::tests::{canonical_temp, journal};
    use super::*;

    struct ChildCleanup(std::process::Child);
    impl Drop for ChildCleanup {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn one_actor_per_workspace_keeps_other_workspaces_independent() {
        let folder = canonical_temp();
        let journal = Arc::new(journal(&folder.path().join("state"), 'a'));
        let owner = Uuid::new_v4();
        let first = Workspace::open(Arc::clone(&journal), owner, folder.path()).unwrap();
        assert!(matches!(
            Workspace::open(Arc::clone(&journal), owner, folder.path()),
            Err(Error::ExecutionBusy)
        ));
        assert!(matches!(
            Workspace::open(Arc::clone(&journal), Uuid::new_v4(), folder.path()),
            Err(Error::ExecutionBusy)
        ));
        let other_root = canonical_temp();
        let other = Workspace::open(Arc::clone(&journal), Uuid::new_v4(), other_root.path()).unwrap();
        drop(other);
        drop(first);
        Workspace::open(journal, owner, folder.path()).unwrap();
    }

    #[test]
    fn different_owners_and_state_namespaces_cannot_claim_the_same_root_concurrently() {
        let folder = canonical_temp();
        let journal_a = Arc::new(journal(&folder.path().join("state-a"), 'a'));
        let journal_b = Arc::new(journal(&folder.path().join("state-b"), 'b'));
        let first = Workspace::open(journal_a, Uuid::new_v4(), folder.path()).unwrap();
        let alias = folder.path().join("root-alias");
        std::os::unix::fs::symlink(folder.path(), &alias).unwrap();
        assert!(matches!(
            Workspace::open(Arc::clone(&journal_b), Uuid::new_v4(), &alias),
            Err(Error::ExecutionBusy)
        ));
        // Explicit directory unlock must not depend on the last inherited clone.
        let inherited = first.root_directory().unwrap();
        drop(first);
        let next = Workspace::open(journal_b, Uuid::new_v4(), folder.path()).unwrap();
        drop(inherited);
        drop(next);
    }

    #[test]
    fn changing_owner_cannot_bypass_unfinished_work_on_the_same_root() {
        let folder = canonical_temp();
        let journal = Arc::new(journal(&folder.path().join("state"), 'a'));
        let owner = Uuid::new_v4();
        let first = Workspace::open(Arc::clone(&journal), owner, folder.path()).unwrap();
        let operation = first.start(Kind::Run, Duration::from_secs(30)).unwrap();
        drop(first);
        let other_owner = Uuid::new_v4();
        assert!(matches!(
            Workspace::open(Arc::clone(&journal), other_owner, folder.path()),
            Err(Error::OwnershipRefused)
        ));
        journal.confirm_released(owner, operation.id).unwrap();
        let other = Workspace::open(journal, other_owner, folder.path()).unwrap();
        other.start(Kind::Run, Duration::from_secs(30)).unwrap();
    }

    #[test]
    fn dropping_execution_lease_unlocks_an_inherited_file_description() {
        let folder = canonical_temp();
        let journal = Arc::new(journal(&folder.path().join("state"), 'a'));
        let owner = Uuid::new_v4();
        let root_file = File::open(folder.path()).unwrap();
        let first = Lease(journal.store.claim(owner, folder.path(), &root_file).unwrap());
        let inherited = first.0.try_clone().unwrap();
        assert!(matches!(
            Workspace::open(Arc::clone(&journal), owner, folder.path()),
            Err(Error::ExecutionBusy)
        ));
        drop(first);
        let restarted = Workspace::open(journal, owner, folder.path()).unwrap();
        drop(inherited);
        drop(restarted);
    }

    #[test]
    fn restart_requires_confirmed_reconciliation_and_refuses_credential_drift() {
        let folder = canonical_temp();
        let state = folder.path().join("state");
        let owner = Uuid::new_v4();
        let journal = Arc::new(journal(&state, 'a'));
        let first = Workspace::open(Arc::clone(&journal), owner, folder.path()).unwrap();
        let operation = first.start(Kind::Run, Duration::from_secs(30)).unwrap();
        drop(first);
        let mut restarted = Workspace::open(Arc::clone(&journal), owner, folder.path()).unwrap();
        assert_eq!(
            restarted.start(Kind::Run, Duration::from_secs(30)).err(),
            Some(Error::ReconciliationRequired)
        );
        assert_eq!(restarted.finish_reconciliation(), Err(Error::ReconciliationRequired));
        drop(restarted);
        let changed = Arc::new(super::super::tests::journal(&state, 'b'));
        assert!(matches!(
            Workspace::open(changed, owner, folder.path()),
            Err(Error::OwnershipRefused)
        ));
        journal.confirm_released(owner, operation.id).unwrap();
        let mut restarted = Workspace::open(journal, owner, folder.path()).unwrap();
        restarted.finish_reconciliation().unwrap();
        restarted.start(Kind::Run, Duration::from_secs(30)).unwrap();
    }

    #[test]
    fn root_drift_or_missing_execution_lock_cannot_initialize_a_new_actor() {
        let folder = canonical_temp();
        let state = folder.path().join("state");
        let owner = Uuid::new_v4();
        let journal = Arc::new(journal(&state, 'a'));
        let first = Workspace::open(Arc::clone(&journal), owner, folder.path()).unwrap();
        drop(first);
        let other = folder.path().join("other-root");
        std::fs::create_dir(&other).unwrap();
        assert!(matches!(
            Workspace::open(Arc::clone(&journal), owner, &other),
            Err(Error::OwnershipRefused)
        ));
        let lock = state.join(format!("execution-{owner}.lock"));
        std::fs::rename(&lock, state.join("saved-lock")).unwrap();
        assert!(matches!(
            Workspace::open(Arc::clone(&journal), owner, folder.path()),
            Err(Error::JournalInvalid)
        ));
        std::fs::rename(state.join("saved-lock"), lock).unwrap();
        Workspace::open(journal, owner, folder.path()).unwrap();
    }
    #[test]
    fn killed_actor_releases_kernel_lease_but_retains_its_unfinished_operation() {
        const FIXTURE: &str = "HORIZON_NATIVE_LEASE_FIXTURE";
        if let Ok(input) = std::env::var(FIXTURE) {
            let (root, owner): (PathBuf, Uuid) = serde_json::from_str(&input).unwrap();
            let journal = Arc::new(journal(&root.join("state"), 'a'));
            let workspace = Workspace::open(journal, owner, &root).unwrap();
            let operation = workspace.start(Kind::Run, Duration::from_secs(30)).unwrap();
            std::fs::write(root.join("ready"), operation.id.to_string()).unwrap();
            std::thread::sleep(Duration::from_secs(25));
            return;
        }
        let folder = canonical_temp();
        let owner = Uuid::new_v4();
        let mut child = ChildCleanup(std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "journal::execution::tests::killed_actor_releases_kernel_lease_but_retains_its_unfinished_operation",
            ])
            .env(FIXTURE, serde_json::to_string(&(folder.path(), owner)).unwrap())
            .spawn()
            .unwrap());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !folder.path().join("ready").exists() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        let journal = Arc::new(journal(&folder.path().join("state"), 'a'));
        assert!(matches!(
            Workspace::open(Arc::clone(&journal), owner, folder.path()),
            Err(Error::ExecutionBusy)
        ));
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        let restarted = Workspace::open(journal, owner, folder.path()).unwrap();
        assert_eq!(
            restarted.start(Kind::Run, Duration::from_secs(30)).err(),
            Some(Error::ReconciliationRequired)
        );
    }
    #[test]
    fn replacing_a_live_lock_with_identical_bytes_cannot_create_a_second_actor() {
        let folder = canonical_temp();
        let state = folder.path().join("state");
        let owner = Uuid::new_v4();
        let journal = Arc::new(journal(&state, 'a'));
        let first = Workspace::open(Arc::clone(&journal), owner, folder.path()).unwrap();
        let lock = state.join(format!("execution-{owner}.lock"));
        let replacement = state.join("replacement");
        std::fs::copy(&lock, &replacement).unwrap();
        std::fs::rename(&replacement, &lock).unwrap();
        assert!(matches!(
            Workspace::open(journal, owner, folder.path()),
            Err(Error::JournalInvalid)
        ));
        drop(first);
    }

    #[test]
    fn live_root_redirect_or_replacement_refuses_new_operation() {
        let folder = canonical_temp();
        let root = folder.path().join("project");
        std::fs::create_dir(&root).unwrap();
        let journal = Arc::new(journal(&folder.path().join("state"), 'a'));
        let owner = Uuid::new_v4();
        let workspace = Workspace::open(Arc::clone(&journal), owner, &root).unwrap();
        let saved = folder.path().join("saved");
        std::fs::rename(&root, &saved).unwrap();
        std::os::unix::fs::symlink(&saved, &root).unwrap();
        assert_eq!(
            workspace.start(Kind::Run, Duration::from_secs(30)).err(),
            Some(Error::OwnershipRefused)
        );
        std::fs::remove_file(&root).unwrap();
        std::fs::create_dir(&root).unwrap();
        assert_eq!(
            workspace.start(Kind::Run, Duration::from_secs(30)).err(),
            Some(Error::OwnershipRefused)
        );
        assert!(journal.pending(owner).unwrap().is_empty());
        drop(workspace);
        assert!(matches!(
            Workspace::open(journal, owner, &root),
            Err(Error::OwnershipRefused)
        ));
    }
}
