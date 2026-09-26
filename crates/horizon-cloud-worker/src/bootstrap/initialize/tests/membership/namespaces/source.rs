use super::*;
use crate::bootstrap::source::{self, Boundary as SourceBoundary};
use horizon_cloud_protocol::membership::{Artifact, Source};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn git(path: &std::path::Path, args: &[&str]) -> Vec<u8> {
    let result = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    result.stdout
}
fn bytes() -> (Source, Vec<u8>) {
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init", "-q"]);
    git(temp.path(), &["config", "user.name", "Fixture"]);
    git(temp.path(), &["config", "user.email", "fixture@example.invalid"]);
    fs::write(temp.path().join("committed"), "committed source").unwrap();
    git(temp.path(), &["add", "committed"]);
    git(temp.path(), &["commit", "-qm", "Initial source"]);
    let revision = String::from_utf8(git(temp.path(), &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    let mut child = Command::new("git")
        .arg("-C")
        .arg(temp.path())
        .args(["pack-objects", "--stdout", "--revs"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "{revision}").unwrap();
    let pack = child.wait_with_output().unwrap();
    assert!(pack.status.success());
    fs::create_dir(temp.path().join("material")).unwrap();
    fs::create_dir(temp.path().join("material/lfs")).unwrap();
    fs::write(
        temp.path().join("material/manifest.json"),
        br#"{"modules":[],"assets":[]}"#,
    )
    .unwrap();
    let material = Command::new("tar")
        .arg("-cf")
        .arg("-")
        .arg("-C")
        .arg(temp.path().join("material"))
        .arg(".")
        .output()
        .unwrap();
    assert!(material.status.success());
    let descriptor = Source {
        version: 1,
        revision,
        pack: artifact(&pack.stdout),
        material: artifact(&material.stdout),
    };
    let mut bytes = pack.stdout;
    bytes.extend(material.stdout);
    (descriptor, bytes)
}
fn artifact(bytes: &[u8]) -> Artifact {
    Artifact {
        length: bytes.len() as u64,
        sha256: Sha256::digest(bytes).into(),
    }
}
fn request(f: &Fixture, project: &ProjectIdentity, descriptor: Source) -> RecoveryRequest {
    f.membership(
        project,
        f.manifest().revision,
        OperationId::generate(),
        &Request::ImportSource { descriptor },
    )
}
fn import(
    f: &Fixture,
    request: &RecoveryRequest,
    bytes: &[u8],
    checkpoint: &mut impl FnMut(SourceBoundary) -> io::Result<()>,
) -> io::Result<Receipt> {
    let store = Store::open(&f.root())?;
    let deadline = std::time::Instant::now() + Source::WORKER_TIMEOUT;
    let receipt = source::prepare(&store, &f.runtime, request, deadline, |_| Ok(()))?;
    source::import(
        &store,
        &f.runtime,
        request,
        &receipt,
        &mut &bytes[..],
        deadline,
        checkpoint,
    )?;
    Ok(receipt)
}
fn prepared(f: &Fixture, name: &str, port: u16) -> ProjectIdentity {
    let project = reserved(f, name, port);
    f.change(&prepare(f, &project)).unwrap();
    project
}

#[test]
fn one_source_deadline_covers_preparation_transfer_and_publication() {
    let (descriptor, bytes) = bytes();
    let f = Fixture::ready();
    let project = prepared(&f, "deadline", 8000);
    let request = request(&f, &project, descriptor);
    let store = Store::open(&f.root()).unwrap();
    let expired = std::time::Instant::now();
    assert_eq!(
        source::prepare(&store, &f.runtime, &request, expired, |_| panic!(
            "expired before probe"
        ))
        .unwrap_err()
        .kind(),
        io::ErrorKind::TimedOut
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let receipt = source::prepare(&store, &f.runtime, &request, deadline, |_| Ok(())).unwrap();
    let mut received = false;
    let result = source::import(
        &store,
        &f.runtime,
        &request,
        &receipt,
        &mut &bytes[..],
        deadline,
        &mut |at| {
            if at == SourceBoundary::Received {
                received = true;
                std::thread::sleep(deadline.saturating_duration_since(std::time::Instant::now()));
            }
            Ok(())
        },
    );
    assert!(received);
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert!(!project_root(&f, &project).join("repository/source").exists());
    drop(store);
    import(&f, &request, &bytes, &mut |_| Ok(())).unwrap();
}

#[test]
fn source_recovers_stream_and_publication_without_resetting_data() {
    let (descriptor, bytes) = bytes();
    for failure in [
        None,
        Some(SourceBoundary::Anchored),
        Some(SourceBoundary::Received),
        Some(SourceBoundary::Imported),
        Some(SourceBoundary::Published),
        Some(SourceBoundary::Synced),
    ] {
        let f = Fixture::ready();
        let project = prepared(&f, "source", 8000);
        let preparation = f
            .manifest()
            .operations
            .iter()
            .find(|entry| entry.receipt.identity == project && entry.receipt.state == State::Preparing)
            .unwrap()
            .clone();
        let request = request(&f, &project, descriptor.clone());
        let first = import(&f, &request, &bytes, &mut |at| {
            if Some(at) == failure { Err(invalid()) } else { Ok(()) }
        });
        assert_eq!(first.is_ok(), failure.is_none());
        if failure.is_some() {
            assert!(f.change(&cancel(&f, &project)).is_err());
        }
        boot(&f).unwrap();
        let receipt = import(&f, &request, &bytes, &mut |_| Ok(())).unwrap();
        assert_eq!(receipt.state, State::Importing);
        assert_eq!(
            f.change(&RecoveryRequest {
                message: preparation.message,
                payload: preparation.payload
            })
            .unwrap(),
            preparation.receipt
        );
        let root = project_root(&f, &project).join("repository/source");
        let inode = fs::metadata(&root).unwrap().ino();
        assert_eq!(
            git(&root.join("repository.git"), &["show", "HEAD:committed"]),
            b"committed source"
        );
        assert_eq!(receipt, import(&f, &request, &bytes, &mut |_| Ok(())).unwrap());
        assert_eq!(fs::metadata(&root).unwrap().ino(), inode);
        f.change(&cancel(&f, &project)).unwrap();
        assert!(import(&f, &request, &bytes, &mut |_| Ok(())).is_err());
        boot(&f).unwrap();
        assert_eq!(fs::metadata(root).unwrap().ino(), inode);
    }
    let f = Fixture::ready();
    let project = prepared(&f, "source", 8000);
    let request = request(&f, &project, descriptor);
    assert!(import(&f, &request, &bytes[..31], &mut |_| Ok(())).is_err());
    boot(&f).unwrap();
    import(&f, &request, &bytes, &mut |_| Ok(())).unwrap();
}

#[test]
fn source_rejects_replacement_dirty_publication_changed_bytes_and_late_cancellation() {
    let (descriptor, bytes) = bytes();
    for replacement in ["parent", "source", "staging", "dirty", "bytes", "signature", "revision"] {
        let f = Fixture::ready();
        let project = prepared(&f, "source", 8000);
        let request = request(&f, &project, descriptor.clone());
        let receipt = import(&f, &request, &bytes, &mut |at| {
            if at == SourceBoundary::Received {
                Err(invalid())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(receipt.kind(), io::ErrorKind::Other);
        let record: serde_json::Value =
            serde_json::from_slice(&fs::read(f.root().join(format!("source-{}.json", project.project_id()))).unwrap())
                .unwrap();
        let staging = f.root().join(format!(
            ".source-{}.next",
            record["receipt"]["operation"].as_str().unwrap()
        ));
        let parent = project_root(&f, &project).join("repository");
        let mut changed = request;
        let mut transfer = bytes.clone();
        match replacement {
            "parent" => {
                fs::rename(&parent, parent.with_extension("old")).unwrap();
                fs::create_dir(&parent).unwrap();
                fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
            }
            "source" => {
                fs::create_dir(parent.join("source")).unwrap();
            }
            "staging" => {
                fs::rename(&staging, staging.with_extension("old")).unwrap();
                fs::create_dir(&staging).unwrap();
            }
            "dirty" => {
                import(&f, &changed, &bytes, &mut |_| Ok(())).unwrap();
                fs::write(parent.join("source/repository.git/HEAD"), "dirty").unwrap();
            }
            "bytes" => {
                transfer[0] ^= 1;
            }
            "signature" => {
                changed.payload.push(' ');
            }
            _ => {
                let mut modified = descriptor.clone();
                modified.revision = "0".repeat(40);
                changed = self::request(&f, &project, modified);
            }
        }
        assert!(
            import(&f, &changed, &transfer, &mut |_| Ok(())).is_err(),
            "{replacement}"
        );
        assert!(f.change(&cancel(&f, &project)).is_err(), "{replacement}");
    }
}

#[test]
fn preparation_rechecks_capabilities_before_each_upload_and_never_releases_pending_source() {
    let (descriptor, bytes) = bytes();
    let f = Fixture::ready();
    let project = prepared(&f, "source", 8000);
    let request = request(&f, &project, descriptor);
    for prior in [false, true] {
        if prior {
            import(&f, &request, &bytes, &mut |_| Ok(())).unwrap();
        }
        let store = Store::open(&f.root()).unwrap();
        assert!(
            source::prepare(
                &store,
                &f.runtime,
                &request,
                std::time::Instant::now() + Source::WORKER_TIMEOUT,
                |_| Err(invalid())
            )
            .is_err()
        );
    }
}

#[test]
fn auxiliary_archive_rejects_sparse_expansion_links_duplicate_and_unsafe_members_before_writes() {
    let temp = tempfile::tempdir().unwrap();
    let helper = include_str!("../../../../source/import.py");
    let definitions = helper.split("if sys.argv[1] ==").next().unwrap();
    let test = r#"
import io
for case in ('sparse', 'aggregate', 'link', 'duplicate', 'traversal', 'unexpected'):
    MAX_MATERIAL_BYTES = 1024 if case == 'aggregate' else 4 * 1024**3
    with tarfile.open('material.tar', 'w', format=tarfile.GNU_FORMAT) as output:
        def add(name, content=b'', kind=tarfile.REGTYPE):
            entry = tarfile.TarInfo(name)
            entry.type = kind
            entry.size = len(content)
            output.addfile(entry, io.BytesIO(content))
        add('.', kind=tarfile.DIRTYPE)
        add('lfs', kind=tarfile.DIRTYPE)
        modules = [{"path": "child", "revision": "a" * 40}] if case in ('sparse', 'aggregate') else []
        if case == 'aggregate': modules.append({"path": "other", "revision": "b" * 40})
        add('manifest.json', json.dumps({"modules": modules, "assets": []}).encode())
        if case == 'sparse': add('module-0.pack', bytes(1024), tarfile.GNUTYPE_SPARSE)
        if case == 'aggregate':
            add('module-0.pack', bytes(800))
            add('module-1.pack', bytes(800))
        if case == 'link': add('link', kind=tarfile.SYMTYPE)
        if case == 'duplicate': add('manifest.json', b'{}')
        if case == 'traversal': add('../outside', b'unsafe')
        if case == 'unexpected': add('extra', b'unexpected')
    if case == 'sparse':
        with tarfile.open('material.tar') as check:
            assert check.getmember('module-0.pack').issparse()
    try:
        archive()
    except (ValueError, tarfile.TarError):
        pass
    else:
        raise AssertionError(case)
    assert not Path('material').exists()
"#;
    let output = Command::new("python3")
        .args(["-I", "-c", &format!("{definitions}\n{test}")])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn source_git_output_is_bounded_during_execution_and_prefix_children_are_reaped() {
    let temp = tempfile::tempdir().unwrap();
    let definitions = include_str!("../../../../source/import.py")
        .split("if sys.argv[1] ==")
        .next()
        .unwrap();
    let test = r#"
repo = Path('repository.git')
subprocess.run(['/usr/bin/git', 'init', '--bare', '-q', str(repo)], check=True)
os.environ.update(GIT_AUTHOR_NAME='Fixture', GIT_AUTHOR_EMAIL='fixture@example.invalid',
                  GIT_COMMITTER_NAME='Fixture', GIT_COMMITTER_EMAIL='fixture@example.invalid')
original_run = subprocess.run
def observed_run(*args, **kwargs):
    try:
        return original_run(*args, **kwargs)
    finally:
        output = kwargs.get('stdout')
        if hasattr(output, 'fileno'):
            assert os.fstat(output.fileno()).st_size <= MAX_GIT_OUTPUT + 1
subprocess.run = observed_run
def blob(content):
    return git(repo, ['hash-object', '-w', '--stdin'], data=content).strip().decode()
attributes = blob(b'value filter=lfs\n')
pointer = b'version https://git-lfs.github.com/spec/v1\noid sha256:' + b'a' * 64 + b'\nsize 3\n'
for content, assets, valid in [(b'A' * (20 * 1024**2), [], True),
                              (pointer + b'A' * (20 * 1024**2), [], False),
                              (pointer, [{'path': 'value', 'oid': 'a' * 64, 'size': 3}], True)]:
    oid = blob(content)
    assert git(repo, ['cat-file', 'blob', oid], prefix=1025) == content[:1025]
    if len(content) > MAX_GIT_OUTPUT:
        try:
            git(repo, ['cat-file', 'blob', oid])
        except (ValueError, subprocess.CalledProcessError):
            pass
        else:
            raise AssertionError('full output exceeded limit')
    tree = git(repo, ['mktree'], data=f'100644 blob {attributes}\t.gitattributes\n100644 blob {oid}\tvalue\n'.encode()).strip().decode()
    revision = git(repo, ['commit-tree', tree], data=b'Fixture\n').strip().decode()
    try:
        selected_material({'modules': [], 'assets': assets}, revision)
    except ValueError:
        assert not valid
    else:
        assert valid
try:
    git(repo, ['cat-file', 'blob', '0' * 40], prefix=1025)
except ValueError:
    pass
else:
    raise AssertionError('failed command accepted as a prefix')
for output in ('', "os.write(1, b'x' * 4096)"):
    GIT_TIMEOUT = 1
    script = "import os,time; open('child.pid','w').write(str(os.getpid())); " + (output or 'pass') + "; time.sleep(5)"
    try:
        result = git_prefix([sys.executable, '-I', '-c', script], dict(os.environ), 1025)
    except ValueError:
        assert not output
    else:
        assert output and result == b'x' * 1025
    try:
        os.kill(int(Path('child.pid').read_text()), 0)
    except ProcessLookupError:
        pass
    else:
        raise AssertionError('prefix child survived completion')
"#;
    let output = Command::new("python3")
        .args(["-I", "-c", &format!("{definitions}\n{test}")])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn source_helper_completes_fixed_file_prefixes_but_fences_unknown_git_locks() {
    let (descriptor, bytes) = bytes();
    for lock in [false, true] {
        let f = Fixture::ready();
        let project = prepared(&f, "source", 8000);
        let request = request(&f, &project, descriptor.clone());
        assert!(
            import(&f, &request, &bytes, &mut |at| if at == SourceBoundary::Received {
                Err(invalid())
            } else {
                Ok(())
            })
            .is_err()
        );
        let operation = f.manifest().operations.last().unwrap().receipt.operation;
        let staging = f.root().join(format!(".source-{operation}.next/repository.git"));
        fs::create_dir(&staging).unwrap();
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(staging.join(if lock { "index.lock" } else { "config" }), b"").unwrap();
        fs::set_permissions(
            staging.join(if lock { "index.lock" } else { "config" }),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert_eq!(import(&f, &request, &bytes, &mut |_| Ok(())).is_err(), lock);
        if lock {
            assert!(f.change(&cancel(&f, &project)).is_err());
        }
    }
}

#[test]
fn fifo_substitution_before_append_open_fails_without_waiting_for_a_peer() {
    let (descriptor, bytes) = bytes();
    let f = Fixture::ready();
    let project = prepared(&f, "source", 8000);
    let request = request(&f, &project, descriptor);
    let mut preserved = None;
    let start = std::time::Instant::now();
    assert!(
        import(&f, &request, &bytes, &mut |at| {
            if at == SourceBoundary::Opened && preserved.is_none() {
                let operation = f.manifest().operations.last().unwrap().receipt.operation;
                let path = f.root().join(format!(".source-{operation}.next/pack"));
                let original = path.with_extension("preserved");
                fs::rename(&path, &original)?;
                assert!(Command::new("mkfifo").arg(&path).status()?.success());
                preserved = Some(original);
            }
            Ok(())
        })
        .is_err()
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    assert!(fs::read(preserved.unwrap()).unwrap().is_empty());
}

#[test]
fn invalid_pack_commit_or_unreferenced_material_never_publishes_source() {
    let (descriptor, original) = bytes();
    for case in ["pack", "commit", "module", "asset"] {
        let mut descriptor = descriptor.clone();
        let mut bytes = original.clone();
        let pack_length = usize::try_from(descriptor.pack.length).unwrap();
        match case {
            "pack" => {
                bytes[0] ^= 1;
                descriptor.pack = artifact(&bytes[..pack_length]);
            }
            "commit" => {
                descriptor.revision = "0".repeat(40);
            }
            _ => {
                let temp = tempfile::tempdir().unwrap();
                fs::create_dir(temp.path().join("lfs")).unwrap();
                let manifest = if case == "module" {
                    fs::write(temp.path().join("module-0.pack"), &bytes[..pack_length]).unwrap();
                    serde_json::json!({"modules":[{"path":"unreferenced","revision":descriptor.revision}],"assets":[]})
                } else {
                    let mut oid = String::new();
                    for byte in Sha256::digest(b"unreferenced") {
                        use std::fmt::Write;
                        write!(oid, "{byte:02x}").unwrap();
                    }
                    fs::write(temp.path().join("lfs").join(&oid), b"unreferenced").unwrap();
                    serde_json::json!({"modules":[],"assets":[{"path":"unreferenced","oid":oid,"size":12}]})
                };
                fs::write(
                    temp.path().join("manifest.json"),
                    serde_json::to_vec(&manifest).unwrap(),
                )
                .unwrap();
                let tar = Command::new("tar")
                    .args(["-cf", "-", "-C"])
                    .arg(temp.path())
                    .arg(".")
                    .output()
                    .unwrap();
                assert!(tar.status.success());
                descriptor.material = artifact(&tar.stdout);
                bytes.truncate(pack_length);
                bytes.extend(tar.stdout);
            }
        }
        let f = Fixture::ready();
        let project = prepared(&f, "source", 8000);
        let request = request(&f, &project, descriptor);
        assert!(import(&f, &request, &bytes, &mut |_| Ok(())).is_err(), "{case}");
        assert!(!project_root(&f, &project).join("repository/source").exists());
        assert!(f.change(&cancel(&f, &project)).is_err());
    }
}

fn lease_child(root: &std::path::Path) {
    let store = Store::open(root).unwrap();
    let directory = root.parent().unwrap();
    let script = r"
import json, os, subprocess, sys
lease = os.dup(0)
with open(sys.argv[1], 'rb') as input:
    git = subprocess.Popen(['/usr/bin/git', 'hash-object', '--stdin'], stdin=input,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, pass_fds=(lease,))
with open(sys.argv[2], 'w') as output:
    json.dump([os.getpid(), git.pid], output)
git.wait()
";
    Command::new("python3")
        .args(["-I", "-c", script])
        .arg(directory.join("lease-input"))
        .arg(directory.join("lease-pids"))
        .stdin(store.lease().unwrap())
        .status()
        .unwrap();
}

struct LeaseProcesses {
    worker: std::process::Child,
    children: Vec<u32>,
}
impl Drop for LeaseProcesses {
    fn drop(&mut self) {
        let _ = self.worker.kill();
        let _ = self.worker.wait();
        for child in &self.children {
            if let Some(pid) = rustix::process::Pid::from_raw(child.cast_signed()) {
                let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
            }
        }
    }
}

#[test]
fn source_lock_survives_worker_and_python_exit_until_actual_git_exits() {
    if let Some(root) = std::env::var_os("HORIZON_SOURCE_LEASE_CHILD_ROOT") {
        lease_child(std::path::Path::new(&root));
        return;
    }
    let f = Fixture::ready();
    let directory = f.root().parent().unwrap().to_owned();
    let fifo = directory.join("lease-input");
    assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let _peer = fs::OpenOptions::new().read(true).write(true).open(fifo).unwrap();
    let mut processes = LeaseProcesses {
        worker: Command::new(std::env::current_exe().unwrap()).args(["--exact", "bootstrap::initialize::tests::membership::namespaces::source::source_lock_survives_worker_and_python_exit_until_actual_git_exits", "--nocapture"])
            .env("HORIZON_SOURCE_LEASE_CHILD_ROOT", f.root()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap(),
        children: Vec::new(),
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Ok(bytes) = fs::read(directory.join("lease-pids"))
            && let Ok(pids) = serde_json::from_slice::<Vec<u32>>(&bytes)
        {
            processes.children = pids;
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(Store::open(&f.root()).is_err());
    processes.worker.kill().unwrap();
    processes.worker.wait().unwrap();
    assert!(Store::open(&f.root()).is_err(), "worker exit released helper lease");
    let python = processes.children.remove(0);
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(python.cast_signed()).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while fs::read_to_string(format!("/proc/{python}/stat")).is_ok_and(|stat| {
        !stat
            .rsplit_once(") ")
            .is_some_and(|(_, fields)| fields.starts_with('Z'))
    }) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let git = processes.children[0];
    assert_eq!(
        fs::read_link(format!("/proc/{git}/exe")).unwrap(),
        std::path::Path::new("/usr/bin/git")
    );
    assert!(Store::open(&f.root()).is_err(), "Python exit released Git lease");
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(git.cast_signed()).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    processes.children.clear();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while Store::open(&f.root()).is_err() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    for executable in ["/bin/true", "/nonexistent-source-lease-fixture"] {
        let store = Store::open(&f.root()).unwrap();
        let result = Command::new(executable).stdin(store.lease().unwrap()).status();
        assert_eq!(result.is_ok(), executable == "/bin/true");
        drop(store);
        drop(Store::open(&f.root()).unwrap());
    }
}
