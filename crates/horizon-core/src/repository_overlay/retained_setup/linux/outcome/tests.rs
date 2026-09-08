use super::super::super::{SetupCompletionState, outcome::tests::snapshot, tests::intent};
use super::super::tests::{directory, private};
use super::super::{Admission, CLAIM, Mode};
use super::*;
use std::{
    fs,
    io::Write,
    os::unix::fs::{PermissionsExt, symlink},
};

fn fixture() -> (tempfile::TempDir, Directory, SetupIntent, SetupCompletion) {
    let root = private();
    let directory = directory(root.path());
    let intent = intent();
    assert!(matches!(
        directory.admit_with(
            &intent,
            &mut |f, b| f.write_all(b),
            &mut File::sync_all,
            &mut super::super::link
        ),
        Ok(Admission::Fresh)
    ));
    let completion = snapshot(root.path(), &intent, SetupCompletionState::Rejected);
    (root, directory, intent, completion)
}

// State-only faults use the existing unqualified test directory, never a production
// qualification bypass. Real confined file operations do not prove power-loss survival.
#[test]
fn write_sync_and_link_faults_retain_named_results_and_never_replace_them() {
    for fail in 0..6 {
        let (root, directory, intent, completion) = fixture();
        let mut syncs = 0;
        let result = record(
            &directory,
            &intent,
            &completion,
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
            &mut |file, parent| {
                if fail == 4 {
                    return Err(rustix::io::Errno::IO);
                }
                link(file, parent)?;
                if fail == 5 {
                    return Err(rustix::io::Errno::IO);
                }
                Ok(())
            },
        );
        assert!(result.is_err());
        let named = matches!(fail, 2 | 3 | 5);
        assert_eq!(root.path().join(RESULT).exists(), named);
        assert_eq!(read(&directory, &intent).unwrap().is_some(), named);
        assert_eq!(directory.observe(&intent), Ok(SetupObservation::ClaimedUnknown));
        if named {
            assert_eq!(
                record(
                    &directory,
                    &intent,
                    &completion,
                    &mut |_, _| panic!("write"),
                    &mut |_| panic!("sync"),
                    &mut |_, _| panic!("link")
                ),
                Err(SetupRecordError::Existing)
            );
        }
    }
}

#[test]
fn correct_order_records_historical_result_without_observer_synchronization() {
    let (root, directory, intent, completion) = fixture();
    let mut syncs = 0;
    record(
        &directory,
        &intent,
        &completion,
        &mut |file, bytes| file.write_all(bytes),
        &mut |file| {
            syncs += 1;
            assert_eq!(file.metadata().unwrap().is_dir(), syncs == 3);
            assert_eq!(read(&directory, &intent).unwrap().is_some(), syncs > 1);
            file.sync_all()
        },
        &mut link,
    )
    .unwrap();
    assert_eq!(syncs, 3);
    assert_eq!(read(&directory, &intent).unwrap(), Some(completion));
    drop(directory);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
}

#[test]
fn existing_special_corrupt_and_conflicting_results_are_not_adopted() {
    for kind in 0..7 {
        let (root, directory, intent, completion) = fixture();
        let path = root.path().join(RESULT);
        match kind {
            0 => fs::write(&path, b"invalid").unwrap(),
            1 => fs::create_dir(&path).unwrap(),
            2 => symlink("missing", &path).unwrap(),
            3 => rustix::fs::mknodat(
                &directory.handle,
                RESULT,
                rustix::fs::FileType::Fifo,
                Mode::RUSR | Mode::WUSR,
                0,
            )
            .unwrap(),
            _ => {
                let mut expected = intent.clone();
                if kind == 6 {
                    expected.workspace_local_id.push('2');
                }
                fs::write(&path, codec::encode(root.path(), &expected, &completion).unwrap()).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(if kind == 4 { 0o644 } else { 0o600 })).unwrap();
                if kind == 5 {
                    fs::hard_link(&path, root.path().join("alias")).unwrap();
                }
            }
        }
        let before = fs::symlink_metadata(&path).unwrap().ino();
        assert!(read(&directory, &intent).is_err());
        assert!(
            record(
                &directory,
                &intent,
                &completion,
                &mut |_, _| panic!("write"),
                &mut |_| panic!("sync"),
                &mut |_, _| panic!("link")
            )
            .is_err()
        );
        assert_eq!(fs::symlink_metadata(&path).unwrap().ino(), before);
    }
}

#[test]
fn claim_root_and_readback_changes_never_acknowledge_or_clean() {
    for kind in 0..4 {
        let (root, directory, intent, completion) = fixture();
        let mut syncs = 0;
        let result = record(
            &directory,
            &intent,
            &completion,
            &mut |file, bytes| file.write_all(bytes),
            &mut |file| {
                syncs += 1;
                if syncs == 3 {
                    match kind {
                        0 => fs::write(root.path().join(CLAIM), b"invalid").unwrap(),
                        1 => fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap(),
                        2 => fs::write(root.path().join(RESULT), b"invalid").unwrap(),
                        _ => {
                            fs::rename(root.path().join(RESULT), root.path().join("retained-original")).unwrap();
                            fs::write(
                                root.path().join(RESULT),
                                codec::encode(root.path(), &intent, &completion).unwrap(),
                            )
                            .unwrap();
                            fs::set_permissions(root.path().join(RESULT), fs::Permissions::from_mode(0o600)).unwrap();
                        }
                    }
                }
                file.sync_all()
            },
            &mut link,
        );
        assert!(result.is_err());
        assert!(root.path().join(RESULT).exists());
        assert!(root.path().join(CLAIM).exists());
    }
}
