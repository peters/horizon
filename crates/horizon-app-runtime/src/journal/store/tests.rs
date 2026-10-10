use super::super::{
    Journal, Kind,
    execution::Workspace,
    tests::{canonical_temp, journal as initialized_journal},
};
use super::*;
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[derive(Debug, Eq, PartialEq)]
struct Snapshot {
    inode: u64,
    modified: SystemTime,
    bytes: Vec<u8>,
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Snapshot> {
    fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Snapshot>) {
        let metadata = std::fs::metadata(path).unwrap();
        entries.insert(
            path.strip_prefix(root).unwrap().to_owned(),
            Snapshot {
                inode: metadata.ino(),
                modified: metadata.modified().unwrap(),
                bytes: if metadata.is_file() {
                    std::fs::read(path).unwrap()
                } else {
                    Vec::new()
                },
            },
        );
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), entries);
            }
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

fn existing_journal(state: &Path, realm: char) -> Arc<Journal> {
    Arc::new(Journal {
        store: Store::open_existing(state).unwrap(),
        realm: realm.to_string().repeat(64),
    })
}

#[test]
fn existing_status_preserves_files_and_holds_the_original_leases() {
    let folder = canonical_temp();
    let state = folder.path().join("state");
    let owner = Uuid::new_v4();
    let journal = Arc::new(initialized_journal(&state, 'a'));
    let original = Workspace::open(Arc::clone(&journal), owner, folder.path()).unwrap();
    let operation = original.start(Kind::Run, Duration::from_secs(30)).unwrap();
    drop(original);
    drop(journal);
    let before = snapshot(folder.path());
    let journal = existing_journal(&state, 'a');
    let inspected = Workspace::open_existing(Arc::clone(&journal), owner, folder.path()).unwrap();
    assert_eq!(inspected.journal().pending(owner).unwrap()[0].id, operation.id);
    assert!(matches!(
        Workspace::open_existing(Arc::clone(&journal), owner, folder.path()),
        Err(Error::ExecutionBusy)
    ));
    assert_eq!(snapshot(folder.path()), before);
    drop(inspected);
    assert_eq!(snapshot(folder.path()), before);
    Workspace::open_existing(journal, owner, folder.path()).unwrap();
}

#[test]
fn missing_state_registry_namespace_and_owner_are_never_initialized() {
    let folder = canonical_temp();
    let before = snapshot(folder.path());
    for state in [folder.path().join("missing-parent/state"), folder.path().join("state")] {
        assert!(Store::open_existing(&state).is_err());
        assert_eq!(snapshot(folder.path()), before);
    }
    let state = folder.path().join("state");
    let journal = Arc::new(initialized_journal(&state, 'a'));
    let before = snapshot(folder.path());
    assert!(Store::open_existing(&folder.path().join("missing-namespace")).is_err());
    assert_eq!(
        Workspace::open_existing(journal, Uuid::new_v4(), folder.path()).err(),
        Some(Error::OwnershipRefused)
    );
    assert_eq!(snapshot(folder.path()), before);
}

#[test]
fn foreign_owner_root_and_credential_realm_refuse_without_persistence() {
    let folder = canonical_temp();
    let state = folder.path().join("state");
    let owner = Uuid::new_v4();
    let journal = Arc::new(initialized_journal(&state, 'a'));
    let original = Workspace::open(Arc::clone(&journal), owner, folder.path()).unwrap();
    original.start(Kind::Run, Duration::from_secs(30)).unwrap();
    drop(original);
    let other = folder.path().join("other-root");
    std::fs::create_dir(&other).unwrap();
    let before = snapshot(folder.path());
    assert_eq!(
        Workspace::open_existing(Arc::clone(&journal), Uuid::new_v4(), folder.path()).err(),
        Some(Error::OwnershipRefused)
    );
    assert_eq!(
        Workspace::open_existing(Arc::clone(&journal), owner, &other).err(),
        Some(Error::OwnershipRefused)
    );
    assert_eq!(
        Workspace::open_existing(existing_journal(&state, 'b'), owner, folder.path()).err(),
        Some(Error::OwnershipRefused)
    );
    assert_eq!(snapshot(folder.path()), before);
}

#[test]
fn missing_existing_registry_or_owner_locks_are_not_recreated() {
    let folder = canonical_temp();
    let state = folder.path().join("state");
    let owner = Uuid::new_v4();
    let journal = Arc::new(initialized_journal(&state, 'a'));
    drop(Workspace::open(Arc::clone(&journal), owner, folder.path()).unwrap());
    let registry = folder.path().join(".native-journal-registry");
    let registry_lock = registry.join("journal.lock");
    std::fs::rename(&registry_lock, registry.join("saved-lock")).unwrap();
    let before = snapshot(folder.path());
    assert!(Store::open_existing(&state).is_err());
    assert!(Workspace::open_existing(Arc::clone(&journal), owner, folder.path()).is_err());
    assert_eq!(snapshot(folder.path()), before);
    std::fs::rename(registry.join("saved-lock"), registry_lock).unwrap();
    let owner_lock = state.join(format!("execution-{owner}.lock"));
    std::fs::rename(&owner_lock, state.join("saved-owner-lock")).unwrap();
    let before = snapshot(folder.path());
    assert_eq!(
        Workspace::open_existing(journal, owner, folder.path()).err(),
        Some(Error::JournalInvalid)
    );
    assert_eq!(snapshot(folder.path()), before);
}
