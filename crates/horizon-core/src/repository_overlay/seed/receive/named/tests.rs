use super::*;
use crate::cloud_run::ArtifactDigest;
use crate::repository_overlay::seed::{
    export::{PackExportLimits, PreparedGitPack, prepare_git_base_pack},
    prepare_git_seed,
    tests::{Fixture, private_fixture, snapshot},
};
use rustix::io::Errno;
use std::{
    cell::Cell,
    fs::{self, OpenOptions},
    io::{self, Cursor, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
};

const HASH: &str = "1111111111111111111111111111111111111111";

fn pack(parent: &Path) -> PreparedGitPack {
    let fixture = Fixture::new();
    let resolved = fixture.resolve(vec![], vec![], vec![]);
    let seed = prepare_git_seed(parent, &resolved, &mut fixture.source(), || false).unwrap();
    prepare_git_base_pack(parent, &seed, PackExportLimits::default(), || false).unwrap()
}

fn claim(parent: &File, name: &str) -> rustix::io::Result<()> {
    mkdirat(parent, name, Mode::from_raw_mode(0o700))
}

fn relocate(directory: &File, source: &str, target: &str) -> rustix::io::Result<()> {
    renameat(directory, source, directory, target)
}

struct Unread;
impl Read for Unread {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("unexpected input consumption")
    }
}

fn expected(digest: &ArtifactDigest) -> ExpectedGitPack<'_> {
    ExpectedGitPack {
        base_commit: git2::Oid::from_str(HASH).unwrap(),
        sha256: digest,
        encoded_bytes: 32,
    }
}

#[test]
fn public_named_receive_observe_and_default_receive_keep_identical_pack_semantics() {
    let source = private_fixture();
    let pack = pack(source.path());
    let before = snapshot(source.path());
    let destination = private_fixture();
    let received = receive_named_git_base_pack(
        destination.path(),
        "attempt",
        (&pack).into(),
        &mut File::open(pack.path()).unwrap(),
        PackReceiveLimits::default(),
        || false,
    )
    .unwrap();
    let observed =
        observe_git_base_pack(received.path(), (&pack).into(), PackReceiveLimits::default(), || false).unwrap();
    let default = super::super::receive_git_base_pack(
        destination.path(),
        (&pack).into(),
        &mut File::open(pack.path()).unwrap(),
        PackReceiveLimits::default(),
        || false,
    )
    .unwrap();
    assert_eq!(received.path(), destination.path().join("attempt"));
    for result in [&received, &observed, &default] {
        assert_eq!(result.base_commit(), pack.base_commit());
        assert_eq!(result.sha256(), pack.sha256());
        assert_eq!(result.encoded_bytes(), pack.encoded_bytes());
        assert_eq!(result.objects(), received.objects());
        assert!(!format!("{result:?}").contains(destination.path().to_str().unwrap()));
    }
    let saved = snapshot(destination.path());
    let failure = receive_named_git_base_pack(
        destination.path(),
        "attempt",
        (&pack).into(),
        &mut Unread,
        PackReceiveLimits::default(),
        || false,
    )
    .unwrap_err();
    assert_eq!(failure.residue(), Some(received.path()));
    drop((received, observed, default, failure));
    assert_eq!(snapshot(destination.path()), saved);
    assert_eq!(snapshot(source.path()), before);
}

#[test]
fn invalid_admission_never_claims_or_reads() {
    let outer = private_fixture();
    let valid = private_fixture();
    let insecure = outer.path().join("insecure");
    fs::create_dir(&insecure).unwrap();
    fs::set_permissions(&insecure, fs::Permissions::from_mode(0o755)).unwrap();
    let alias = outer.path().join("alias");
    symlink(valid.path(), &alias).unwrap();
    let digest = ArtifactDigest::sha256(b"synthetic");
    for case in 0..12 {
        let mut parent = valid.path().to_path_buf();
        let mut name = "attempt".to_owned();
        let mut identity = expected(&digest);
        let mut limits = PackReceiveLimits::default();
        match case {
            0 => name.clear(),
            1 => name = "../escape".into(),
            2 => name = "/absolute".into(),
            3 => name = "a".repeat(256),
            4 => parent = PathBuf::from("relative"),
            5 => parent = outer.path().join("missing"),
            6 => parent.clone_from(&insecure),
            7 => parent.clone_from(&alias),
            8 => identity.base_commit = git2::Oid::ZERO_SHA1,
            9 => limits.encoded_bytes = 31,
            10 => parent = PathBuf::from(format!("/{}", "x".repeat(MAX_PACK_PATH_BYTES))),
            11 => {}
            _ => unreachable!(),
        }
        let failure = receive_with(
            &parent,
            &name,
            identity,
            &mut Unread,
            limits,
            &|| case == 11,
            &mut Operations {
                claim: &mut |_, _| panic!("unexpected claim"),
                rename: &mut |_, _, _| panic!("unexpected rename"),
            },
        )
        .unwrap_err();
        assert!(failure.residue().is_none());
        if case == 10 {
            assert_eq!(failure.reason, Error::Limit);
        }
        assert!(!format!("{failure:?} {failure}").contains(parent.to_str().unwrap()));
    }
    assert_eq!(fs::read_dir(valid.path()).unwrap().count(), 0);
}

#[test]
fn exact_candidate_path_limit_admits_only_the_boundary_before_claiming() {
    let root = private_fixture();
    let name = "attempt";
    let target_parent_bytes = MAX_PACK_PATH_BYTES - 1 - name.len();
    let mut parent = root.path().to_path_buf();
    while parent.as_os_str().len() < target_parent_bytes {
        let remaining = target_parent_bytes - parent.as_os_str().len();
        // Keep every component portable and avoid leaving room for only a separator.
        let component_bytes = if remaining == 129 {
            126
        } else {
            (remaining - 1).min(127)
        };
        parent.push("a".repeat(component_bytes));
    }
    fs::create_dir_all(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    let digest = ArtifactDigest::sha256(b"synthetic");
    for attempt in [name, "attemptx"] {
        let count = Cell::new(0);
        let candidate = parent.join(attempt);
        let boundary = attempt == name;
        assert_eq!(
            candidate.as_os_str().len(),
            MAX_PACK_PATH_BYTES + usize::from(!boundary)
        );
        let failure = receive_with(
            &parent,
            attempt,
            expected(&digest),
            &mut Unread,
            PackReceiveLimits::default(),
            &|| false,
            &mut Operations {
                claim: &mut |_, _| {
                    count.set(count.get() + 1);
                    Err(Errno::IO)
                },
                rename: &mut |_, _, _| panic!("no rename before a successful claim"),
            },
        )
        .unwrap_err();
        assert_eq!(count.get(), usize::from(boundary));
        assert_eq!(failure.reason, if boundary { Error::Storage } else { Error::Limit });
        assert_eq!(failure.residue(), boundary.then_some(candidate.as_path()));
    }
    assert_eq!(fs::read_dir(parent).unwrap().count(), 0);
}

#[test]
fn every_dispatched_claim_error_retains_only_an_attempted_locator_without_ownership() {
    let digest = ArtifactDigest::sha256(b"synthetic");
    for error in [Errno::IO, Errno::NOENT, Errno::EXIST, Errno::NOSYS] {
        for happened in [false, true] {
            let root = private_fixture();
            let count = Cell::new(0);
            let failure = receive_with(
                root.path(),
                "attempt",
                expected(&digest),
                &mut Unread,
                PackReceiveLimits::default(),
                &|| false,
                &mut Operations {
                    claim: &mut |parent, name| {
                        count.set(count.get() + 1);
                        if happened {
                            claim(parent, name)?;
                        }
                        Err(error)
                    },
                    rename: &mut |_, _, _| panic!("claim error cannot grant rename"),
                },
            )
            .unwrap_err();
            assert_eq!(count.get(), 1);
            assert_eq!(failure.reason, Error::Storage);
            assert_eq!(failure.residue(), Some(root.path().join("attempt").as_path()));
            assert_eq!(root.path().join("attempt").exists(), happened);
            drop(failure);
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), usize::from(happened));
        }
    }
}

#[test]
fn existing_empty_partial_file_and_link_attempts_never_grant_a_receiver() {
    let digest = ArtifactDigest::sha256(b"synthetic");
    for kind in 0..4 {
        let root = private_fixture();
        let target = root.path().join("attempt");
        match kind {
            0 | 1 => {
                fs::create_dir(&target).unwrap();
                if kind == 1 {
                    fs::write(target.join("partial"), b"retain").unwrap();
                }
            }
            2 => fs::write(&target, b"retain").unwrap(),
            3 => symlink("missing", &target).unwrap(),
            _ => unreachable!(),
        }
        let metadata = fs::symlink_metadata(&target).unwrap();
        let failure = receive_named_git_base_pack(
            root.path(),
            "attempt",
            expected(&digest),
            &mut Unread,
            PackReceiveLimits::default(),
            || false,
        )
        .unwrap_err();
        assert_eq!(failure.residue(), Some(target.as_path()));
        assert!(same_metadata(&metadata, &fs::symlink_metadata(&target).unwrap()));
        if kind == 1 {
            assert_eq!(fs::read(target.join("partial")).unwrap(), b"retain");
        }
    }
}

#[test]
fn successful_claim_followed_by_cancellation_or_parent_replacement_never_reads_input() {
    let digest = ArtifactDigest::sha256(b"synthetic");
    for replace in [false, true] {
        let outer = private_fixture();
        let root = outer.path().join("root");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let cancel = Cell::new(false);
        let failure = receive_with(
            &root,
            "attempt",
            expected(&digest),
            &mut Unread,
            PackReceiveLimits::default(),
            &|| cancel.get(),
            &mut Operations {
                claim: &mut |parent, name| {
                    claim(parent, name)?;
                    if replace {
                        fs::rename(&root, outer.path().join("retained")).unwrap();
                        fs::create_dir(&root).unwrap();
                        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
                    } else {
                        cancel.set(true);
                    }
                    Ok(())
                },
                rename: &mut |_, _, _| panic!("no rename after failed claim verification"),
            },
        )
        .unwrap_err();
        assert_eq!(failure.residue(), Some(root.join("attempt").as_path()));
        let retained = if replace {
            outer.path().join("retained/attempt")
        } else {
            root.join("attempt")
        };
        assert_eq!(fs::read_dir(retained).unwrap().count(), 0);
        assert_eq!(
            failure.reason,
            if replace { Error::UnsafeParent } else { Error::Cancelled }
        );
    }
}

#[test]
fn rename_errors_retain_partial_or_complete_names_without_replay() {
    let source = private_fixture();
    let pack = pack(source.path());
    for failed_call in 1..=2 {
        for happened in [false, true] {
            let root = private_fixture();
            let calls = Cell::new(0);
            let failure = receive_with(
                root.path(),
                "attempt",
                (&pack).into(),
                &mut File::open(pack.path()).unwrap(),
                PackReceiveLimits::default(),
                &|| false,
                &mut Operations {
                    claim: &mut claim,
                    rename: &mut |directory, source, target| {
                        calls.set(calls.get() + 1);
                        if calls.get() == failed_call {
                            if happened {
                                relocate(directory, source, target)?;
                            }
                            Err(Errno::IO)
                        } else {
                            relocate(directory, source, target)
                        }
                    },
                },
            )
            .unwrap_err();
            assert_eq!(failure.reason, Error::Storage);
            assert_eq!(calls.get(), failed_call);
            let path = failure.residue().unwrap();
            let names: Vec<_> = fs::read_dir(path.join("decoded/objects/pack"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            assert_eq!(names.len(), 2);
            assert_eq!(
                names.iter().filter(|name| name.starts_with("pack-")).count(),
                failed_call - 1 + usize::from(happened)
            );
            assert!(observe_git_base_pack(path, (&pack).into(), PackReceiveLimits::default(), || false).is_err());
            let before = snapshot(root.path());
            drop(failure);
            assert_eq!(snapshot(root.path()), before);
            assert!(
                receive_named_git_base_pack(
                    root.path(),
                    "attempt",
                    (&pack).into(),
                    &mut Unread,
                    PackReceiveLimits::default(),
                    || false
                )
                .is_err()
            );
        }
    }
}

fn pair(root: &Path) -> Reservation {
    let parent = staging::private_parent(root).unwrap();
    claim(parent.root.handle(), "attempt").unwrap();
    let reservation = Reservation {
        root: directory(parent.root.handle(), "attempt", true).unwrap(),
        parent,
        parent_path: root.to_path_buf(),
        name: "attempt".into(),
    };
    let directory = root.join("attempt/decoded/objects/pack");
    fs::create_dir_all(&directory).unwrap();
    for extension in ["pack", "idx"] {
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(directory.join(format!("received.{extension}")))
            .unwrap()
            .write_all(extension.as_bytes())
            .unwrap();
    }
    reservation
}

#[test]
fn cancellation_at_each_private_rename_boundary_retains_the_observed_phase() {
    for after in 0..=2 {
        let root = private_fixture();
        let reservation = pair(root.path());
        let calls = Cell::new(0);
        let result = reservation.relocate(HASH, &|| calls.get() == after, &mut |directory, source, target| {
            calls.set(calls.get() + 1);
            relocate(directory, source, target)
        });
        assert_eq!(result, Err(Error::Cancelled));
        assert_eq!(calls.get(), after);
        assert_eq!(
            fs::read_dir(root.path().join("attempt/decoded/objects/pack"))
                .unwrap()
                .count(),
            2
        );
    }
}

#[test]
fn foreign_nodes_destinations_and_changed_directory_bindings_prevent_renames() {
    for case in 0..8 {
        let root = private_fixture();
        let reservation = pair(root.path());
        let path = root.path().join("attempt/decoded/objects/pack");
        let source = path.join("received.pack");
        match case {
            0 => fs::write(path.join("foreign"), b"retain").unwrap(),
            1 => fs::write(path.join(format!("pack-{HASH}.pack")), b"retain").unwrap(),
            2 => {
                fs::rename(&source, root.path().join("original")).unwrap();
                symlink(root.path().join("original"), &source).unwrap();
            }
            3 => fs::hard_link(&source, root.path().join("alias")).unwrap(),
            4 => fs::set_permissions(&source, fs::Permissions::from_mode(0o644)).unwrap(),
            5 => {
                fs::rename(root.path().join("attempt"), root.path().join("retained")).unwrap();
                fs::create_dir(root.path().join("attempt")).unwrap();
            }
            6 => {
                fs::rename(&path, root.path().join("retained")).unwrap();
                symlink(root.path().join("retained"), &path).unwrap();
            }
            7 => fs::set_permissions(root.path().join("attempt"), fs::Permissions::from_mode(0o755)).unwrap(),
            _ => unreachable!(),
        }
        assert!(
            reservation
                .relocate(HASH, &|| false, &mut |_, _, _| panic!("unsafe rename"))
                .is_err()
        );
        if case == 1 {
            assert_eq!(fs::read(path.join(format!("pack-{HASH}.pack"))).unwrap(), b"retain");
        }
    }
}

#[test]
fn changes_after_first_rename_cannot_silently_replace_the_second_node() {
    for case in 0..4 {
        let root = private_fixture();
        let reservation = pair(root.path());
        let path = root.path().join("attempt/decoded/objects/pack");
        let calls = Cell::new(0);
        let result = reservation.relocate(HASH, &|| false, &mut |directory, source, target| {
            calls.set(calls.get() + 1);
            relocate(directory, source, target)?;
            match case {
                0 => fs::write(path.join("received.idx"), b"changed").unwrap(),
                1 => fs::write(path.join(target), b"changed").unwrap(),
                2 => fs::hard_link(path.join(target), root.path().join("alias")).unwrap(),
                3 => fs::write(path.join(format!("pack-{HASH}.idx")), b"retain").unwrap(),
                _ => unreachable!(),
            }
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(calls.get(), 1);
    }
}

#[test]
fn streamed_digest_eof_read_errors_cancellation_and_wrong_base_keep_the_claim() {
    struct Reader<'a>(&'a mut dyn FnMut(&mut [u8]) -> io::Result<usize>);
    impl Read for Reader<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.0(buffer)
        }
    }
    let source = private_fixture();
    let pack = pack(source.path());
    for kind in 0..6 {
        let root = private_fixture();
        let mut bytes = fs::read(pack.path()).unwrap();
        match kind {
            0 => {
                bytes.pop();
            }
            1 => bytes.push(0),
            2 => bytes[15] ^= 1,
            _ => {}
        }
        let cancelled = Cell::new(false);
        let mut cursor = Cursor::new(bytes);
        let mut input = |buffer: &mut [u8]| {
            if kind == 3 {
                return Err(io::Error::other("private-marker"));
            }
            let result = cursor.read(buffer);
            cancelled.set(kind == 4);
            result
        };
        let mut identity = (&pack).into();
        if kind == 5 {
            identity = expected(pack.sha256());
            identity.encoded_bytes = pack.encoded_bytes();
        }
        let failure = receive_named_git_base_pack(
            root.path(),
            "attempt",
            identity,
            &mut Reader(&mut input),
            PackReceiveLimits::default(),
            || cancelled.get(),
        )
        .unwrap_err();
        assert_eq!(failure.residue(), Some(root.path().join("attempt").as_path()));
        assert!(!format!("{failure:?} {failure}").contains("private-marker"));
        assert!(!root.path().join("attempt/decoded/objects/pack/received.idx").exists());
    }
}
