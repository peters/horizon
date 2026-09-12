use super::*;
use crate::repository_overlay::bundle::store::tests::private_fixture;
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, symlink},
    path::Path,
    sync::Barrier,
    thread,
};

fn claim(root: &File, name: &str) -> Result<(), rustix::io::Errno> {
    mkdirat(root, name, PRIVATE)
}

fn publish(slot: &File) -> Result<(), rustix::io::Errno> {
    renameat(slot, PENDING, slot, RECORD)
}

fn private_dir(path: &Path) {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .expect("private directory");
}

fn private_file(path: &Path, bytes: &[u8]) {
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .expect("private file")
        .write_all(bytes)
        .expect("fixture bytes");
}

#[test]
fn incomplete_claims_are_never_absence_reclaimed_or_overwritten() {
    for pending in [None, Some(b"partial".as_slice())] {
        let fixture = private_fixture();
        let parent = Directory::open_named(fixture.path()).expect("root");
        let digest = ArtifactDigest::sha256(b"fixture");
        assert!(read(&parent, &digest).expect("true absence").is_none());
        let slot = fixture.path().join(digest.as_str());
        private_dir(&slot);
        if let Some(bytes) = pending {
            private_file(&slot.join(PENDING), bytes);
        }
        assert!(matches!(read(&parent, &digest), Err(Error::Conflict)));
        assert_eq!(put(&parent, &digest, b"complete"), Err(Error::Conflict));
        assert!(!slot.join(RECORD).exists());
        assert_eq!(fs::read(slot.join(PENDING)).ok().as_deref(), pending);
    }
}

#[test]
fn each_sync_failure_preserves_partial_or_completed_claim_without_replay() {
    for failed_sync in 1..=5 {
        let fixture = private_fixture();
        let parent = Directory::open_named(fixture.path()).expect("root");
        let digest = ArtifactDigest::sha256(b"fixture");
        let mut calls = 0;
        assert_eq!(
            put_with(
                &parent,
                &digest,
                b"complete",
                |file| {
                    calls += 1;
                    if calls == failed_sync {
                        Err(io::Error::other("injected sync failure"))
                    } else {
                        file.sync_all()
                    }
                },
                claim,
                publish
            ),
            Err(Error::WriteFailed)
        );
        if failed_sync <= 2 {
            assert_eq!(put(&parent, &digest, b"complete"), Err(Error::Conflict));
        } else {
            let before = read(&parent, &digest).expect("read").expect("complete");
            let inode = before.file.metadata().expect("inode").ino();
            put(&parent, &digest, b"complete").expect("explicit resync");
            let after = read(&parent, &digest).expect("read").expect("complete");
            assert_eq!(after.file.metadata().expect("inode").ino(), inode);
            assert_eq!(after.bytes, b"complete");
        }
        assert_eq!(fs::read_dir(fixture.path()).expect("claims").count(), 1);
    }
}

#[test]
fn ambiguous_mkdir_never_confers_write_ownership() {
    for created in [false, true] {
        let fixture = private_fixture();
        let parent = Directory::open_named(fixture.path()).expect("root");
        let digest = ArtifactDigest::sha256(b"fixture");
        assert_eq!(
            put_with(
                &parent,
                &digest,
                b"must not write",
                File::sync_all,
                |root, name| {
                    if created {
                        claim(root, name)?;
                    }
                    Err(rustix::io::Errno::IO)
                },
                |_| panic!("no publish authority")
            ),
            Err(Error::WriteFailed)
        );
        let slot = fixture.path().join(digest.as_str());
        assert_eq!(slot.exists(), created);
        if created {
            assert_eq!(fs::read_dir(&slot).expect("preserved claim").count(), 0);
            assert_eq!(put(&parent, &digest, b"retry"), Err(Error::Conflict));
        }
    }
}

#[test]
fn ambiguous_rename_preserves_evidence_and_only_completed_retry_can_resync() {
    for renamed in [false, true] {
        let fixture = private_fixture();
        let parent = Directory::open_named(fixture.path()).expect("root");
        let digest = ArtifactDigest::sha256(b"fixture");
        assert_eq!(
            put_with(&parent, &digest, b"complete", File::sync_all, claim, |slot| {
                if renamed {
                    publish(slot)?;
                }
                Err(rustix::io::Errno::IO)
            }),
            Err(Error::WriteFailed)
        );
        let slot = fixture.path().join(digest.as_str());
        assert_eq!(slot.join(PENDING).exists(), !renamed);
        assert_eq!(slot.join(RECORD).exists(), renamed);
        if renamed {
            put(&parent, &digest, b"complete").expect("read and resync only");
            assert_eq!(put(&parent, &digest, b"different"), Err(Error::Conflict));
            assert_eq!(fs::read(slot.join(RECORD)).expect("unchanged"), b"complete");
        } else {
            assert_eq!(put(&parent, &digest, b"complete"), Err(Error::Conflict));
            assert_eq!(fs::read(slot.join(PENDING)).expect("partial retained"), b"complete");
        }
    }
}

#[test]
fn completed_idempotency_requires_every_sync_and_exact_stable_identity() {
    let fixture = private_fixture();
    let parent = Directory::open_named(fixture.path()).expect("root");
    let digest = ArtifactDigest::sha256(b"fixture");
    put(&parent, &digest, b"complete").expect("initial write");
    for failed_sync in 1..=3 {
        let mut calls = 0;
        assert_eq!(
            put_with(
                &parent,
                &digest,
                b"complete",
                |file| {
                    calls += 1;
                    if calls == failed_sync {
                        Err(io::Error::other("injected sync failure"))
                    } else {
                        file.sync_all()
                    }
                },
                claim,
                |_| panic!("existing record never renamed")
            ),
            Err(Error::WriteFailed)
        );
        assert_eq!(
            read(&parent, &digest).expect("read").expect("retained").bytes,
            b"complete"
        );
    }
    let slot = fixture.path().join(digest.as_str());
    let mut swapped = false;
    assert_eq!(
        put_with(
            &parent,
            &digest,
            b"complete",
            |file| {
                file.sync_all()?;
                if !swapped {
                    swapped = true;
                    fs::rename(slot.join(RECORD), slot.join("retained"))?;
                    private_file(&slot.join(RECORD), b"complete");
                }
                Ok(())
            },
            claim,
            publish
        ),
        Err(Error::Conflict)
    );
}

#[test]
fn independent_conflicting_writers_have_one_owner_and_keep_the_winner() {
    let fixture = private_fixture();
    let digest = ArtifactDigest::sha256(b"same name");
    let barrier = Barrier::new(2);
    let results = thread::scope(|scope| {
        let handles: Vec<_> = [b"first".as_slice(), b"second"]
            .into_iter()
            .map(|bytes| {
                let root = fixture.path();
                let digest = &digest;
                let barrier = &barrier;
                scope.spawn(move || {
                    let parent = Directory::open_named(root).expect("root");
                    put_with(
                        &parent,
                        digest,
                        bytes,
                        File::sync_all,
                        |root, name| {
                            barrier.wait();
                            claim(root, name)
                        },
                        publish,
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("writer"))
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results.iter().filter(|result| **result == Err(Error::Conflict)).count(),
        1
    );
    let parent = Directory::open_named(fixture.path()).expect("root");
    let record = read(&parent, &digest).expect("read").expect("winner");
    assert!(record.bytes == b"first" || record.bytes == b"second");
    let slot = fixture.path().join(digest.as_str());
    assert_eq!(fs::read_dir(slot).expect("one record").count(), 1);
}

#[test]
fn symlink_hardlink_mode_and_slot_replacement_are_refused() {
    for kind in ["slot-link", "record-link", "hardlink", "mode", "pending"] {
        let fixture = private_fixture();
        let outside = private_fixture();
        let parent = Directory::open_named(fixture.path()).expect("root");
        let digest = ArtifactDigest::sha256(b"fixture");
        let slot = fixture.path().join(digest.as_str());
        private_file(&outside.path().join(RECORD), b"untouched");
        if kind == "slot-link" {
            symlink(outside.path(), &slot).expect("symlink");
        } else {
            private_dir(&slot);
            match kind {
                "record-link" => symlink(outside.path().join(RECORD), slot.join(RECORD)).expect("symlink"),
                "hardlink" => fs::hard_link(outside.path().join(RECORD), slot.join(RECORD)).expect("hardlink"),
                _ => {
                    private_file(&slot.join(RECORD), b"untouched");
                    if kind == "mode" {
                        fs::set_permissions(slot.join(RECORD), fs::Permissions::from_mode(0o644)).expect("mode");
                    } else {
                        private_file(&slot.join(PENDING), b"preserve partial");
                    }
                }
            }
        }
        assert!(read(&parent, &digest).is_err());
        assert!(put(&parent, &digest, b"replacement").is_err());
        assert_eq!(
            fs::read(outside.path().join(RECORD)).expect("outside retained"),
            b"untouched"
        );
    }
    let fixture = private_fixture();
    let parent = Directory::open_named(fixture.path()).expect("root");
    let digest = ArtifactDigest::sha256(b"fixture");
    let slot = fixture.path().join(digest.as_str());
    let mut calls = 0;
    assert_eq!(
        put_with(
            &parent,
            &digest,
            b"retained",
            |file| {
                file.sync_all()?;
                calls += 1;
                if calls == 2 {
                    fs::rename(&slot, fixture.path().join("retained"))?;
                    private_dir(&slot);
                }
                Ok(())
            },
            claim,
            |_| panic!("replaced slot must not publish")
        ),
        Err(Error::Conflict)
    );
    assert_eq!(fs::read_dir(slot).expect("replacement empty").count(), 0);
    assert_eq!(
        fs::read(fixture.path().join("retained/pending")).expect("original retained"),
        b"retained"
    );
}
