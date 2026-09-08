use super::super::{RetainedSetup, SetupAdmission, tests::intent};
use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    sync::{Arc, Barrier},
    thread,
};

fn private() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    root
}

// Real confinement, private files, link and synchronization; these state tests
// bypass only storage qualification and make no power-loss/durability claim.
fn directory(path: &Path) -> Directory {
    Directory {
        reader: SelectedRepositoryReader::open(path).unwrap().root,
        handle: File::open(path).unwrap(),
        path: path.to_owned(),
    }
}

fn admit(directory: &Directory, intent: &SetupIntent) -> Result<Admission, Error> {
    directory.admit_with(
        intent,
        &mut |file, bytes| file.write_all(bytes),
        &mut File::sync_all,
        &mut link,
    )
}

fn claim(path: &Path, bytes: &[u8]) {
    fs::write(path.join(CLAIM), bytes).unwrap();
    fs::set_permissions(path.join(CLAIM), fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn observations_and_identical_retries_never_write_sync_or_replay() {
    let root = private();
    let directory = directory(root.path());
    let intent = intent();
    assert_eq!(directory.observe(&intent), Ok(SetupObservation::Absent));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    assert!(matches!(admit(&directory, &intent), Ok(Admission::Fresh)));
    let before = fs::read(root.path().join(CLAIM)).unwrap();
    let metadata = fs::metadata(root.path().join(CLAIM)).unwrap();
    assert_eq!(metadata.mode() & 0o7777, 0o600);
    for _ in 0..3 {
        assert_eq!(directory.observe(&intent), Ok(SetupObservation::ClaimedUnknown));
        assert!(matches!(
            directory.admit_with(
                &intent,
                &mut |_, _| panic!("write"),
                &mut |_| panic!("sync"),
                &mut |_, _| panic!("link")
            ),
            Ok(Admission::Existing)
        ));
    }
    assert_eq!(fs::read(root.path().join(CLAIM)).unwrap(), before);
    assert_eq!(fs::metadata(root.path().join(CLAIM)).unwrap().ino(), metadata.ino());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    assert!(!root.path().join(super::super::SCRATCH_NAME).exists());
}

#[test]
fn concurrent_identical_and_conflicting_admissions_have_at_most_one_winner() {
    for conflict in [false, true] {
        let root = private();
        let barrier = Arc::new(Barrier::new(8));
        let directory = Arc::new(directory(root.path()));
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let directory = Arc::clone(&directory);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let mut expected = intent();
                    if conflict {
                        expected.workspace_local_id.push_str(&index.to_string());
                    }
                    barrier.wait();
                    admit(&directory, &expected)
                })
            })
            .collect();
        let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(outcomes.iter().filter(|r| matches!(r, Ok(Admission::Fresh))).count(), 1);
        assert!(
            outcomes
                .iter()
                .all(|r| matches!(r, Ok(Admission::Fresh | Admission::Existing) | Err(Error::Conflict)))
        );
        if conflict {
            assert_eq!(outcomes.iter().filter(|r| matches!(r, Err(Error::Conflict))).count(), 7);
        }
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[test]
fn changed_fields_conflict_after_reopen_instead_of_selecting_another_slot() {
    let root = private();
    let expected = intent();
    assert!(matches!(
        admit(&directory(root.path()), &expected),
        Ok(Admission::Fresh)
    ));
    for field in 0..5 {
        let mut changed = expected.clone();
        match field {
            0 => changed.workspace_local_id.push('2'),
            1 => changed.objects_directory.push("different"),
            2 => changed.bundle_store.push("different"),
            3 => changed.bundle_manifest = crate::cloud_run::ArtifactDigest::sha256(b"different"),
            _ => changed.destination.push('2'),
        }
        let reopened = directory(root.path());
        assert_eq!(reopened.observe(&changed), Err(Error::Conflict));
        assert!(matches!(admit(&reopened, &changed), Err(Error::Conflict)));
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn write_and_sync_faults_never_grant_and_post_link_claims_block_replay() {
    for fail in 0..4 {
        let root = private();
        let directory = directory(root.path());
        let mut syncs = 0;
        let result = directory.admit_with(
            &intent(),
            &mut |file, bytes| {
                if fail == 0 {
                    file.write_all(&bytes[..5])?;
                    return Err(io::ErrorKind::Other.into());
                }
                file.write_all(bytes)
            },
            &mut |file| {
                syncs += 1;
                if syncs == fail {
                    return Err(io::ErrorKind::Other.into());
                }
                file.sync_all()
            },
            &mut link,
        );
        assert!(matches!(result, Err(Error::Storage)));
        let named = fail >= 2;
        assert_eq!(root.path().join(CLAIM).exists(), named);
        assert_eq!(
            directory.observe(&intent()).unwrap(),
            if named {
                SetupObservation::ClaimedUnknown
            } else {
                SetupObservation::Absent
            }
        );
        if named {
            assert!(matches!(admit(&directory, &intent()), Ok(Admission::Existing)));
        }
    }
}

#[test]
fn link_error_after_real_link_is_uncertain_and_cannot_grant_on_retry() {
    for actual_link in [false, true] {
        let root = private();
        let directory = directory(root.path());
        let result = directory.admit_with(
            &intent(),
            &mut |file, bytes| file.write_all(bytes),
            &mut File::sync_all,
            &mut |file, parent| {
                if actual_link {
                    link(file, parent)?;
                }
                Err(rustix::io::Errno::IO)
            },
        );
        assert!(matches!(result, Err(Error::Storage)));
        assert_eq!(root.path().join(CLAIM).exists(), actual_link);
        if actual_link {
            assert!(matches!(admit(&directory, &intent()), Ok(Admission::Existing)));
        }
    }
}

#[test]
fn synchronization_order_exposes_only_unknown_before_fresh_acknowledgement() {
    let root = private();
    let directory = directory(root.path());
    let mut syncs = 0;
    let result = directory.admit_with(
        &intent(),
        &mut |file, bytes| file.write_all(bytes),
        &mut |file| {
            syncs += 1;
            assert_eq!(
                directory.observe(&intent()).unwrap(),
                if syncs == 1 {
                    SetupObservation::Absent
                } else {
                    SetupObservation::ClaimedUnknown
                }
            );
            assert_eq!(file.metadata().unwrap().is_dir(), syncs == 3);
            file.sync_all()
        },
        &mut link,
    );
    assert!(matches!(result, Ok(Admission::Fresh)));
    assert_eq!(syncs, 3);
}

#[test]
fn malformed_and_unsafe_claims_fail_closed_without_repair() {
    for kind in 0..7 {
        let root = private();
        let expected = intent();
        match kind {
            0 => claim(root.path(), b""),
            1 => claim(root.path(), b"{\"version\":1"),
            2 => claim(root.path(), &vec![b'x'; codec::MAX_RECORD_BYTES + 1]),
            3 => {
                claim(root.path(), &codec::encode(&expected).unwrap());
                fs::set_permissions(root.path().join(CLAIM), fs::Permissions::from_mode(0o644)).unwrap();
            }
            4 => {
                claim(root.path(), &codec::encode(&expected).unwrap());
                fs::hard_link(root.path().join(CLAIM), root.path().join("alias")).unwrap();
            }
            5 => symlink("missing", root.path().join(CLAIM)).unwrap(),
            _ => fs::create_dir(root.path().join(CLAIM)).unwrap(),
        }
        let directory = directory(root.path());
        let error = if kind <= 2 { Error::InvalidRecord } else { Error::Read };
        assert_eq!(directory.observe(&expected), Err(error));
        assert!(matches!(admit(&directory, &expected), Err(actual) if actual == error));
        assert!(fs::symlink_metadata(root.path().join(CLAIM)).is_ok());
    }
}

#[test]
fn reader_errors_preserve_unsupported_confinement_and_invalid_records() {
    assert_eq!(root_error(RepositoryReadError::Unsupported), Error::Unsupported);
    assert_eq!(read_error(RepositoryReadError::Unsupported), Error::Unsupported);
    assert_eq!(read_error(RepositoryReadError::TooLarge), Error::InvalidRecord);
    for error in [
        RepositoryReadError::InvalidRoot,
        RepositoryReadError::Missing,
        RepositoryReadError::UnsafePath,
        RepositoryReadError::UnsupportedNode,
        RepositoryReadError::Changed,
        RepositoryReadError::ReadFailed,
    ] {
        assert_eq!(root_error(error), Error::UnsafeRoot);
        assert_eq!(read_error(error), Error::Read);
    }
}

#[test]
fn missing_insecure_removed_and_replaced_roots_never_become_absent_claims() {
    let outer = private();
    let missing = outer.path().join("missing");
    assert!(matches!(RetainedSetup::open(&missing), Err(Error::UnsafeRoot)));
    assert!(!missing.exists());
    for remove in [false, true] {
        let root = outer.path().join(if remove { "removed" } else { "replaced" });
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let directory = directory(&root);
        if remove {
            fs::remove_dir(&root).unwrap();
        } else {
            fs::rename(&root, outer.path().join("old")).unwrap();
            fs::create_dir(&root).unwrap();
        }
        assert_eq!(directory.observe(&intent()), Err(Error::UnsafeRoot));
        assert!(matches!(admit(&directory, &intent()), Err(Error::UnsafeRoot)));
    }
    fs::set_permissions(outer.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(RetainedSetup::open(outer.path()), Err(Error::UnsafeRoot)));
}

#[test]
fn qualified_public_admission_grant_drop_and_process_reopen_never_replay() {
    let root = private();
    let store = match RetainedSetup::open(root.path()) {
        Err(Error::Unsupported) => {
            eprintln!("SKIP real claim: requires qualified journaled ext4");
            return;
        }
        other => other.unwrap(),
    };
    let SetupAdmission::Fresh(grant) = store.admit(intent()).unwrap() else {
        panic!("fresh grant");
    };
    assert_eq!(grant.intent(), &intent());
    assert_eq!(format!("{grant:?}"), "SetupGrant { .. }");
    drop(grant);
    drop(store);
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "repository_overlay::retained_setup::linux::tests::reopen_child",
            "--nocapture",
        ])
        .env("HORIZON_SETUP_CLAIM_FIXTURE", root.path())
        .output()
        .unwrap();
    assert!(child.status.success(), "{}", String::from_utf8_lossy(&child.stderr));
    assert!(String::from_utf8_lossy(&child.stdout).contains("REOPEN_NO_REPLAY"));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn reopen_child() {
    let Some(root) = std::env::var_os("HORIZON_SETUP_CLAIM_FIXTURE") else {
        return;
    };
    let store = RetainedSetup::open(Path::new(&root)).unwrap();
    assert_eq!(store.observe(&intent()), Ok(SetupObservation::ClaimedUnknown));
    assert!(matches!(store.admit(intent()), Ok(SetupAdmission::Existing)));
    println!("REOPEN_NO_REPLAY");
}

#[test]
fn volatile_storage_is_rejected_without_claim_or_root_creation() {
    let root = tempfile::tempdir_in("/dev/shm").unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(RetainedSetup::open(root.path()), Err(Error::Unsupported)));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}
