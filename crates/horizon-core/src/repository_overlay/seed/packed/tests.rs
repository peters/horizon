use super::super::tests::{commit, private_fixture, snapshot};
use super::*;
use crate::{
    cloud_run::{GitCommitSha, GitSource},
    repository_overlay::{RepositoryOverlayPlan, bundle::RepositoryOverlayBundle, namespace::resolve_namespaces},
};
use git2::{ObjectType, Repository};
use std::{
    cell::Cell,
    fs,
    io::{self, Read},
    os::unix::fs::{PermissionsExt, symlink},
    process::Command,
};

fn git(path: &Path, arguments: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .current_dir(path)
        .args(arguments)
        .env_clear()
        .envs([
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("LC_ALL", "C"),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

fn raw(source: &mut impl GitObjectSource, oid: Oid) -> Vec<u8> {
    let mut stream = source.open(oid).unwrap();
    let mut bytes = Vec::new();
    stream.reader.read_to_end(&mut bytes).unwrap();
    assert_eq!(stream.bytes, bytes.len() as u64);
    assert_eq!(Oid::hash_object(stream.kind, &bytes).unwrap(), oid);
    bytes
}

fn validate(objects: &Path, cancelled: impl Fn() -> bool) -> Result<(), SeedError> {
    let parent = private_fixture();
    view::validate(objects, parent.path(), &cancelled)
}

fn packed(parent: &Path, repository: &Repository) -> PackedGitObjectSource<'static> {
    PackedGitObjectSource::new(
        parent,
        &repository.path().join("objects"),
        PackedSourceLimits::default(),
        || false,
    )
    .unwrap()
}

#[test]
fn real_loose_packed_delta_and_seed_consumer_preserve_exact_objects_and_source() {
    let fixture = private_fixture();
    let path = fixture.path().join("source : quoted\" and \\ path æøå");
    let repository = Repository::init(&path).unwrap();
    let mut bytes = vec![b'a'; 2 * 1024 * 1024];
    let first = repository.blob(&bytes).unwrap();
    bytes[1234] = b'z';
    let second = repository.blob(&bytes).unwrap();
    let base = commit(
        &repository,
        &[("one", first, 0o100_644), ("two", second, 0o100_755)],
        &[],
    );
    let parent = private_fixture();
    {
        let mut source = packed(parent.path(), &repository);
        assert_eq!(raw(&mut source, second), bytes);
    }
    git(&path, &["repack", "-ad", "--window=20", "--depth=20"]);
    let index = fs::read_dir(repository.path().join("objects/pack"))
        .unwrap()
        .map(Result::unwrap)
        .map(|entry| entry.path())
        .find(|p| p.extension().is_some_and(|e| e == "idx"))
        .unwrap();
    let listing = git(&path, &["verify-pack", "-v", index.to_str().unwrap()]);
    assert!(listing.lines().any(|line| line.split_whitespace().count() == 7));
    let before = snapshot(&path);
    let mut source = packed(parent.path(), &repository);
    assert_eq!(raw(&mut source, second), bytes);
    let plan = RepositoryOverlayPlan::new(
        GitSource {
            repository: "synthetic/project".into(),
            commit: GitCommitSha::parse(base.to_string()).unwrap(),
            branch: None,
        },
        vec![],
        vec![],
    )
    .unwrap();
    let resolved = resolve_namespaces(&repository, RepositoryOverlayBundle::new(plan, vec![]).unwrap()).unwrap();
    let seed = super::super::prepare_git_seed(parent.path(), &resolved, &mut source, || false).unwrap();
    let seeded = Repository::open(seed.path()).unwrap();
    assert_eq!(seeded.find_blob(second).unwrap().content(), bytes);
    let metadata = source.metadata_path().to_owned();
    assert!(!format!("{source:?}").contains(metadata.to_str().unwrap()));
    drop(source);
    assert!(metadata.join("HEAD").exists());
    assert_eq!(snapshot(&path), before);
}

#[test]
fn isolated_command_discards_source_configuration_and_missing_promisors() {
    let fixture = private_fixture();
    let repository = Repository::init(fixture.path()).unwrap();
    let oid = repository.blob(b"literal raw object").unwrap();
    let marker = fixture.path().join("helper-ran");
    let helper = fixture.path().join("helper");
    fs::write(&helper, format!("#!/bin/sh\nprintf ran > '{}'\n", marker.display())).unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = repository.config().unwrap();
    config
        .set_str("remote.synthetic.url", &format!("ext::{}", helper.display()))
        .unwrap();
    config.set_bool("remote.synthetic.promisor", true).unwrap();
    config.set_i32("core.repositoryformatversion", 1).unwrap();
    config.set_str("extensions.partialClone", "synthetic").unwrap();
    config.set_str("core.hooksPath", helper.to_str().unwrap()).unwrap();
    config
        .set_str("filter.synthetic.smudge", helper.to_str().unwrap())
        .unwrap();
    config
        .set_str("include.path", fixture.path().join("missing-include").to_str().unwrap())
        .unwrap();
    let objects = repository.path().join("objects");
    fs::write(
        objects.join("pack/multi-pack-index"),
        b"malicious MIDX must not be parsed",
    )
    .unwrap();
    fs::write(
        objects.join("pack/pack-0000000000000000000000000000000000000000.promisor"),
        b"",
    )
    .unwrap();
    let before = snapshot(fixture.path());
    let parent = private_fixture();
    let mut source =
        PackedGitObjectSource::new(parent.path(), &objects, PackedSourceLimits::default(), || false).unwrap();
    assert_eq!(raw(&mut source, oid), b"literal raw object");
    let missing = Oid::hash_object(ObjectType::Blob, b"absent nonzero promisor probe").unwrap();
    assert!(!repository.odb().unwrap().exists(missing));
    assert!(source.open(missing).is_err());
    assert!(source.open(oid).is_err());
    assert!(!marker.exists());
    assert_eq!(snapshot(fixture.path()), before);
    let control = Command::new("/usr/bin/git")
        .current_dir(fixture.path())
        .args([
            "-c",
            "core.multiPackIndex=false",
            "cat-file",
            "-p",
            &missing.to_string(),
        ])
        .env_clear()
        .envs([
            ("GIT_ALLOW_PROTOCOL", "ext"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_TERMINAL_PROMPT", "0"),
        ])
        .output()
        .unwrap();
    assert!(
        !control.status.success() && marker.exists(),
        "local helper positive control did not run: {}",
        String::from_utf8_lossy(&control.stderr)
    );
    let command = view::command(
        &staging::reserve(parent.path(), &|| false).unwrap(),
        &objects,
        PackedSourceLimits::default(),
    )
    .unwrap();
    let environment: std::collections::BTreeMap<_, _> = command.get_envs().collect();
    let arguments: Vec<_> = command.get_args().collect();
    assert!(arguments.contains(&std::ffi::OsStr::new("core.multiPackIndex=false")));
    assert!(arguments.contains(&std::ffi::OsStr::new("core.commitGraph=false")));
    assert_eq!(
        environment.get(std::ffi::OsStr::new("GIT_CONFIG_GLOBAL")),
        Some(&Some(std::ffi::OsStr::new("/dev/null")))
    );
    assert_eq!(
        environment.get(std::ffi::OsStr::new("GIT_ALLOW_PROTOCOL")),
        Some(&Some(std::ffi::OsStr::new("")))
    );
}

#[test]
fn unsafe_alternates_links_special_nodes_ancestry_and_limits_are_rejected() {
    for kind in 0..5 {
        let fixture = private_fixture();
        let objects = fixture.path().join("objects");
        fs::create_dir(&objects).unwrap();
        fs::create_dir(objects.join("info")).unwrap();
        match kind {
            0 => fs::write(objects.join("info/alternates"), b"/unauthorized\n").unwrap(),
            1 => symlink("/dev/null", objects.join("object")).unwrap(),
            2 => rustix::fs::mknodat(
                rustix::fs::CWD,
                objects.join("fifo"),
                rustix::fs::FileType::Fifo,
                rustix::fs::Mode::RUSR,
                0,
            )
            .unwrap(),
            3 => {
                fs::write(objects.join("one"), b"hardlink").unwrap();
                fs::hard_link(objects.join("one"), objects.join("two")).unwrap();
            }
            _ => {
                fs::rename(&objects, fixture.path().join("actual")).unwrap();
                symlink("actual", &objects).unwrap();
            }
        }
        assert_eq!(validate(&objects, || false), Err(SeedError::UnsafeParent));
    }
    for limits in [
        PackedSourceLimits {
            cpu_seconds: 0,
            ..PackedSourceLimits::default()
        },
        PackedSourceLimits {
            address_space_bytes: u64::MAX,
            ..PackedSourceLimits::default()
        },
        PackedSourceLimits {
            object_timeout: Duration::ZERO,
            ..PackedSourceLimits::default()
        },
    ] {
        assert_eq!(limits.validate(), Err(SeedError::Limit));
    }
    let fixture = private_fixture();
    assert_eq!(validate(fixture.path(), || true), Err(SeedError::Cancelled));
    fs::create_dir_all(fixture.path().join("a/b/c/d")).unwrap();
    assert_eq!(validate(fixture.path(), || false), Err(SeedError::Limit));
}

#[test]
fn scratch_inside_source_is_rejected_before_reservation_without_source_changes() {
    let fixture = private_fixture();
    let nested = fixture.path().join("scratch");
    fs::create_dir(&nested).unwrap();
    fs::set_permissions(&nested, fs::Permissions::from_mode(0o700)).unwrap();
    let before = snapshot(fixture.path());
    for parent in [fixture.path(), nested.as_path()] {
        let failure =
            PackedGitObjectSource::new(parent, fixture.path(), PackedSourceLimits::default(), || false).unwrap_err();
        assert_eq!(failure.reason, SeedError::UnsafeParent);
        assert!(failure.residue().is_none());
        assert_eq!(snapshot(fixture.path()), before);
    }
}

fn peer(script: &str, timeout: Duration) -> process::Session<'static> {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", script]).env_clear();
    process::Session::spawn(command, timeout, Box::new(|| false)).unwrap()
}

#[test]
fn strict_headers_accept_only_exact_oid_type_canonical_length_and_field_count() {
    let oid = Oid::ZERO_SHA1;
    for suffix in ["blob 0", "tree 123", "commit 456", "tag 1"] {
        assert!(protocol::Header::parse(format!("{oid} {suffix}").as_bytes(), oid).is_some());
    }
    for suffix in [
        "missing",
        "blob +1",
        "blob -1",
        "blob 01",
        "blob 18446744073709551616",
        "blob 1 extra",
        "blob 1\r",
        "blob ",
        "blob  1",
        "other 1",
    ] {
        assert!(protocol::Header::parse(format!("{oid} {suffix}").as_bytes(), oid).is_none());
    }
    assert!(protocol::Header::parse(format!("{} blob 1", "1".repeat(40)).as_bytes(), oid).is_none());
}

#[test]
fn protocol_rejects_bad_second_header_truncation_delimiter_and_oversized_header() {
    for response in [
        "blob 3\nabc!",
        "blob 3\nab",
        "tree 3\nabc\n",
        "blob 4\nabc\n",
        "blob 3\nabc",
        "blob 3 extra\nabc\n",
    ] {
        let script = format!("read a oid; printf '%s blob 3\\n' \"$oid\"; read a oid; printf '%s {response}' \"$oid\"");
        let mut session = peer(&script, Duration::from_secs(2));
        let header = session.info(Oid::ZERO_SHA1).unwrap();
        let mut reader = protocol::ObjectReader::new(&mut session, header);
        assert!(reader.read_to_end(&mut Vec::new()).is_err());
        drop(reader);
        assert!(session.info(Oid::ZERO_SHA1).is_err());
    }
    let mut session = peer("read a oid; printf '%090d' 0", Duration::from_secs(2));
    assert!(session.info(Oid::ZERO_SHA1).is_err());
}

#[test]
fn delayed_contents_empty_reads_zero_payload_and_early_drop_obey_framing() {
    let fixture = private_fixture();
    let marker = fixture.path().join("contents-requested");
    let script = format!(
        "read a oid; printf '%s blob 0\\n' \"$oid\"; read a oid; printf yes > '{}'; printf '%s blob 0\\n\\n' \"$oid\"",
        marker.display()
    );
    let mut session = peer(&script, Duration::from_secs(2));
    let header = session.info(Oid::ZERO_SHA1).unwrap();
    let mut reader = protocol::ObjectReader::new(&mut session, header);
    assert_eq!(reader.read(&mut []).unwrap(), 0);
    assert!(!marker.exists());
    assert_eq!(reader.read(&mut [0]).unwrap(), 0);
    assert!(marker.exists());
    drop(reader);
    let mut session = peer(
        "read a oid; printf '%s blob 100\\n' \"$oid\"; read a oid",
        Duration::from_secs(2),
    );
    let header = session.info(Oid::ZERO_SHA1).unwrap();
    drop(protocol::ObjectReader::new(&mut session, header));
    assert!(session.info(Oid::ZERO_SHA1).is_err());
}

#[test]
fn stalled_owned_child_times_out_and_midstream_cancellation_poisons_source() {
    let mut session = peer("read a oid; read again", Duration::from_millis(50));
    let process = PathBuf::from(format!("/proc/{}", session.id().unwrap()));
    let started = std::time::Instant::now();
    assert!(session.info(Oid::ZERO_SHA1).is_err());
    assert_eq!(session.read(&mut [0]).unwrap_err().kind(), io::ErrorKind::TimedOut);
    session.poison();
    assert!(!process.exists());
    assert!(started.elapsed() < Duration::from_secs(2));
    let cancelled = Cell::new(false);
    let mut command = Command::new("/bin/sh");
    command.env_clear();
    command.args([
        "-c",
        "read a oid; printf '%s blob 3\\n' \"$oid\"; read a oid; printf '%s blob 3\\nabc\\n' \"$oid\"",
    ]);
    let mut session = process::Session::spawn(command, Duration::from_secs(2), Box::new(|| cancelled.get())).unwrap();
    let header = session.info(Oid::ZERO_SHA1).unwrap();
    let mut reader = protocol::ObjectReader::new(&mut session, header);
    cancelled.set(true);
    assert_eq!(
        reader.read(&mut [0]).unwrap_err().kind(),
        io::ErrorKind::ConnectionAborted
    );
    drop(reader);
    assert!(session.info(Oid::ZERO_SHA1).is_err());
    let calls = Cell::new(0);
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "read a oid; read again"]).env_clear();
    let mut session = process::Session::spawn(
        command,
        Duration::from_secs(2),
        Box::new(|| {
            calls.set(calls.get() + 1);
            calls.get() >= 3
        }),
    )
    .unwrap();
    assert_eq!(session.info(Oid::ZERO_SHA1), Err(SeedError::Cancelled));
    assert_eq!(
        calls.get(),
        3,
        "cancellation must occur inside header I/O, not its entry preflight"
    );
}

#[test]
fn native_resource_limits_apply_before_git_and_limit_termination_is_reported() {
    let fixture = private_fixture();
    let repository = Repository::init(fixture.path()).unwrap();
    let oid = repository.blob(b"limits probe").unwrap();
    let parent = private_fixture();
    let limits = PackedSourceLimits::default();
    let mut source =
        PackedGitObjectSource::new(parent.path(), &repository.path().join("objects"), limits, || false).unwrap();
    raw(&mut source, oid);
    let process = PathBuf::from(format!("/proc/{}", source.session.id().unwrap()));
    let actual = fs::read_to_string(process.join("limits")).unwrap();
    for (label, value) in [
        ("Max address space", limits.address_space_bytes),
        ("Max cpu time", u64::from(limits.cpu_seconds)),
        ("Max open files", 64),
        ("Max core file size", 0),
    ] {
        let fields: Vec<_> = actual
            .lines()
            .find_map(|line| line.strip_prefix(label))
            .unwrap()
            .split_whitespace()
            .collect();
        assert_eq!(fields[..2], [value.to_string(), value.to_string()]);
    }
    drop(source);
    assert!(!process.exists());
    let mut command = Command::new("/usr/bin/prlimit");
    command
        .args(["--cpu=1:1", "--core=0:0", "--", "/bin/sh", "-c", "while :; do :; done"])
        .env_clear();
    let mut session = process::Session::spawn(command, Duration::from_secs(5), Box::new(|| false)).unwrap();
    let process = PathBuf::from(format!("/proc/{}", session.id().unwrap()));
    let started = std::time::Instant::now();
    assert!(session.info(Oid::ZERO_SHA1).is_err());
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "pipe timeout is not proof of CPU-limit termination"
    );
    session.poison();
    assert!(!process.exists());
}

#[test]
fn casefold_alternate_alias_is_rejected_when_fixture_capability_is_available() {
    let fixture = private_fixture();
    let enabled = Command::new("/usr/bin/chattr").arg("+F").arg(fixture.path()).output();
    if !enabled.is_ok_and(|output| output.status.success()) {
        eprintln!("SKIP casefold fixture: per-directory casefold is unavailable");
        return;
    }
    fs::create_dir(fixture.path().join("INFO")).unwrap();
    fs::write(fixture.path().join("INFO/ALTERNATES"), b"/unauthorized\n").unwrap();
    assert!(fixture.path().join("info/alternates").exists());
    assert_eq!(validate(fixture.path(), || false), Err(SeedError::UnsafeParent));
}

#[test]
fn actual_source_enumeration_charges_node_and_path_limits_before_growth() {
    for (entries, width) in [(view::MAX_NODES + 1, 0), (view::MAX_PATH_BYTES / 200 + 1, 200)] {
        let fixture = private_fixture();
        for number in 0..entries {
            fs::File::create(fixture.path().join(format!("{number:0width$}"))).unwrap();
        }
        assert_eq!(validate(fixture.path(), || false), Err(SeedError::Limit));
    }
}
