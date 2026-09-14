use super::{Error, storage};
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn durable_claim_is_never_overwritten_after_an_uncertain_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let claim = root.path().join("start.claimed");
    storage::write_new(&claim, b"first").unwrap();
    assert!(matches!(storage::write_new(&claim, b"replayed"), Err(Error::Storage)));
    assert_eq!(fs::read(&claim).unwrap(), b"first");
    assert_eq!(fs::metadata(&claim).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn competing_controller_cannot_acquire_task_lock() {
    let root = tempfile::tempdir().unwrap();
    let first = storage::lock(root.path()).unwrap();
    assert!(matches!(storage::lock(root.path()), Err(Error::Storage)));
    drop(first);
    assert!(storage::lock(root.path()).is_ok());
}

#[test]
fn task_creation_refuses_existing_directory_without_changing_it() {
    let root = tempfile::tempdir().unwrap();
    let sentinel = root.path().join("keep");
    fs::write(&sentinel, b"existing").unwrap();
    assert!(storage::create_root(root.path()).is_err());
    assert_eq!(fs::read(sentinel).unwrap(), b"existing");
}

#[test]
fn task_storage_requires_private_directory_permissions() {
    let parent = tempfile::tempdir().unwrap();
    let root = storage::create_root(&parent.path().join("task")).unwrap();
    assert!(storage::private_directory(&root).is_ok());
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(storage::private_directory(&root).is_err());
}
