use super::super::observe::tests::{SnapshotNode, snapshot};
use super::*;
use crate::repository_overlay::seed::{
    export::{PackExportLimits, prepare_git_base_pack},
    prepare_git_seed,
    receive::{observe_git_base_pack, receive_git_base_pack},
    tests::{Fixture, private_fixture},
};
use crate::repository_overlay::{
    bundle::codec,
    checkout::prepare_private_checkout,
    namespace::resolve_namespaces_from_source,
    seed::packed::{PackedGitObjectSource, PackedSourceLimits},
};
use PackPublicationError as Error;
use PackPublicationFailure as Failure;
use linux::SyncPoint;
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs::{self, File},
    os::unix::{
        fs::{MetadataExt, PermissionsExt, symlink},
        net::UnixListener,
    },
};

fn received(parent: &Path) -> ReceivedGitPack {
    let fixture = Fixture::new();
    received_fixture(parent, &fixture)
}

fn received_fixture(parent: &Path, fixture: &Fixture) -> ReceivedGitPack {
    let source = private_fixture();
    let resolved = fixture.resolve(vec![], vec![], vec![]);
    let seed = prepare_git_seed(source.path(), &resolved, &mut fixture.source(), || false).unwrap();
    let pack = prepare_git_base_pack(source.path(), &seed, PackExportLimits::default(), || false).unwrap();
    receive_git_base_pack(
        parent,
        (&pack).into(),
        &mut File::open(pack.path()).unwrap(),
        PackReceiveLimits::default(),
        || false,
    )
    .unwrap()
}

fn tree(root: &Path) -> BTreeMap<PathBuf, SnapshotNode> {
    snapshot(root)
        .into_iter()
        .filter_map(|(path, node)| {
            let relative = path.strip_prefix(root).unwrap();
            (!relative.as_os_str().is_empty()).then(|| (relative.to_path_buf(), node))
        })
        .collect()
}

fn sync(_: SyncPoint, file: &File) -> Result<(), Error> {
    file.sync_all().map_err(|_| Error::Storage)
}

#[test]
fn qualified_pack_publication_reopens_without_recreation() {
    let parent = private_fixture();
    if linux::supported_storage(&File::open(parent.path()).unwrap()) == Err(Error::Unsupported) {
        eprintln!("SKIP qualified pack publication: journaled ext4 capability unavailable");
        return;
    }
    let fixture = Fixture::new();
    let pack = received_fixture(parent.path(), &fixture);
    let old = pack.path().to_path_buf();
    let before = tree(&old);
    let inode = fs::metadata(&old).unwrap().ino();
    let published = publish_sibling_git_pack(pack, "ready", PackReceiveLimits::default(), || false).unwrap();
    assert_eq!(published.path(), parent.path().join("ready"));
    assert!(!old.exists());
    assert_eq!(fs::metadata(published.path()).unwrap().ino(), inode);
    assert_eq!(tree(published.path()), before);
    let observed = observe_git_base_pack(
        published.path(),
        published.pack().into(),
        PackReceiveLimits::default(),
        || false,
    )
    .unwrap();
    assert_eq!(observed.sha256(), published.pack().sha256());
    let scratch = private_fixture();
    let mut source = PackedGitObjectSource::new(
        scratch.path(),
        observed.objects_directory(),
        PackedSourceLimits::default(),
        || false,
    )
    .unwrap();
    let encoded = codec::encode(fixture.resolve(vec![], vec![], vec![]).bundle()).unwrap();
    let resolved = resolve_namespaces_from_source(&mut source, codec::decode(&encoded).unwrap(), || false).unwrap();
    let checkout = prepare_private_checkout(scratch.path(), &resolved, &mut source, || false).unwrap();
    assert_eq!(fs::read(checkout.path().join("kept")).unwrap(), b"base\0literal\xff");
    assert_eq!(
        git2::Repository::open(checkout.path())
            .unwrap()
            .head()
            .unwrap()
            .target(),
        Some(fixture.commit)
    );
    assert_eq!(tree(published.path()), before);
    drop(observed);
    drop(published);
    assert!(fs::metadata(parent.path().join("ready")).unwrap().is_dir());
}

#[test]
fn every_sync_failure_retains_the_correct_name_and_unmodified_data() {
    let expected = [
        vec![SyncPoint::File; 8],
        vec![SyncPoint::Directory; 11],
        vec![SyncPoint::PublishedRoot, SyncPoint::Parent],
    ]
    .concat();
    for failure_at in 0..=expected.len() {
        let parent = private_fixture();
        let pack = received(parent.path());
        let old = pack.path().to_path_buf();
        let target = parent.path().join("ready");
        let before = snapshot(parent.path());
        let contents = tree(&old);
        let mut points = Vec::new();
        let result = linux::publish(
            pack,
            "ready",
            PackReceiveLimits::default(),
            &|| false,
            &mut |point, file| {
                assert_eq!(
                    target.exists(),
                    matches!(point, SyncPoint::PublishedRoot | SyncPoint::Parent)
                );
                let index = points.len();
                points.push(point);
                if index == failure_at {
                    Err(Error::Storage)
                } else {
                    sync(point, file)
                }
            },
            &mut linux::rename,
            &|_| Ok(()),
        );
        assert_eq!(points, expected[..(failure_at + 1).min(expected.len())]);
        if failure_at == expected.len() {
            drop(result.unwrap());
        } else {
            let failure = result.unwrap_err();
            assert!(!format!("{failure:?} {failure}").contains(parent.path().to_str().unwrap()));
            match failure {
                Failure::Unpublished { reason, pack } if failure_at < 19 => {
                    assert_eq!(reason, Error::Storage);
                    assert_eq!(pack.path(), old);
                    drop(pack);
                    assert_eq!(snapshot(parent.path()), before);
                }
                Failure::PublishedUnsynchronized { reason, pack } if failure_at >= 19 => {
                    assert_eq!(reason, Error::Storage);
                    assert_eq!(pack.path(), target);
                    drop(pack);
                }
                _ => panic!("incorrect publication state"),
            }
        }
        assert_eq!(old.exists(), failure_at < 19);
        assert_eq!(target.exists(), failure_at >= 19);
        assert_eq!(tree(if failure_at < 19 { &old } else { &target }), contents);
    }
}

#[test]
fn cancellation_before_and_after_rename_never_rolls_back_or_replays() {
    for after in [false, true] {
        let parent = private_fixture();
        let pack = received(parent.path());
        let old = pack.path().to_path_buf();
        let target = parent.path().join("ready");
        let root = fs::metadata(&old).unwrap().ino();
        let stop = Cell::new(false);
        let result = linux::publish(
            pack,
            "ready",
            PackReceiveLimits::default(),
            &|| {
                if after { target.exists() } else { stop.get() }
            },
            &mut |point, file| {
                sync(point, file)?;
                if point == SyncPoint::Directory && file.metadata().unwrap().ino() == root {
                    stop.set(true);
                }
                Ok(())
            },
            &mut linux::rename,
            &|_| Ok(()),
        );
        let reason = match result.unwrap_err() {
            Failure::Unpublished { reason, pack } if !after => {
                assert_eq!(pack.path(), old);
                reason
            }
            Failure::PublishedUnsynchronized { reason, pack } if after => {
                assert_eq!(pack.path(), target);
                reason
            }
            _ => panic!("incorrect cancellation state"),
        };
        assert_eq!(reason, Error::Verification(SeedError::Cancelled));
        assert_eq!(old.exists(), !after);
        assert_eq!(target.exists(), after);
    }
}

fn unpublished(failure: Failure, expected: Error) -> ReceivedGitPack {
    assert!(!format!("{failure:?} {failure}").contains("/tmp/"));
    let Failure::Unpublished { reason, pack } = failure else {
        panic!("incorrect unpublished state")
    };
    assert_eq!(reason, expected);
    pack
}

#[test]
fn invalid_and_existing_siblings_never_replace_or_change_data() {
    let parent = private_fixture();
    let mut pack = received(parent.path());
    let own_name = pack.path().file_name().unwrap().to_str().unwrap().to_owned();
    fs::write(parent.path().join("file"), b"private-marker").unwrap();
    fs::create_dir(parent.path().join("directory")).unwrap();
    fs::create_dir(parent.path().join("nonempty")).unwrap();
    fs::write(parent.path().join("nonempty/sentinel"), b"retained").unwrap();
    symlink("missing", parent.path().join("symlink")).unwrap();
    let _socket = UnixListener::bind(parent.path().join("socket")).unwrap();
    let before = snapshot(parent.path());
    for name in [
        "",
        ".",
        "..",
        "../escape",
        "a/b",
        "a\\b",
        ".git",
        "nul\0private-marker",
        "line\n",
        "bad.",
        &own_name,
    ] {
        pack = unpublished(
            linux::publish(
                pack,
                name,
                PackReceiveLimits::default(),
                &|| false,
                &mut |_, _| panic!("invalid name reached synchronization"),
                &mut linux::rename,
                &|_| Ok(()),
            )
            .unwrap_err(),
            Error::InvalidName,
        );
        assert_eq!(snapshot(parent.path()), before);
    }
    for name in ["file", "directory", "nonempty", "symlink", "socket"] {
        pack = unpublished(
            linux::publish(
                pack,
                name,
                PackReceiveLimits::default(),
                &|| false,
                &mut sync,
                &mut linux::rename,
                &|_| Ok(()),
            )
            .unwrap_err(),
            Error::DestinationExists,
        );
        assert_eq!(snapshot(parent.path()), before);
    }
    drop(pack);
    assert_eq!(snapshot(parent.path()), before);
}

#[test]
fn qualification_limits_and_unsafe_inputs_fail_before_synchronization() {
    for case in [
        "unsupported",
        "qualification-error",
        "limit",
        "cancel",
        "corrupt",
        "parent-mode",
        "root-mode",
    ] {
        let parent = private_fixture();
        let pack = received(parent.path());
        match case {
            "corrupt" => fs::write(pack.path().join("decoded/config"), b"private-marker").unwrap(),
            "parent-mode" => fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o755)).unwrap(),
            "root-mode" => fs::set_permissions(pack.path(), fs::Permissions::from_mode(0o755)).unwrap(),
            _ => {}
        }
        let before = snapshot(parent.path());
        let limits = if case == "limit" {
            PackReceiveLimits {
                encoded_bytes: 0,
                ..PackReceiveLimits::default()
            }
        } else {
            PackReceiveLimits::default()
        };
        let failure = linux::publish(
            pack,
            "ready",
            limits,
            &|| case == "cancel",
            &mut |_, _| panic!("invalid input reached synchronization"),
            &mut linux::rename,
            &|_| match case {
                "unsupported" => Err(Error::Unsupported),
                "qualification-error" => Err(Error::Storage),
                _ => Ok(()),
            },
        )
        .unwrap_err();
        assert!(!format!("{failure:?} {failure}").contains("private-marker"));
        assert!(matches!(failure, Failure::Unpublished { .. }));
        drop(failure);
        assert_eq!(snapshot(parent.path()), before);
    }
}

#[test]
fn uncertain_rename_retains_both_candidate_names_without_post_sync_or_replay() {
    for moved in [false, true] {
        let parent = private_fixture();
        let pack = received(parent.path());
        let old = pack.path().to_path_buf();
        let target = parent.path().join("ready");
        let before = tree(&old);
        let failure = linux::publish(
            pack,
            "ready",
            PackReceiveLimits::default(),
            &|| false,
            &mut |point, file| {
                assert!(matches!(point, SyncPoint::File | SyncPoint::Directory));
                sync(point, file)
            },
            &mut |binding, sibling| {
                if moved {
                    linux::rename(binding, sibling)?;
                }
                Err(rustix::io::Errno::IO)
            },
            &|_| Ok(()),
        )
        .unwrap_err();
        assert!(!format!("{failure:?} {failure}").contains(parent.path().to_str().unwrap()));
        let Failure::RenameUnconfirmed { pack, destination } = failure else {
            panic!("incorrect uncertain state")
        };
        assert_eq!(pack.path(), old);
        assert_eq!(destination, target);
        assert_eq!(old.exists(), !moved);
        assert_eq!(target.exists(), moved);
        let retained = if moved { &target } else { &old };
        let observed =
            observe_git_base_pack(retained, pack.as_ref().into(), PackReceiveLimits::default(), || false).unwrap();
        drop(observed);
        drop(pack);
        assert_eq!(tree(retained), before);
    }
}

#[test]
fn same_content_inode_substitution_cannot_receive_a_sync_acknowledgement() {
    for when in [SyncPoint::Directory, SyncPoint::PublishedRoot, SyncPoint::Parent] {
        let parent = private_fixture();
        let pack = received(parent.path());
        let old = pack.path().to_path_buf();
        let target = parent.path().join("ready");
        let mut changed = None;
        let failure = linux::publish(
            pack,
            "ready",
            PackReceiveLimits::default(),
            &|| false,
            &mut |point, file| {
                sync(point, file)?;
                if point == when && changed.is_none() {
                    let root = if target.exists() { &target } else { &old };
                    let configuration = root.join("decoded/config");
                    let bytes = fs::read(&configuration).unwrap();
                    let replacement = parent.path().join("replacement");
                    fs::write(&replacement, &bytes).unwrap();
                    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600)).unwrap();
                    fs::rename(replacement, &configuration).unwrap();
                    assert_eq!(fs::read(configuration).unwrap(), bytes);
                    changed = Some(snapshot(parent.path()));
                }
                Ok(())
            },
            &mut linux::rename,
            &|_| Ok(()),
        )
        .unwrap_err();
        let receipt = match failure {
            Failure::Unpublished { reason, pack } if when == SyncPoint::Directory => {
                assert_eq!(reason, Error::Verification(SeedError::Object));
                assert_eq!(pack.path(), old);
                pack
            }
            Failure::PublishedUnsynchronized { reason, pack } if when != SyncPoint::Directory => {
                assert_eq!(reason, Error::Verification(SeedError::Object));
                assert_eq!(pack.path(), target);
                pack.0
            }
            _ => panic!("incorrect changed-input state"),
        };
        assert_eq!(snapshot(parent.path()), changed.unwrap());
        // Current content can still verify; that is not a past synchronization receipt.
        let observed = observe_git_base_pack(receipt.path(), (&receipt).into(), PackReceiveLimits::default(), || {
            false
        })
        .unwrap();
        drop(observed);
        drop(receipt);
    }
}

#[test]
fn replaced_source_and_parent_bindings_fail_without_cleanup() {
    for (replace_parent, after) in [(false, false), (true, false), (false, true), (true, true)] {
        let outer = private_fixture();
        let parent = outer.path().join("parent");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        let pack = received(&parent);
        let old = pack.path().to_path_buf();
        let active = if after { parent.join("ready") } else { old.clone() };
        let change_at = if after { SyncPoint::Parent } else { SyncPoint::Directory };
        let moved = outer.path().join("retained-original");
        let mut changed = None;
        let failure = linux::publish(
            pack,
            "ready",
            PackReceiveLimits::default(),
            &|| false,
            &mut |point, file| {
                sync(point, file)?;
                if point == change_at && changed.is_none() {
                    let replaced = if replace_parent { &parent } else { &active };
                    fs::rename(replaced, &moved).unwrap();
                    fs::create_dir(replaced).unwrap();
                    fs::set_permissions(replaced, fs::Permissions::from_mode(0o700)).unwrap();
                    fs::write(replaced.join("sentinel"), b"unchanged").unwrap();
                    changed = Some(snapshot(outer.path()));
                }
                Ok(())
            },
            &mut linux::rename,
            &|_| Ok(()),
        )
        .unwrap_err();
        assert!(matches!(
            (&failure, after),
            (Failure::Unpublished { .. }, false) | (Failure::PublishedUnsynchronized { .. }, true)
        ));
        drop(failure);
        assert_eq!(snapshot(outer.path()), changed.unwrap());
        let retained = if replace_parent {
            moved.join(active.file_name().unwrap())
        } else {
            moved
        };
        assert!(retained.join("decoded/config").is_file());
        if !after {
            assert!(!parent.join("ready").exists());
        }
    }
}

#[test]
fn concurrent_same_destination_publishers_have_one_winner_and_retain_the_loser() {
    for _ in 0..4 {
        let parent = private_fixture();
        let first = received(parent.path());
        let second = received(parent.path());
        let old = [first.path().to_path_buf(), second.path().to_path_buf()];
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let publish = |pack| {
                barrier.wait();
                linux::publish(
                    pack,
                    "ready",
                    PackReceiveLimits::default(),
                    &|| false,
                    &mut sync,
                    &mut linux::rename,
                    &|_| Ok(()),
                )
            };
            let left = scope.spawn(move || publish(first));
            let right = scope.spawn(move || publish(second));
            [left.join().unwrap(), right.join().unwrap()]
        });
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        for result in results {
            match result {
                Ok(pack) => assert_eq!(pack.path(), parent.path().join("ready")),
                Err(failure) => {
                    drop(unpublished(failure, Error::DestinationExists));
                }
            }
        }
        assert_eq!(old.iter().filter(|path| path.exists()).count(), 1);
        assert!(parent.path().join("ready/decoded/config").is_file());
        assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 2);
    }
}
