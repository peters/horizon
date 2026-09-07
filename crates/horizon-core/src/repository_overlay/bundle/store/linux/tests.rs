use super::super::tests::private_fixture;
use super::*;
use std::{fs, sync::Barrier, thread};

#[test]
fn partial_anonymous_write_never_has_a_name_and_is_discarded_on_close() {
    let fixture = private_fixture();
    let directory = Directory::open(fixture.path()).expect("directory");
    let mut partial = directory.anonymous().expect("anonymous file");
    partial.write_all(b"partial input").expect("partial write");
    assert_eq!(partial.metadata().expect("inode").nlink(), 0);
    assert_eq!(fs::read_dir(fixture.path()).expect("directory").count(), 0);
    drop(partial);
    assert_eq!(fs::read_dir(fixture.path()).expect("no orphan name").count(), 0);
}

#[test]
fn each_sync_failure_is_reported_and_explicit_retry_retains_any_published_inode() {
    for failed_sync in 1..=3 {
        let fixture = private_fixture();
        let directory = Directory::open(fixture.path()).expect("directory");
        let digest = ArtifactDigest::sha256(b"synthetic publication identity");
        let bytes = b"complete synthetic bytes";
        let mut calls = 0;
        let result = directory.put_with_sync(&digest, bytes, |file| {
            calls += 1;
            if calls == failed_sync {
                Err(io::Error::other("synthetic fsync failure"))
            } else {
                file.sync_all()
            }
        });
        assert_eq!(result, Err(Error::WriteFailed));
        let before = directory.read(&digest).expect("read outcome");
        assert_eq!(before.is_some(), failed_sync > 1);
        let inode = before
            .as_ref()
            .map(|record| record.file.metadata().expect("published inode").ino());
        directory.put(&digest, bytes).expect("explicit retry");
        let retained = directory.read(&digest).expect("retry read").expect("complete record");
        assert_eq!(retained.bytes, bytes);
        if let Some(inode) = inode {
            assert_eq!(retained.file.metadata().expect("same inode").ino(), inode);
        }
        assert_eq!(fs::read_dir(fixture.path()).expect("one record").count(), 1);
    }
}

#[test]
fn existing_record_retry_requires_both_synchronization_steps() {
    let fixture = private_fixture();
    let directory = Directory::open(fixture.path()).expect("directory");
    let digest = ArtifactDigest::sha256(b"identity");
    directory.put(&digest, b"bytes").expect("initial publication");
    for failed_sync in 1..=2 {
        let mut calls = 0;
        assert_eq!(
            directory.put_with_sync(&digest, b"bytes", |file| {
                calls += 1;
                if calls == failed_sync {
                    Err(io::Error::other("sync failure"))
                } else {
                    file.sync_all()
                }
            }),
            Err(Error::WriteFailed)
        );
        assert_eq!(directory.read(&digest).expect("read").expect("record").bytes, b"bytes");
    }
}

#[test]
fn no_replace_publication_preserves_the_winner_of_a_conflicting_race() {
    let fixture = private_fixture();
    let digest = ArtifactDigest::sha256(b"shared name");
    let barrier = Barrier::new(2);
    let results = thread::scope(|scope| {
        let root = fixture.path();
        let identity = &digest;
        let start = &barrier;
        let handles: Vec<_> = [b"first".as_slice(), b"second"]
            .into_iter()
            .map(|bytes| {
                scope.spawn(move || {
                    let directory = Directory::open(root).expect("independent directory");
                    let mut synchronized_data = false;
                    directory.put_with_sync(identity, bytes, |file| {
                        file.sync_all()?;
                        if !synchronized_data {
                            synchronized_data = true;
                            start.wait();
                        }
                        Ok(())
                    })
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
    let directory = Directory::open(fixture.path()).expect("fresh directory");
    let record = directory.read(&digest).expect("read").expect("winner");
    assert!(record.bytes == b"first" || record.bytes == b"second");
    assert_eq!(fs::read_dir(fixture.path()).expect("one winner").count(), 1);
}

#[test]
fn unsupported_filesystems_have_no_weaker_publication_fallback() {
    for error in [
        rustix::io::Errno::NOSYS,
        rustix::io::Errno::INVAL,
        rustix::io::Errno::OPNOTSUPP,
    ] {
        assert_eq!(storage_error(error), Error::Unsupported);
    }
    assert_eq!(storage_error(rustix::io::Errno::ROFS), Error::WriteFailed);
}
