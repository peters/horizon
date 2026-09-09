use super::*;
use crate::repository_overlay::seed::{
    export::{PackExportLimits, PreparedGitPack, prepare_git_base_pack},
    prepare_git_seed,
    receive::receive_git_base_pack,
    tests::{Fixture, commit, private_fixture},
};
use crate::repository_overlay::{
    bundle::codec,
    checkout::prepare_private_checkout,
    namespace::resolve_namespaces_from_source,
    seed::packed::{PackedGitObjectSource, PackedSourceLimits},
};
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs::{self, File},
    io::{Seek, SeekFrom},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt, symlink},
        net::UnixListener,
    },
    path::PathBuf,
    process::Command,
    time::Duration,
};

struct Prepared {
    fixture: Fixture,
    _source: tempfile::TempDir,
    destination: tempfile::TempDir,
    pack: PreparedGitPack,
    path: PathBuf,
}

impl Prepared {
    fn new() -> Self {
        Self::from_fixture(Fixture::new())
    }

    fn from_fixture(fixture: Fixture) -> Self {
        let source = private_fixture();
        let resolved = fixture.resolve(vec![], vec![], vec![]);
        let seed = prepare_git_seed(source.path(), &resolved, &mut fixture.source(), || false).unwrap();
        let pack = prepare_git_base_pack(source.path(), &seed, PackExportLimits::default(), || false).unwrap();
        let destination = private_fixture();
        let received = receive_git_base_pack(
            destination.path(),
            (&pack).into(),
            &mut File::open(pack.path()).unwrap(),
            PackReceiveLimits::default(),
            || false,
        )
        .unwrap();
        let path = received.path().to_path_buf();
        drop(received);
        Self {
            fixture,
            _source: source,
            destination,
            pack,
            path,
        }
    }

    fn observe(&self) -> Result<ReceivedGitPack, SeedError> {
        observe_git_base_pack(&self.path, (&self.pack).into(), PackReceiveLimits::default(), || false)
    }

    fn packed(&self) -> PathBuf {
        fs::read_dir(self.path.join("decoded/objects/pack"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().unwrap() == "pack")
            .unwrap()
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(in super::super) struct SnapshotNode {
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
    bytes: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    contents: Vec<u8>,
}

pub(in super::super) fn snapshot(root: &Path) -> BTreeMap<PathBuf, SnapshotNode> {
    let mut result = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path).unwrap();
        if metadata.is_dir() {
            pending.extend(fs::read_dir(&path).unwrap().map(|entry| entry.unwrap().path()));
        }
        let contents = if metadata.is_file() {
            fs::read(&path).unwrap()
        } else if metadata.is_symlink() {
            fs::read_link(&path).unwrap().as_os_str().as_bytes().to_vec()
        } else {
            vec![]
        };
        result.insert(
            path,
            SnapshotNode {
                device: metadata.dev(),
                inode: metadata.ino(),
                mode: metadata.mode(),
                links: metadata.nlink(),
                bytes: metadata.len(),
                modified: (metadata.mtime(), metadata.mtime_nsec()),
                changed: (metadata.ctime(), metadata.ctime_nsec()),
                contents,
            },
        );
    }
    result
}

#[test]
fn dropped_receipt_reopens_without_changing_existing_input() {
    let prepared = Prepared::new();
    let before = snapshot(prepared.destination.path());
    let observed = prepared.observe().unwrap();
    assert_eq!(observed.base_commit(), prepared.fixture.commit);
    assert_eq!(observed.sha256(), prepared.pack.sha256());
    assert_eq!(observed.encoded_bytes(), prepared.pack.encoded_bytes());
    assert_eq!(observed.objects(), 3);
    let scratch = private_fixture();
    let mut reader = PackedGitObjectSource::new(
        scratch.path(),
        observed.objects_directory(),
        PackedSourceLimits::default(),
        || false,
    )
    .unwrap();
    let encoded = codec::encode(prepared.fixture.resolve(vec![], vec![], vec![]).bundle()).unwrap();
    let bundle = codec::decode(&encoded).unwrap();
    let resolved = resolve_namespaces_from_source(&mut reader, bundle, || false).unwrap();
    let checkout = prepare_private_checkout(scratch.path(), &resolved, &mut reader, || false).unwrap();
    assert_eq!(fs::read(checkout.path().join("kept")).unwrap(), b"base\0literal\xff");
    let repository = git2::Repository::open(checkout.path()).unwrap();
    assert_eq!(repository.head().unwrap().target(), Some(prepared.fixture.commit));
    assert!(repository.find_commit(prepared.fixture.ancestor).is_err());
    assert_eq!(snapshot(prepared.destination.path()), before);
    drop(observed);
    assert_eq!(snapshot(prepared.destination.path()), before);
}

#[test]
fn corrupt_missing_extra_and_unsafe_nodes_are_not_repaired_or_removed() {
    for case in [
        "pack",
        "index",
        "missing-index",
        "head",
        "config",
        "shallow",
        "selection",
        "alternates",
        "extra",
        "reference",
        "root-mode",
        "view-mode",
        "file-mode",
        "hardlink",
        "symlink",
        "socket",
    ] {
        let prepared = Prepared::new();
        let pack = prepared.packed();
        let index = pack.with_extension("idx");
        let _listener = if case == "socket" {
            Some(UnixListener::bind(prepared.path.join("socket")).unwrap())
        } else {
            None
        };
        match case {
            "pack" | "index" => {
                let path = if case == "pack" { &pack } else { &index };
                let mut bytes = fs::read(path).unwrap();
                let last = bytes.len() - 1;
                bytes[last] ^= 1;
                fs::write(path, bytes).unwrap();
            }
            "missing-index" => fs::remove_file(index).unwrap(),
            "head" => fs::write(prepared.path.join("decoded/HEAD"), "ref: refs/heads/private-marker\n").unwrap(),
            "config" => fs::write(prepared.path.join("decoded/config"), "[include]\npath=private-marker\n").unwrap(),
            "shallow" => fs::write(
                prepared.path.join("decoded/shallow"),
                format!("{}\n", prepared.fixture.ancestor),
            )
            .unwrap(),
            "selection" => fs::write(prepared.path.join("selection/config"), "private-marker").unwrap(),
            "alternates" => fs::write(prepared.path.join("decoded/objects/info/alternates"), "private-marker").unwrap(),
            "extra" => fs::write(prepared.path.join("private-marker"), "unchanged").unwrap(),
            "reference" => fs::write(prepared.path.join("decoded/refs/unexpected"), "private-marker").unwrap(),
            "root-mode" => fs::set_permissions(&prepared.path, fs::Permissions::from_mode(0o755)).unwrap(),
            "view-mode" => {
                fs::set_permissions(prepared.path.join("decoded"), fs::Permissions::from_mode(0o755)).unwrap();
            }
            "file-mode" => fs::set_permissions(&pack, fs::Permissions::from_mode(0o640)).unwrap(),
            "hardlink" => fs::hard_link(&pack, prepared.destination.path().join("extra-link")).unwrap(),
            "symlink" => {
                let retained = prepared.destination.path().join("retained-pack");
                fs::rename(&pack, &retained).unwrap();
                symlink(retained, &pack).unwrap();
            }
            "socket" => {}
            _ => unreachable!(),
        }
        let before = snapshot(prepared.destination.path());
        let error = prepared.observe().unwrap_err();
        assert!(!format!("{error:?} {error}").contains("private-marker"));
        assert!(!format!("{error:?} {error}").contains(prepared.path.to_str().unwrap()));
        assert_eq!(snapshot(prepared.destination.path()), before, "{case}");
    }
}

#[test]
fn expectation_errors_missing_roots_and_cancellation_are_read_only() {
    let prepared = Prepared::new();
    let before = snapshot(prepared.destination.path());
    let wrong_digest = crate::cloud_run::ArtifactDigest::sha256(b"unrelated");
    for case in [
        "digest",
        "base",
        "zero-base",
        "short",
        "long",
        "missing",
        "relative",
        "limit",
    ] {
        let mut expected = ExpectedGitPack::from(&prepared.pack);
        let mut limits = PackReceiveLimits::default();
        let mut path = prepared.path.clone();
        match case {
            "digest" => expected.sha256 = &wrong_digest,
            "base" => expected.base_commit = prepared.fixture.ancestor,
            "zero-base" => expected.base_commit = git2::Oid::ZERO_SHA1,
            "short" => expected.encoded_bytes -= 1,
            "long" => expected.encoded_bytes += 1,
            "missing" => path = prepared.destination.path().join("absent"),
            "relative" => path = PathBuf::from("relative"),
            "limit" => limits.encoded_bytes = expected.encoded_bytes - 1,
            _ => unreachable!(),
        }
        assert!(
            observe_git_base_pack(&path, expected, limits, || false).is_err(),
            "{case}"
        );
        assert_eq!(snapshot(prepared.destination.path()), before, "{case}");
    }
    for threshold in [0, 1, 3, 15, 25, 45, 70] {
        let calls = Cell::new(0);
        let error = observe_git_base_pack(
            &prepared.path,
            (&prepared.pack).into(),
            PackReceiveLimits::default(),
            || {
                let call = calls.get();
                calls.set(call + 1);
                call >= threshold
            },
        )
        .unwrap_err();
        assert_eq!(error, SeedError::Cancelled, "{threshold}");
        assert_eq!(snapshot(prepared.destination.path()), before);
    }
}

#[test]
fn pack_name_identity_cannot_be_substituted() {
    let prepared = Prepared::new();
    let pack = prepared.packed();
    let renamed = pack.with_file_name(format!("pack-{}.pack", "0".repeat(40)));
    fs::rename(pack.with_extension("idx"), renamed.with_extension("idx")).unwrap();
    fs::rename(pack, renamed).unwrap();
    let before = snapshot(prepared.destination.path());
    assert!(prepared.observe().is_err());
    assert_eq!(snapshot(prepared.destination.path()), before);
}

#[test]
fn held_root_and_file_bindings_reject_replacement_without_mutation() {
    for replace_root in [false, true] {
        let prepared = Prepared::new();
        let layout = layout::Layout::open(&prepared.path, (&prepared.pack).into(), &|| false).unwrap();
        if replace_root {
            fs::rename(&prepared.path, prepared.destination.path().join("retained-original")).unwrap();
            fs::create_dir(&prepared.path).unwrap();
            fs::set_permissions(&prepared.path, fs::Permissions::from_mode(0o700)).unwrap();
            fs::write(prepared.path.join("sentinel"), "unchanged").unwrap();
        } else {
            fs::write(prepared.path.join("decoded/config"), "changed").unwrap();
        }
        let before = snapshot(prepared.destination.path());
        assert!(layout.recheck(&|| false).is_err());
        assert_eq!(snapshot(prepared.destination.path()), before);
    }
}

#[test]
fn observation_commands_are_read_only_and_native_output_exit_and_deadline_are_checked() {
    let prepared = Prepared::new();
    let pack = prepared.packed();
    let limits = PackedSourceLimits::default();
    let before = snapshot(prepared.destination.path());
    let command = view::verify_command(
        &prepared.path.join("decoded"),
        &pack,
        &pack.with_extension("idx"),
        limits,
        prepared.pack.encoded_bytes(),
    );
    let args: Vec<_> = command.get_args().map(|arg| arg.to_str().unwrap()).collect();
    assert_eq!(command.get_program(), "/usr/bin/prlimit");
    for required in [
        "index-pack",
        "--verify",
        "--strict",
        "--no-rev-index",
        "--threads=1",
        "--object-format=sha1",
    ] {
        assert!(args.contains(&required));
    }
    assert!(!args.contains(&"--stdin") && !args.contains(&"--fix-thin"));
    assert!(
        !command
            .get_envs()
            .any(|(key, _)| key == "GIT_ALTERNATE_OBJECT_DIRECTORIES")
    );
    assert_eq!(snapshot(prepared.destination.path()), before);
    for (script, expected) in [
        ("exit 0", None),
        ("printf unexpected", Some(SeedError::Object)),
        ("exit 1", Some(SeedError::Source)),
        ("exec sleep 30", Some(SeedError::Source)),
    ] {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        let limits = PackedSourceLimits {
            object_timeout: Duration::from_millis(100),
            ..limits
        };
        assert_eq!(native::verify(command, limits, &|| false).err(), expected);
        assert_eq!(snapshot(prepared.destination.path()), before);
    }
}

#[test]
fn initial_and_empty_parented_bases_reopen() {
    for empty in [false, true] {
        let mut fixture = Fixture::new();
        fixture.commit = if empty {
            commit(&fixture.repository, &[], &[fixture.commit])
        } else {
            fixture.ancestor
        };
        let prepared = Prepared::from_fixture(fixture);
        let before = snapshot(prepared.destination.path());
        let observed = prepared.observe().unwrap();
        assert_eq!(observed.base_commit(), prepared.fixture.commit);
        assert_eq!(observed.objects(), if empty { 2 } else { 3 });
        assert_eq!(snapshot(prepared.destination.path()), before);
    }
}

#[test]
fn large_nested_repeated_objects_reopen_and_materialize_without_input_changes() {
    const BYTES: usize = 65 * 1024 * 1024 + 1;
    let mut fixture = Fixture::new();
    let mut bytes = vec![b'z'; BYTES];
    bytes[BYTES - 1] = 0xfd;
    let blob = fixture.repository.blob(&bytes).unwrap();
    drop(bytes);
    fixture.commit = commit(
        &fixture.repository,
        &[
            ("nested/executable", blob, 0o100_755),
            ("nested/repeated", blob, 0o100_644),
        ],
        &[fixture.commit],
    );
    let prepared = Prepared::from_fixture(fixture);
    let before = snapshot(prepared.destination.path());
    let observed = prepared.observe().unwrap();
    assert_eq!(observed.objects(), 4);
    let scratch = private_fixture();
    let mut reader = PackedGitObjectSource::new(
        scratch.path(),
        observed.objects_directory(),
        PackedSourceLimits::default(),
        || false,
    )
    .unwrap();
    let encoded = codec::encode(prepared.fixture.resolve(vec![], vec![], vec![]).bundle()).unwrap();
    let resolved = resolve_namespaces_from_source(&mut reader, codec::decode(&encoded).unwrap(), || false).unwrap();
    let checkout = prepare_private_checkout(scratch.path(), &resolved, &mut reader, || false).unwrap();
    for (name, executable) in [("executable", true), ("repeated", false)] {
        let mut file = File::open(checkout.path().join("nested").join(name)).unwrap();
        assert_eq!(file.metadata().unwrap().len(), BYTES as u64);
        assert_eq!(file.metadata().unwrap().mode() & 0o100 != 0, executable);
        file.seek(SeekFrom::End(-1)).unwrap();
        let mut tail = [0];
        file.read_exact(&mut tail).unwrap();
        assert_eq!(tail, [0xfd]);
    }
    assert_eq!(snapshot(prepared.destination.path()), before);
}
