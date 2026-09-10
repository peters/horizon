use super::*;
use crate::repository_overlay::intake::storage_status::WorkerStorageStatus as Status;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

fn private() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

#[test]
fn qualification_is_separate_from_claims_and_preserves_existing_bytes() {
    let directory = private();
    let path = directory.path();
    let before = fs::metadata(path).unwrap();
    // This injected success covers control flow, not real filesystem qualification.
    assert_eq!(inspect_storage_with(path, &|_| Ok(())), Status::Qualified);
    assert_eq!(fs::read_dir(path).unwrap().count(), 0);
    fs::write(path.join(CLAIM), b"not a valid intake record").unwrap();
    let record = fs::metadata(path.join(CLAIM)).unwrap();
    assert_eq!(inspect_storage_with(path, &|_| Ok(())), Status::Qualified);
    assert_eq!(fs::read(path.join(CLAIM)).unwrap(), b"not a valid intake record");
    let after = fs::metadata(path.join(CLAIM)).unwrap();
    assert_eq!(
        (record.dev(), record.ino(), record.mtime(), record.mtime_nsec()),
        (after.dev(), after.ino(), after.mtime(), after.mtime_nsec())
    );
    assert_eq!(fs::metadata(path).unwrap().ino(), before.ino());
    assert_eq!(fs::read_dir(path).unwrap().count(), 1);
}

#[test]
fn missing_unsafe_and_linked_roots_refuse_before_qualification() {
    let directory = private();
    let unexpected = |_: &File| -> Result<(), IntakeError> { panic!("unsafe root reached qualifier") };
    let missing = directory.path().join("missing");
    assert_eq!(inspect_storage_with(&missing, &unexpected), Status::Unavailable);
    assert!(!missing.exists());
    let link = directory.path().join("link");
    symlink(directory.path(), &link).unwrap();
    assert_eq!(inspect_storage_with(&link, &unexpected), Status::Unavailable);
    let file = directory.path().join("file");
    fs::write(&file, b"retained").unwrap();
    assert_eq!(inspect_storage_with(&file, &unexpected), Status::Unavailable);
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(inspect_storage_with(directory.path(), &unexpected), Status::Unavailable);
    assert_eq!(fs::read(file).unwrap(), b"retained");
}

#[test]
fn unsupported_and_unavailable_checks_preserve_an_empty_root() {
    let directory = private();
    for (error, expected) in [
        (IntakeError::Unsupported, Status::Unsupported),
        (IntakeError::Storage, Status::Unavailable),
    ] {
        assert_eq!(inspect_storage_with(directory.path(), &|_| Err(error)), expected);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

#[test]
fn replacement_or_permission_change_during_qualification_cannot_pass() {
    for replace in [false, true] {
        let directory = private();
        let path = directory.path().join("worker");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let original = fs::metadata(&path).unwrap();
        assert_eq!(
            inspect_storage_with(&path, &|_| {
                if replace {
                    fs::rename(&path, directory.path().join("retained")).unwrap();
                    fs::create_dir(&path).unwrap();
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
                } else {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
                }
                Ok(())
            }),
            Status::Unavailable
        );
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
        if replace {
            assert_eq!(
                fs::metadata(directory.path().join("retained")).unwrap().ino(),
                original.ino()
            );
        }
    }
}
