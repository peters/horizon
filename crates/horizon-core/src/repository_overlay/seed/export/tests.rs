use super::super::{
    prepare_git_seed,
    tests::{Fixture, commit, private_fixture, snapshot},
};
use super::*;
use crate::repository_overlay::{OverlayChange, OverlayContent, bundle::VerifiedOverlayBlob};
use git2::Repository;
use std::{
    cell::Cell,
    collections::BTreeSet,
    fs::{self, File},
    io::{Cursor, Read},
    os::unix::fs::{MetadataExt, symlink},
    process::{Command, Stdio},
    time::Duration,
};

fn seed(fixture: &Fixture, parent: &Path) -> PreparedGitSeed {
    let blob = VerifiedOverlayBlob::new(b"staged-only overlay bytes".to_vec()).unwrap();
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
    let resolved = fixture.resolve(vec![added], vec![removed], vec![blob]);
    prepare_git_seed(parent, &resolved, &mut fixture.source(), || false).unwrap()
}

fn unpack(pack: &PreparedGitPack, parent: &Path) -> Repository {
    let repository = Repository::init(parent).unwrap();
    fs::write(repository.path().join("shallow"), format!("{}\n", pack.base_commit())).unwrap();
    let result = Command::new("/usr/bin/git")
        .arg("--git-dir")
        .arg(repository.path())
        .args(["index-pack", "--stdin", "--strict", "--threads=1"])
        .env_clear()
        .envs([
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_SYSTEM", "/dev/null"),
            ("GIT_ALLOW_PROTOCOL", ""),
            ("LC_ALL", "C"),
        ])
        .stdin(Stdio::from(File::open(pack.path()).unwrap()))
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    repository
}

#[test]
fn actual_seed_exports_exact_base_not_overlay_history_or_seed_configuration() {
    let fixture = Fixture::new();
    let parent = private_fixture();
    let seed = seed(&fixture, parent.path());
    let seeded = Repository::open(seed.path()).unwrap();
    let ancestor = fixture.repository.find_commit(fixture.ancestor).unwrap();
    for oid in [fixture.ancestor, ancestor.tree_id(), fixture.private_blob] {
        let database = fixture.repository.odb().unwrap();
        let raw = database.read(oid).unwrap();
        assert_eq!(seeded.odb().unwrap().write(raw.kind(), raw.data()).unwrap(), oid);
    }
    // The isolated view must ignore even changed private seed metadata.
    fs::write(seeded.path().join("HEAD"), format!("{}\n", fixture.ancestor)).unwrap();
    fs::write(seeded.path().join("shallow"), "").unwrap();
    fs::write(
        seeded.path().join("config"),
        "[include]\npath = /not-an-authorized-config\n",
    )
    .unwrap();
    let before = snapshot(seed.path());
    let original = snapshot(fixture.directory.path());
    let pack = prepare_git_base_pack(parent.path(), &seed, PackExportLimits::default(), || false).unwrap();
    assert_eq!(snapshot(seed.path()), before);
    assert_eq!(snapshot(fixture.directory.path()), original);
    assert_eq!(pack.base_commit(), fixture.commit);
    let metadata = fs::metadata(pack.path()).unwrap();
    assert_eq!(metadata.mode() & 0o7777, 0o600);
    assert_eq!(metadata.nlink(), 1);
    let bytes = fs::read(pack.path()).unwrap();
    assert_eq!(pack.encoded_bytes(), bytes.len() as u64);
    assert_eq!(pack.sha256(), &ArtifactDigest::sha256(&bytes));
    let target = private_fixture();
    let receiver = unpack(&pack, target.path());
    let base = fixture.repository.find_commit(fixture.commit).unwrap();
    let expected = BTreeSet::from([fixture.commit, base.tree_id(), fixture.base_blob]);
    let mut actual = BTreeSet::new();
    receiver
        .odb()
        .unwrap()
        .foreach(|oid| {
            actual.insert(*oid);
            true
        })
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        receiver.find_blob(fixture.base_blob).unwrap().content(),
        b"base\0literal\xff"
    );
    let root = receiver.find_tree(base.tree_id()).unwrap();
    assert!(root.get_name("removed").is_some());
    assert!(root.get_name("staged-only").is_none());
    let retained = pack.path().to_path_buf();
    assert!(!format!("{pack:?}").contains(retained.to_str().unwrap()));
    drop(pack);
    assert!(retained.exists());
}

#[test]
fn invalid_bounds_cancellation_and_overlapping_or_unsafe_roots_do_not_mutate_seed() {
    let fixture = Fixture::new();
    let parent = private_fixture();
    let seed = seed(&fixture, parent.path());
    let before = snapshot(parent.path());
    for encoded_bytes in [0, 31, PackExportLimits::MAX_ENCODED_BYTES + 1] {
        let limits = PackExportLimits {
            encoded_bytes,
            ..PackExportLimits::default()
        };
        let failure = prepare_git_base_pack(parent.path(), &seed, limits, || false).unwrap_err();
        assert_eq!(failure.reason, SeedError::Limit);
        assert!(failure.residue().is_none());
    }
    let failure = prepare_git_base_pack(parent.path(), &seed, PackExportLimits::default(), || true).unwrap_err();
    assert_eq!(failure.reason, SeedError::Cancelled);
    assert!(failure.residue().is_none());
    for overlap in [
        seed.path().to_path_buf(),
        seed.path().join(".git"),
        seed.path().join(".git/objects"),
    ] {
        let failure = prepare_git_base_pack(&overlap, &seed, PackExportLimits::default(), || false).unwrap_err();
        assert_eq!(failure.reason, SeedError::UnsafeParent);
        assert!(failure.residue().is_none());
    }
    assert_eq!(snapshot(parent.path()), before);
    let alias = parent.path().join("alias");
    symlink(seed.path(), &alias).unwrap();
    let failure = prepare_git_base_pack(&alias, &seed, PackExportLimits::default(), || false).unwrap_err();
    assert_eq!(failure.reason, SeedError::UnsafeParent);
    assert!(failure.residue().is_none());
}

#[test]
fn large_removed_base_blob_streams_through_real_seed_pack_and_strict_receiver() {
    let mut fixture = Fixture::new();
    let mut bytes = vec![b'a'; 65 * 1024 * 1024 + 1];
    *bytes.last_mut().unwrap() = b'z';
    let object = fixture.repository.blob(&bytes).unwrap();
    fixture.commit = commit(
        &fixture.repository,
        &[
            ("kept", object, 0o100_644),
            ("nested/leaf", object, 0o100_644),
            ("removed", object, 0o100_755),
        ],
        &[fixture.commit],
    );
    fixture.base_blob = object;
    let parent = private_fixture();
    let seed = seed(&fixture, parent.path());
    let pack = prepare_git_base_pack(parent.path(), &seed, PackExportLimits::default(), || false).unwrap();
    assert!(pack.encoded_bytes() > 32 * 1024);
    let target = private_fixture();
    let receiver = unpack(&pack, target.path());
    assert_eq!(receiver.find_blob(object).unwrap().content(), bytes);
    let tree = receiver.find_commit(fixture.commit).unwrap().tree().unwrap();
    assert_eq!(tree.get_name("removed").unwrap().filemode(), 0o100_755);
    assert_eq!(tree.get_path(Path::new("nested/leaf")).unwrap().id(), object);
}

#[test]
fn initial_commits_and_empty_base_trees_have_complete_shallow_packs() {
    for empty in [false, true] {
        let mut fixture = Fixture::new();
        fixture.commit = if empty {
            commit(&fixture.repository, &[], &[fixture.commit])
        } else {
            fixture.ancestor
        };
        let resolved = fixture.resolve(vec![], vec![], vec![]);
        let parent = private_fixture();
        let seed = prepare_git_seed(parent.path(), &resolved, &mut fixture.source(), || false).unwrap();
        let pack = prepare_git_base_pack(parent.path(), &seed, PackExportLimits::default(), || false).unwrap();
        let target = private_fixture();
        let receiver = unpack(&pack, target.path());
        let tree = receiver.find_commit(fixture.commit).unwrap().tree().unwrap();
        assert_eq!(tree.is_empty(), empty);
        let mut count = 0;
        receiver
            .odb()
            .unwrap()
            .foreach(|_| {
                count += 1;
                true
            })
            .unwrap();
        assert_eq!(count, if empty { 2 } else { 3 });
    }
}

#[test]
fn pack_command_uses_exact_private_boundary_and_single_resource_limited_process() {
    let parent = private_fixture();
    let metadata = staging::reserve(parent.path(), &|| false).unwrap();
    let source = parent.path().join("not-read-source");
    let limits = PackedSourceLimits::default();
    let command = view::command(&metadata, &source, limits, view::Operation::Pack(Oid::ZERO_SHA1)).unwrap();
    let arguments: Vec<_> = command.get_args().map(|arg| arg.to_str().unwrap()).collect();
    assert_eq!(command.get_program(), "/usr/bin/prlimit");
    for option in [
        "--stdout",
        "--revs",
        "--no-reuse-delta",
        "--no-reuse-object",
        "--window=0",
        "--threads=1",
    ] {
        assert!(arguments.contains(&option));
    }
    for forbidden in ["--all", "--thin", "--include-tag", "cat-file", "--batch-command"] {
        assert!(!arguments.contains(&forbidden));
    }
    assert!(arguments.contains(&format!("--as={0}:{0}", limits.address_space_bytes).as_str()));
    assert!(arguments.contains(&format!("--cpu={0}:{0}", limits.cpu_seconds).as_str()));
    assert_eq!(
        fs::read_to_string(metadata.join("shallow")).unwrap(),
        format!("{}\n", Oid::ZERO_SHA1)
    );
    let environment: std::collections::BTreeMap<_, _> = command.get_envs().collect();
    for key in ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM"] {
        assert_eq!(
            environment.get(std::ffi::OsStr::new(key)),
            Some(&Some(std::ffi::OsStr::new("/dev/null")))
        );
    }
    assert_eq!(
        environment.get(std::ffi::OsStr::new("GIT_ALLOW_PROTOCOL")),
        Some(&Some(std::ffi::OsStr::new("")))
    );
    assert_eq!(command.get_current_dir(), Some(metadata.as_path()));
}

#[test]
fn oversized_pack_retains_unconfirmed_private_artifact_and_source() {
    let fixture = Fixture::new();
    let parent = private_fixture();
    let seed = seed(&fixture, parent.path());
    let before = snapshot(seed.path());
    let failure = prepare_git_base_pack(
        parent.path(),
        &seed,
        PackExportLimits {
            encoded_bytes: 32,
            ..PackExportLimits::default()
        },
        || false,
    )
    .unwrap_err();
    assert_eq!(failure.reason, SeedError::Limit);
    let residue = failure.residue().unwrap().to_path_buf();
    assert!(fs::metadata(residue.join("base.pack")).unwrap().len() <= 32);
    assert!(!format!("{failure:?}").contains(residue.to_str().unwrap()));
    drop(failure);
    assert!(residue.exists());
    assert_eq!(snapshot(seed.path()), before);
}

fn synthetic_pack(size: usize) -> Vec<u8> {
    let mut bytes = b"PACK\0\0\0\x02\0\0\0\x01".to_vec();
    bytes.resize(size, 0);
    bytes
}

#[test]
fn output_bounds_framing_short_writes_and_read_failures_are_explicit() {
    struct ShortWrites(Vec<u8>);
    impl Write for ShortWrites {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let count = bytes.len().min(3);
            self.0.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    for size in [32, 65_537] {
        let bytes = synthetic_pack(size);
        let mut input = Cursor::new(&bytes);
        let mut output = ShortWrites(Vec::new());
        let result = output::copy(&mut |bytes| input.read(bytes), &mut output, size as u64, 1).unwrap();
        assert_eq!(output.0, bytes);
        assert_eq!(result.sha256, ArtifactDigest::sha256(&bytes));
        assert_eq!(result.bytes, size as u64);
    }
    for mut bytes in [vec![], synthetic_pack(11), synthetic_pack(31), synthetic_pack(33)] {
        let mut input = Cursor::new(&mut bytes);
        let mut output = Vec::new();
        assert!(output::copy(&mut |bytes| input.read(bytes), &mut output, 32, 1).is_err());
        assert!(output.len() <= 32);
    }
    for offset in [0, 7, 11] {
        let mut bytes = synthetic_pack(32);
        bytes[offset] = 9;
        let mut input = Cursor::new(bytes);
        let mut output = Vec::new();
        assert_eq!(
            output::copy(&mut |bytes| input.read(bytes), &mut output, 32, 1).err(),
            Some(SeedError::Object)
        );
        assert!(output.is_empty());
    }
    let mut input = |_bytes: &mut [u8]| Err(io::Error::from(io::ErrorKind::ConnectionAborted));
    assert_eq!(
        output::copy(&mut input, &mut Vec::new(), 32, 1).err(),
        Some(SeedError::Cancelled)
    );
    let mut input = Cursor::new(synthetic_pack(32));
    assert_eq!(
        output::copy(
            &mut |bytes| input.read(bytes),
            &mut Cursor::new(&mut [0u8; 0][..]),
            32,
            1
        )
        .err(),
        Some(SeedError::Storage)
    );
}

#[test]
fn one_shot_session_closes_input_checks_exit_deadline_and_cancellation() {
    fn peer(script: &str, timeout: Duration) -> Session<'static> {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]).env_clear();
        Session::spawn(command, timeout, Box::new(|| false)).unwrap()
    }
    for exit in [0, 7] {
        let mut session = peer(
            &format!("read oid; if read extra; then exit 9; fi; printf done; exit {exit}"),
            Duration::from_secs(2),
        );
        session.begin_pack(Oid::ZERO_SHA1).unwrap();
        let mut bytes = [0; 10];
        assert_eq!(session.read(&mut bytes).unwrap(), 4);
        assert_eq!(session.read(&mut bytes).unwrap(), 0);
        assert_eq!(session.finish().is_ok(), exit == 0);
    }
    let mut stalled = peer("read oid; while :; do :; done", Duration::from_millis(50));
    let stalled_process = PathBuf::from(format!("/proc/{}", stalled.id().unwrap()));
    stalled.begin_pack(Oid::ZERO_SHA1).unwrap();
    assert!(stalled.read(&mut [0]).is_err());
    drop(stalled);
    assert!(!stalled_process.exists());
    let cancelled = Cell::new(false);
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "read oid; while :; do :; done"]).env_clear();
    let mut session = Session::spawn(command, Duration::from_secs(2), Box::new(|| cancelled.get())).unwrap();
    let cancelled_process = PathBuf::from(format!("/proc/{}", session.id().unwrap()));
    session.begin_pack(Oid::ZERO_SHA1).unwrap();
    cancelled.set(true);
    assert_eq!(
        session.read(&mut [0]).unwrap_err().kind(),
        io::ErrorKind::ConnectionAborted
    );
    drop(session);
    assert!(!cancelled_process.exists());
}
