use super::super::super::observe::tests::{SnapshotNode, snapshot};
use super::super::publish_named_git_pack;
use super::*;
use crate::repository_overlay::seed::{
    export::{PackExportLimits, PreparedGitPack, prepare_git_base_pack},
    prepare_git_seed,
    receive::receive_git_base_pack,
    tests::{Fixture as RepositoryFixture, private_fixture},
};
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs::{self, DirBuilder, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink},
    sync::{
        Barrier,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

struct Fixture {
    _source: tempfile::TempDir,
    pack: PreparedGitPack,
}

impl Fixture {
    fn new() -> Self {
        let repository = RepositoryFixture::new();
        let source = private_fixture();
        let resolved = repository.resolve(vec![], vec![], vec![]);
        let seed = prepare_git_seed(source.path(), &resolved, &mut repository.source(), || false).unwrap();
        let pack = prepare_git_base_pack(source.path(), &seed, PackExportLimits::default(), || false).unwrap();
        Self { _source: source, pack }
    }

    fn receive(&self, root: &Path) -> ReceivedGitPack {
        receive_git_base_pack(
            root,
            (&self.pack).into(),
            &mut File::open(self.pack.path()).unwrap(),
            PackReceiveLimits::default(),
            || false,
        )
        .unwrap()
    }

    fn destination(&self, root: &Path) -> PathBuf {
        root.join(self.pack.sha256().as_str()).join(PACK)
    }

    fn observe(&self, root: &Path) -> ReceivedGitPack {
        observe_git_base_pack(
            &self.destination(root),
            (&self.pack).into(),
            PackReceiveLimits::default(),
            || false,
        )
        .unwrap()
    }
}

fn private_dir(path: &Path) {
    DirBuilder::new().mode(0o700).create(path).unwrap();
}

fn private_file(path: &Path, bytes: &[u8]) {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

fn sync(_: SyncPoint, file: &File) -> Result<(), Error> {
    file.sync_all().map_err(|_| Error::Storage)
}

fn store(root: &Path, pack: ReceivedGitPack) -> Result<Outcome, Failure> {
    publish_named_git_pack(root, pack, PackReceiveLimits::default(), || false)
}

fn assert_existing(outcome: &Outcome, path: &Path, incoming: Option<&Path>) {
    let Outcome::Existing { pack, unused_incoming } = outcome else {
        panic!("expected existing publication")
    };
    assert_eq!(pack.path(), path);
    assert_eq!(unused_incoming.as_ref().map(ReceivedGitPack::path), incoming);
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

#[test]
fn public_publication_and_explicit_reacknowledgement_retain_exact_data_and_unused_input() {
    let fixture = Fixture::new();
    let root = private_fixture();
    let input = fixture.receive(root.path());
    let old = input.path().to_path_buf();
    let original = tree(&old);
    let inode = fs::metadata(&old).unwrap().ino();
    let Outcome::Published(published) = store(root.path(), input).unwrap() else {
        panic!("fresh publication")
    };
    assert_eq!(published.path(), fixture.destination(root.path()));
    assert_eq!(fs::metadata(published.path()).unwrap().ino(), inode);
    assert_eq!(tree(published.path()), original);
    assert!(!old.exists());
    let observed = fixture.observe(root.path());
    assert_eq!(observed.objects(), 3);
    let before = snapshot(root.path());
    assert_existing(&store(root.path(), observed).unwrap(), published.path(), None);
    assert_eq!(snapshot(root.path()), before);
    let incoming = fixture.receive(root.path());
    let incoming_path = incoming.path().to_path_buf();
    let before = snapshot(root.path());
    let existing = store(root.path(), incoming).unwrap();
    assert_existing(&existing, published.path(), Some(&incoming_path));
    assert_eq!(snapshot(root.path()), before);
    drop((published, existing));
    assert!(incoming_path.exists());
    assert!(fixture.destination(root.path()).exists());
}

#[test]
fn partial_foreign_corrupt_and_unsafe_existing_slots_are_not_reclaimed() {
    let fixture = Fixture::new();
    for kind in 0..6 {
        let root = private_fixture();
        let foreign = private_fixture();
        let input = fixture.receive(root.path());
        let destination = fixture.destination(root.path());
        let slot = destination.parent().unwrap();
        if kind == 5 {
            symlink(foreign.path(), slot).unwrap();
        } else {
            private_dir(slot);
        }
        if kind == 1 {
            private_file(&destination, b"partial");
        }
        if (2..=4).contains(&kind) {
            fs::rename(fixture.receive(root.path()).path(), &destination).unwrap();
            if kind == 2 {
                private_file(&slot.join("foreign"), b"keep");
            }
            if kind == 3 {
                let file = fs::read_dir(destination.join("decoded/objects/pack"))
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|path| path.extension().unwrap() == "pack")
                    .unwrap();
                let mut bytes = fs::read(&file).unwrap();
                bytes[20] ^= 1;
                fs::write(file, bytes).unwrap();
            }
            if kind == 4 {
                fs::set_permissions(slot, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let before = snapshot(root.path());
        assert!(matches!(
            publish(
                root.path(),
                input,
                PackReceiveLimits::default(),
                &|| false,
                &mut sync,
                &mut |_, _| panic!("existing slot grants no claim"),
                &mut |_, _| panic!("no rename")
            ),
            Err(Failure::Retained { .. })
        ));
        assert_eq!(snapshot(root.path()), before);
    }
}

#[test]
fn sync_errors_and_cancellation_preserve_each_fresh_and_existing_phase() {
    let fixture = Fixture::new();
    for (existing, cancel) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut expected = vec![SyncPoint::File; 8];
        expected.extend([SyncPoint::Directory; 11]);
        expected.extend([SyncPoint::Directory, SyncPoint::Parent]);
        if !existing {
            expected.extend([SyncPoint::PublishedRoot, SyncPoint::Directory, SyncPoint::Parent]);
        }
        for failed in 0..=expected.len() {
            let root = private_fixture();
            if existing {
                store(root.path(), fixture.receive(root.path())).unwrap();
            }
            let input = fixture.receive(root.path());
            let old = input.path().to_path_buf();
            let destination = fixture.destination(root.path());
            let original = tree(&old);
            let before = existing.then(|| snapshot(root.path()));
            let stop = Cell::new(false);
            let mut points = Vec::new();
            let mut renames = 0;
            let result = publish(
                root.path(),
                input,
                PackReceiveLimits::default(),
                &|| stop.get(),
                &mut |point, file| {
                    let index = points.len();
                    points.push(point);
                    if index == failed && !cancel {
                        return Err(Error::Storage);
                    }
                    sync(point, file)?;
                    if index == failed {
                        stop.set(true);
                    }
                    Ok(())
                },
                &mut |root, name| {
                    assert!(!existing, "no existing claim");
                    claim(root, name)
                },
                &mut |binding, slot| {
                    assert!(!existing, "no existing rename");
                    renames += 1;
                    rename(binding, slot)
                },
            );
            assert_eq!(points, expected[..(failed + 1).min(expected.len())]);
            let moved = !existing && failed >= 21;
            let reason = if cancel {
                Error::Verification(super::super::SeedError::Cancelled)
            } else {
                Error::Storage
            };
            if let Err(error) = &result {
                assert!(!format!("{error:?} {error}").contains(root.path().to_str().unwrap()));
            }
            match result {
                Ok(Outcome::Published(_)) if failed == expected.len() && !existing => {}
                Ok(outcome) if failed == expected.len() && existing => {
                    assert_existing(&outcome, &destination, Some(&old));
                }
                Err(Failure::Retained {
                    reason: actual,
                    pack,
                    destination: target,
                }) if failed < expected.len() && !moved => {
                    assert_eq!(actual, reason);
                    assert_eq!(pack.path(), old);
                    assert_eq!(target.as_deref(), Some(destination.as_path()));
                }
                Err(Failure::PublishedUnsynchronized { reason: actual, pack }) if failed < expected.len() && moved => {
                    assert_eq!(actual, reason);
                    assert_eq!(pack.path(), destination);
                }
                _ => panic!("incorrect synchronization phase"),
            }
            assert_eq!(renames, usize::from(moved));
            assert_eq!(old.exists(), !moved);
            assert_eq!(destination.exists(), existing || moved);
            assert_eq!(destination.parent().unwrap().exists(), existing || failed >= 19);
            assert_eq!(tree(if moved { &destination } else { &old }), original);
            if let Some(before) = before {
                assert_eq!(snapshot(root.path()), before);
            }
        }
    }
}

#[test]
fn ambiguous_operations_preserve_distinct_claim_and_rename_authority() {
    #[derive(Clone, Copy, PartialEq)]
    enum Operation {
        Claim,
        Rename,
    }
    let fixture = Fixture::new();
    for (operation, changed) in [
        (Operation::Claim, false),
        (Operation::Claim, true),
        (Operation::Rename, false),
        (Operation::Rename, true),
    ] {
        let root = private_fixture();
        let input = fixture.receive(root.path());
        let old = input.path().to_path_buf();
        let original = tree(&old);
        let mut claims = 0;
        let mut renames = 0;
        let failure = publish(
            root.path(),
            input,
            PackReceiveLimits::default(),
            &|| false,
            &mut sync,
            &mut |root, name| {
                claims += 1;
                if operation == Operation::Rename {
                    return claim(root, name);
                }
                if changed {
                    claim(root, name)?;
                }
                Err(rustix::io::Errno::IO)
            },
            &mut |binding, slot| {
                assert!(operation == Operation::Rename, "no ownership from uncertain claim");
                renames += 1;
                if changed {
                    rename(binding, slot)?;
                }
                Err(rustix::io::Errno::IO)
            },
        )
        .unwrap_err();
        let (pack, destination) = match failure {
            Failure::ClaimUnconfirmed { pack, destination } if operation == Operation::Claim => (pack, destination),
            Failure::RenameUnconfirmed { pack, destination } if operation == Operation::Rename => (pack, destination),
            _ => panic!("incorrect uncertain phase"),
        };
        let moved = operation == Operation::Rename && changed;
        let claimed = operation == Operation::Rename || changed;
        assert_eq!(claims, 1);
        assert_eq!(renames, usize::from(operation == Operation::Rename));
        assert_eq!(pack.path(), old);
        assert_eq!(destination, fixture.destination(root.path()));
        assert_eq!(old.exists(), !moved);
        assert_eq!(destination.exists(), moved);
        assert_eq!(destination.parent().unwrap().exists(), claimed);
        assert_eq!(tree(if moved { &destination } else { &old }), original);
        if claimed {
            let before = snapshot(root.path());
            if moved {
                let existing = store(root.path(), fixture.observe(root.path())).unwrap();
                assert_existing(&existing, &destination, None);
            } else {
                assert!(matches!(store(root.path(), *pack), Err(Failure::Retained { .. })));
            }
            assert_eq!(snapshot(root.path()), before);
        }
    }
}

#[test]
fn unsafe_roots_inputs_limits_and_expectations_refuse_before_claiming() {
    let fixture = Fixture::new();
    for case in 0..10 {
        let outer = private_fixture();
        let root = outer.path().join("store");
        private_dir(&root);
        let mut input = fixture.receive(&root);
        let mut selected = root.clone();
        let mut limits = PackReceiveLimits::default();
        match case {
            0 => fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap(),
            1 => {
                selected = outer.path().join("alias");
                symlink(&root, &selected).unwrap();
            }
            2 => {
                fs::rename(input.path(), root.join("retained")).unwrap();
                symlink("retained", input.path()).unwrap();
            }
            3 => fs::hard_link(input.path().join("decoded/config"), outer.path().join("alias")).unwrap(),
            4 => limits.encoded_bytes = 31,
            5 => input.encoded_bytes += 1,
            6 => input.base_commit = git2::Oid::ZERO_SHA1,
            7 => selected = outer.path().join("missing"),
            8 => selected = PathBuf::from(format!("/{}", "x".repeat(MAX_PACK_PATH_BYTES))),
            9 => {}
            _ => unreachable!(),
        }
        let before = snapshot(outer.path());
        let failure = publish(
            &selected,
            input,
            limits,
            &|| case == 9,
            &mut |_, _| panic!("no sync"),
            &mut |_, _| panic!("no claim"),
            &mut |_, _| panic!("no rename"),
        )
        .unwrap_err();
        assert!(matches!(failure, Failure::Retained { .. }));
        if case == 8 {
            assert!(matches!(failure, Failure::Retained { destination: None, .. }));
        }
        if case == 9 {
            assert!(matches!(
                failure,
                Failure::Retained {
                    reason: Error::Verification(super::super::SeedError::Cancelled),
                    ..
                }
            ));
        }
        assert_eq!(snapshot(outer.path()), before);
    }
}

#[test]
fn detected_source_root_slot_and_destination_replacement_cannot_be_acknowledged() {
    let fixture = Fixture::new();
    for case in 0..4 {
        let outer = private_fixture();
        let root = outer.path().join("store");
        private_dir(&root);
        let input = fixture.receive(&root);
        let spare = fixture.receive(&root);
        let old = input.path().to_path_buf();
        let destination = fixture.destination(&root);
        let retained = outer.path().join("retained");
        let mut calls = 0;
        let mut swapped = false;
        let failure = publish(
            &root,
            input,
            PackReceiveLimits::default(),
            &|| false,
            &mut |point, file| {
                sync(point, file)?;
                if calls == [0, 0, 19, 21][case] {
                    let selected = match case {
                        0 => &old,
                        1 => &root,
                        2 => destination.parent().unwrap(),
                        _ => &destination,
                    };
                    fs::rename(selected, &retained).unwrap();
                    if matches!(case, 0 | 3) {
                        fs::rename(spare.path(), selected).unwrap();
                    } else {
                        private_dir(selected);
                    }
                    swapped = true;
                }
                calls += 1;
                Ok(())
            },
            &mut claim,
            &mut rename,
        )
        .unwrap_err();
        assert!(swapped);
        assert!(retained.exists());
        if case == 3 {
            assert!(matches!(failure, Failure::PublishedUnsynchronized { .. }));
        } else {
            assert!(matches!(failure, Failure::Retained { .. }));
        }
    }
}

#[test]
fn same_bytes_replaced_during_existing_resynchronization_are_not_acknowledged() {
    let fixture = Fixture::new();
    let root = private_fixture();
    store(root.path(), fixture.receive(root.path())).unwrap();
    let observed = fixture.observe(root.path());
    let file = observed.path().join("decoded/config");
    let bytes = fs::read(&file).unwrap();
    let inode = fs::metadata(&file).unwrap().ino();
    let mut swapped = false;
    let result = publish(
        root.path(),
        observed,
        PackReceiveLimits::default(),
        &|| false,
        &mut |point, handle| {
            sync(point, handle)?;
            if !swapped {
                fs::rename(&file, root.path().join("retained-config")).unwrap();
                private_file(&file, &bytes);
                swapped = true;
            }
            Ok(())
        },
        &mut |_, _| panic!("no claim"),
        &mut |_, _| panic!("no rename"),
    );
    assert!(swapped);
    assert!(matches!(result, Err(Failure::Retained { .. })));
    assert_eq!(fs::read(&file).unwrap(), bytes);
    assert_ne!(fs::metadata(&file).unwrap().ino(), inode);
}

#[test]
fn a_racing_partial_claim_does_not_grant_rename_authority() {
    let fixture = Fixture::new();
    let root = private_fixture();
    let input = fixture.receive(root.path());
    let old = input.path().to_path_buf();
    let result = publish(
        root.path(),
        input,
        PackReceiveLimits::default(),
        &|| false,
        &mut sync,
        &mut |root, name| {
            claim(root, name)?;
            Err(rustix::io::Errno::EXIST)
        },
        &mut |_, _| panic!("a racing slot is not ours"),
    );
    assert!(matches!(
        result,
        Err(Failure::Retained {
            reason: Error::DestinationExists,
            ..
        })
    ));
    assert!(old.exists());
    assert!(!fixture.destination(root.path()).exists());
}

#[test]
fn competing_publishers_rename_once_and_retain_the_unused_incoming_pack() {
    let fixture = Fixture::new();
    let root = private_fixture();
    let inputs = [fixture.receive(root.path()), fixture.receive(root.path())];
    let paths = inputs.each_ref().map(|pack| pack.path().to_path_buf());
    let barrier = Barrier::new(2);
    let renames = AtomicUsize::new(0);
    let outcomes = thread::scope(|scope| {
        let handles = inputs.map(|input| {
            let barrier = &barrier;
            let renames = &renames;
            let root = root.path();
            scope.spawn(move || {
                publish(
                    root,
                    input,
                    PackReceiveLimits::default(),
                    &|| false,
                    &mut sync,
                    &mut |root, name| {
                        barrier.wait();
                        claim(root, name)
                    },
                    &mut |binding, slot| {
                        renames.fetch_add(1, Ordering::SeqCst);
                        rename(binding, slot)
                    },
                )
            })
        });
        handles.map(|handle| handle.join().unwrap())
    });
    assert_eq!(renames.load(Ordering::SeqCst), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Ok(Outcome::Published(_))))
            .count(),
        1
    );
    assert!(outcomes.iter().all(|outcome| matches!(
        outcome,
        Ok(Outcome::Published(_)
            | Outcome::Existing {
                unused_incoming: Some(_),
                ..
            })
            | Err(Failure::Retained { .. })
    )));
    assert_eq!(paths.iter().filter(|path| path.exists()).count(), 1);
    fixture.observe(root.path());
}
