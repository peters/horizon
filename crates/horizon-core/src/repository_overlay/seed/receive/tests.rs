use super::*;
use crate::repository_overlay::{
    OverlayChange, OverlayContent,
    bundle::{VerifiedOverlayBlob, codec},
    checkout::prepare_private_checkout,
    namespace::resolve_namespaces_from_source,
    seed::{
        export::prepare_git_base_pack,
        packed::PackedGitObjectSource,
        prepare_git_seed,
        tests::{Fixture, commit, private_fixture, snapshot},
    },
};
use git2::Repository;
use std::{
    cell::Cell,
    fs::File,
    io::Cursor,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    process::{Command, Stdio},
    time::Duration,
};

fn prepared(fixture: &Fixture, parent: &Path, overlay: bool) -> (PreparedGitPack, Box<[u8]>) {
    let resolved = if overlay {
        let blob = VerifiedOverlayBlob::new(b"staged-only bytes".to_vec()).unwrap();
        let added = OverlayChange::new(
            "staged-only".into(),
            OverlayContent::File {
                sha256: blob.sha256().clone(),
                bytes: blob.bytes().len() as u64,
                executable: false,
            },
        )
        .unwrap();
        let removed = OverlayChange::new("removed".into(), OverlayContent::Remove).unwrap();
        fixture.resolve(vec![added], vec![removed], vec![blob])
    } else {
        fixture.resolve(vec![], vec![], vec![])
    };
    let bundle = codec::encode(resolved.bundle()).unwrap();
    let seed = prepare_git_seed(parent, &resolved, &mut fixture.source(), || false).unwrap();
    (
        prepare_git_base_pack(parent, &seed, PackExportLimits::default(), || false).unwrap(),
        bundle,
    )
}

fn receive_pack(parent: &Path, pack: &PreparedGitPack) -> ReceivedGitPack {
    receive_git_base_pack(
        parent,
        pack.into(),
        &mut File::open(pack.path()).unwrap(),
        PackReceiveLimits::default(),
        || false,
    )
    .unwrap()
}

#[test]
fn actual_export_receipt_and_existing_checkout_preserve_base_and_overlay_semantics() {
    let fixture = Fixture::new();
    let source = private_fixture();
    let (pack, bundle) = prepared(&fixture, source.path(), true);
    let before = snapshot(source.path());
    let original = snapshot(fixture.directory.path());
    let destination = private_fixture();
    let received = receive_pack(destination.path(), &pack);
    assert_eq!(received.base_commit(), fixture.commit);
    assert_eq!(received.sha256(), pack.sha256());
    assert_eq!(received.encoded_bytes(), pack.encoded_bytes());
    assert_eq!(received.objects(), 3);
    let mut reader = PackedGitObjectSource::new(
        destination.path(),
        received.objects_directory(),
        PackedSourceLimits::default(),
        || false,
    )
    .unwrap();
    let resolved = resolve_namespaces_from_source(&mut reader, codec::decode(&bundle).unwrap(), || false).unwrap();
    let checkout = prepare_private_checkout(destination.path(), &resolved, &mut reader, || false).unwrap();
    assert_eq!(fs::read(checkout.path().join("kept")).unwrap(), b"base\0literal\xff");
    assert_eq!(
        fs::read(checkout.path().join("staged-only")).unwrap(),
        b"staged-only bytes"
    );
    assert!(!checkout.path().join("removed").exists());
    let repository = Repository::open(checkout.path()).unwrap();
    assert_eq!(repository.head().unwrap().target(), Some(fixture.commit));
    assert!(repository.head_detached().unwrap() && repository.is_shallow());
    assert!(repository.find_blob(fixture.private_blob).is_err());
    assert_eq!(snapshot(source.path()), before);
    assert_eq!(snapshot(fixture.directory.path()), original);
    for entry in fs::read_dir(received.objects_directory().join("pack")).unwrap() {
        let metadata = entry.unwrap().metadata().unwrap();
        assert!(metadata.is_file());
        assert_eq!((metadata.mode() & 0o7777, metadata.nlink()), (0o600, 1));
    }
    let retained = received.path().to_path_buf();
    assert!(!format!("{received:?}").contains(retained.to_str().unwrap()));
    drop(received);
    assert!(retained.exists());
}

#[test]
fn initial_empty_and_large_nested_repeated_base_objects_round_trip() {
    for shape in ["initial", "empty", "large"] {
        let mut fixture = Fixture::new();
        let mut bytes = vec![];
        fixture.commit = match shape {
            "initial" => fixture.ancestor,
            "empty" => commit(&fixture.repository, &[], &[fixture.commit]),
            _ => {
                bytes = vec![b'a'; 65 * 1024 * 1024 + 1];
                *bytes.last_mut().unwrap() = b'z';
                let blob = fixture.repository.blob(&bytes).unwrap();
                commit(
                    &fixture.repository,
                    &[("kept", blob, 0o100_644), ("nested/leaf", blob, 0o100_755)],
                    &[fixture.commit],
                )
            }
        };
        let source = private_fixture();
        let (pack, bundle) = prepared(&fixture, source.path(), false);
        let destination = private_fixture();
        let received = receive_pack(destination.path(), &pack);
        assert_eq!(
            received.objects(),
            if shape == "empty" {
                2
            } else if shape == "large" {
                4
            } else {
                3
            }
        );
        let mut reader = PackedGitObjectSource::new(
            destination.path(),
            received.objects_directory(),
            PackedSourceLimits::default(),
            || false,
        )
        .unwrap();
        let resolved = resolve_namespaces_from_source(&mut reader, codec::decode(&bundle).unwrap(), || false).unwrap();
        let checkout = prepare_private_checkout(destination.path(), &resolved, &mut reader, || false).unwrap();
        if shape == "large" {
            assert_eq!(fs::read(checkout.path().join("nested/leaf")).unwrap(), bytes);
            assert_eq!(
                fs::metadata(checkout.path().join("nested/leaf")).unwrap().mode() & 0o111,
                0o111
            );
        }
    }
}

#[test]
fn invalid_metadata_limits_and_unsafe_parent_fail_without_read_or_residue() {
    struct Unread;
    impl Read for Unread {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("invalid preflight read input")
        }
    }
    let fixture = Fixture::new();
    let parent = private_fixture();
    let digest = ArtifactDigest::sha256(b"unread");
    let valid = ExpectedGitPack {
        base_commit: fixture.commit,
        sha256: &digest,
        encoded_bytes: 32,
    };
    let before = snapshot(parent.path());
    for expected in [
        ExpectedGitPack {
            base_commit: Oid::ZERO_SHA1,
            ..valid
        },
        ExpectedGitPack {
            encoded_bytes: 31,
            ..valid
        },
        ExpectedGitPack {
            encoded_bytes: PackExportLimits::MAX_ENCODED_BYTES + 1,
            ..valid
        },
    ] {
        let failure = receive_git_base_pack(
            parent.path(),
            expected,
            &mut Unread,
            PackReceiveLimits::default(),
            || false,
        )
        .unwrap_err();
        assert!(failure.residue().is_none());
    }
    for encoded_bytes in [0, 31, PackExportLimits::MAX_ENCODED_BYTES + 1] {
        let limits = PackReceiveLimits {
            encoded_bytes,
            ..PackReceiveLimits::default()
        };
        assert!(
            receive_git_base_pack(parent.path(), valid, &mut Unread, limits, || false)
                .unwrap_err()
                .residue()
                .is_none()
        );
    }
    let limits = PackReceiveLimits {
        source: PackedSourceLimits {
            object_timeout: Duration::ZERO,
            ..PackedSourceLimits::default()
        },
        ..PackReceiveLimits::default()
    };
    assert!(
        receive_git_base_pack(parent.path(), valid, &mut Unread, limits, || false)
            .unwrap_err()
            .residue()
            .is_none()
    );
    assert!(
        receive_git_base_pack(parent.path(), valid, &mut Unread, PackReceiveLimits::default(), || true)
            .unwrap_err()
            .residue()
            .is_none()
    );
    assert_eq!(snapshot(parent.path()), before);
    let alias = parent.path().join("alias");
    symlink(parent.path(), &alias).unwrap();
    for unsafe_parent in [
        alias.as_path(),
        Path::new("relative"),
        Path::new("/not-a-receiver-directory"),
    ] {
        let failure = receive_git_base_pack(unsafe_parent, valid, &mut Unread, PackReceiveLimits::default(), || {
            false
        })
        .unwrap_err();
        assert_eq!(failure.reason, SeedError::UnsafeParent);
        assert!(failure.residue().is_none());
    }
}

#[test]
fn non_private_parent_fails_before_reading() {
    let fixture = Fixture::new();
    let parent = private_fixture();
    fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let digest = ArtifactDigest::sha256(b"unread");
    let expected = ExpectedGitPack {
        base_commit: fixture.commit,
        sha256: &digest,
        encoded_bytes: 32,
    };
    let failure = receive_git_base_pack(
        parent.path(),
        expected,
        &mut io::empty(),
        PackReceiveLimits::default(),
        || false,
    )
    .unwrap_err();
    assert_eq!(failure.reason, SeedError::UnsafeParent);
    assert!(failure.residue().is_none());
}

#[test]
fn mismatched_truncated_trailing_and_corrupt_input_remains_unconfirmed() {
    let fixture = Fixture::new();
    let source = private_fixture();
    let (pack, _) = prepared(&fixture, source.path(), false);
    let original = fs::read(pack.path()).unwrap();
    for kind in ["digest", "short", "extra", "checksum", "wrong-base"] {
        let mut bytes = original.clone();
        if kind == "short" {
            bytes.pop();
        }
        if kind == "extra" {
            bytes.push(0);
        }
        if kind == "checksum" {
            *bytes.last_mut().unwrap() ^= 1;
        }
        let digest = if kind == "digest" {
            ArtifactDigest::sha256(b"different")
        } else {
            ArtifactDigest::sha256(&bytes)
        };
        let expected = ExpectedGitPack {
            base_commit: if kind == "wrong-base" {
                fixture.ancestor
            } else {
                fixture.commit
            },
            sha256: &digest,
            encoded_bytes: pack.encoded_bytes(),
        };
        let destination = private_fixture();
        let failure = receive_git_base_pack(
            destination.path(),
            expected,
            &mut Cursor::new(bytes),
            PackReceiveLimits::default(),
            || false,
        )
        .unwrap_err();
        let retained = failure.residue().unwrap().to_path_buf();
        assert!(!format!("{failure:?} {failure}").contains(retained.to_str().unwrap()));
        assert!(
            fs::metadata(retained.join("decoded/objects/pack/received.pack"))
                .unwrap()
                .len()
                <= pack.encoded_bytes()
        );
        if matches!(kind, "digest" | "short" | "extra") {
            assert!(!retained.join("decoded/objects/pack/received.idx").exists());
        }
        drop(failure);
        assert!(retained.exists());
    }
    assert_eq!(fs::read(pack.path()).unwrap(), original);
}

#[test]
fn otherwise_valid_packs_with_extras_or_non_commit_roots_are_rejected() {
    let fixture = Fixture::new();
    let root = fixture.repository.find_commit(fixture.commit).unwrap().tree_id();
    let old_root = fixture.repository.find_commit(fixture.ancestor).unwrap().tree_id();
    let tag = fixture
        .repository
        .odb()
        .unwrap()
        .write(
            git2::ObjectType::Tag,
            format!(
                "object {}\ntype commit\ntag receipt\ntagger Fixture <fixture@example.invalid> 1 +0000\n\nreceipt\n",
                fixture.ancestor
            )
            .as_bytes(),
        )
        .unwrap();
    for (base_commit, ids) in [
        (
            fixture.commit,
            vec![fixture.commit, root, fixture.base_blob, fixture.private_blob],
        ),
        (
            fixture.commit,
            vec![
                fixture.commit,
                root,
                fixture.base_blob,
                fixture.ancestor,
                old_root,
                fixture.private_blob,
            ],
        ),
        (fixture.base_blob, vec![fixture.base_blob]),
        (root, vec![root, fixture.base_blob]),
        (tag, vec![tag, fixture.ancestor, old_root, fixture.private_blob]),
    ] {
        let temporary = private_fixture();
        let request = temporary.path().join("objects");
        let mut input = File::create(&request).unwrap();
        for id in ids {
            writeln!(input, "{id}").unwrap();
        }
        drop(input);
        let result = Command::new("/usr/bin/git")
            .args(["--no-replace-objects", "--git-dir"])
            .arg(fixture.repository.path())
            .args(["pack-objects", "--stdout", "--window=0", "--depth=0", "--threads=1"])
            .env_clear()
            .envs([
                ("GIT_CONFIG_NOSYSTEM", "1"),
                ("GIT_CONFIG_GLOBAL", "/dev/null"),
                ("GIT_ALLOW_PROTOCOL", ""),
            ])
            .stdin(Stdio::from(File::open(request).unwrap()))
            .output()
            .unwrap();
        assert!(result.status.success());
        let digest = ArtifactDigest::sha256(&result.stdout);
        let expected = ExpectedGitPack {
            base_commit,
            sha256: &digest,
            encoded_bytes: result.stdout.len() as u64,
        };
        let failure = receive_git_base_pack(
            temporary.path(),
            expected,
            &mut result.stdout.as_slice(),
            PackReceiveLimits::default(),
            || false,
        )
        .unwrap_err();
        assert_eq!(failure.reason, SeedError::Object);
        let files: Vec<_> = fs::read_dir(failure.residue().unwrap().join("decoded/objects/pack"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        if base_commit == tag {
            assert_eq!(files.len(), 1);
            assert_eq!(files[0], "received.pack");
            continue;
        }
        assert_eq!(
            files.len(),
            2,
            "strict decoding succeeded but exact commit closure must reject this pack"
        );
        assert!(files.iter().all(|name| name.to_str().unwrap().starts_with("pack-")));
    }
}

#[test]
fn short_reads_cancellation_and_private_reader_errors_preserve_unconfirmed_data() {
    struct Chunks<'a> {
        input: Cursor<&'a [u8]>,
        cancel: &'a Cell<bool>,
        fail: bool,
        stop: bool,
    }
    impl Read for Chunks<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if self.fail {
                return Err(io::Error::other("private-reader-marker"));
            }
            let count = bytes.len().min(3);
            let read = self.input.read(&mut bytes[..count]);
            self.cancel.set(self.stop);
            read
        }
    }
    let fixture = Fixture::new();
    let source = private_fixture();
    let (pack, _) = prepared(&fixture, source.path(), false);
    let bytes = fs::read(pack.path()).unwrap();
    for (fail, stop) in [(false, false), (true, false), (false, true)] {
        let destination = private_fixture();
        let cancelled = Cell::new(false);
        let mut input = Chunks {
            input: Cursor::new(&bytes),
            cancel: &cancelled,
            fail,
            stop,
        };
        let result = receive_git_base_pack(
            destination.path(),
            (&pack).into(),
            &mut input,
            PackReceiveLimits::default(),
            || cancelled.get(),
        );
        if fail || stop {
            let failure = result.unwrap_err();
            assert_eq!(
                failure.reason,
                if stop { SeedError::Cancelled } else { SeedError::Source }
            );
            assert!(!format!("{failure:?} {failure}").contains("private-reader-marker"));
            assert!(failure.residue().unwrap().exists());
            assert!(
                !failure
                    .residue()
                    .unwrap()
                    .join("decoded/objects/pack/received.idx")
                    .exists()
            );
        } else {
            assert_eq!(result.unwrap().objects(), 3);
        }
    }
}

#[test]
fn receipt_commands_use_only_the_isolated_pack_and_bounded_exact_traversal() {
    let parent = private_fixture();
    let metadata = staging::reserve(parent.path(), &|| false).unwrap();
    let limits = PackedSourceLimits::default();
    let command = view::index_command(&metadata, Oid::ZERO_SHA1, limits, 4096).unwrap();
    let args: Vec<_> = command.get_args().map(|arg| arg.to_str().unwrap()).collect();
    assert_eq!(command.get_program(), "/usr/bin/prlimit");
    for required in [
        "index-pack",
        "--strict",
        "--threads=1",
        "--no-rev-index",
        "--object-format=sha1",
        "--max-input-size=4096",
    ] {
        assert!(args.contains(&required));
    }
    for forbidden in ["--stdin", "--fix-thin", "--promisor", "--fsck-objects"] {
        assert!(!args.contains(&forbidden));
    }
    assert!(
        command
            .get_envs()
            .all(|(key, _)| key != "GIT_ALTERNATE_OBJECT_DIRECTORIES")
    );
    assert_eq!(
        fs::read_to_string(metadata.join("shallow")).unwrap(),
        format!("{}\n", Oid::ZERO_SHA1)
    );
    let selection = staging::reserve(parent.path(), &|| false).unwrap();
    let command = view::command(
        &selection,
        &metadata.join("objects"),
        limits,
        view::Operation::Closure(Oid::ZERO_SHA1),
    )
    .unwrap();
    let args: Vec<_> = command.get_args().map(|arg| arg.to_str().unwrap()).collect();
    assert_eq!(
        &args[args.len() - 4..],
        ["rev-list", "--objects", "--no-object-names", "--stdin"]
    );
    assert!(args.contains(&format!("--as={0}:{0}", limits.address_space_bytes).as_str()));
    assert!(args.contains(&format!("--cpu={0}:{0}", limits.cpu_seconds).as_str()));
}
