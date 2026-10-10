#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt as _};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use horizon_app_process::{
    Error, Event, Kind, Request,
    client::Process,
    diagnostic::{Cause, Operation, Reason, Retention},
};
use serde_json::Value;

struct PrivateTemp {
    _directory: tempfile::TempDir,
    path: std::path::PathBuf,
}
impl PrivateTemp {
    fn path(&self) -> &Path {
        &self.path
    }
}
fn private_temp() -> std::io::Result<PrivateTemp> {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()?;
    let path = directory.path().canonicalize()?;
    Ok(PrivateTemp {
        _directory: directory,
        path,
    })
}

fn retain_evidence(state: &Path, label: &str) {
    let Some(path) = std::env::var_os("HORIZON_PROCESS_TEST_EVIDENCE_DIR") else {
        return;
    };
    let directory = horizon_app_process::storage::Directory::open(Path::new(&path)).unwrap();
    for name in ["output.log", "process.json", "host-diagnostic.json"] {
        let bytes = match std::fs::read(state.join(name)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => panic!("synthetic evidence unavailable: {:?}", error.kind()),
        };
        let mut file = directory.new_file(&format!("{label}-{name}")).unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
    }
}

fn worker() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_horizon-app-process"))
}

fn backend() -> Vec<String> {
    vec!["python3".into(), "-c".into(), "import json,socket,sys,time; s=socket.socket(); s.bind(('127.0.0.1',0)); s.listen(); print(json.dumps({'native_backend_ready':1,'port':s.getsockname()[1]}),flush=True); request=json.loads(sys.stdin.read()); time.sleep(.1); s.close(); print(json.dumps({'native_backend_closed':1,'nonce':request['nonce']}),flush=True)".into()]
}

fn ready(process: &mut Process) -> u16 {
    assert!(matches!(
        process.next(Duration::from_secs(5)).unwrap(),
        Event::Started {}
    ));
    match process.next(Duration::from_secs(5)).unwrap() {
        Event::Ready { port } => port,
        _ => panic!("backend did not become ready"),
    }
}

#[test]
fn build_outputs_stay_private_and_terminal_acknowledgement_follows_cleanup() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let request = Request::new(
        root.path(),
        state.path(),
        vec![
            "/bin/sh".into(),
            "-c".into(),
            "echo synthetic-private-output; echo synthetic-private-error >&2".into(),
        ],
        Kind::Build,
        2,
        3,
    )
    .unwrap();
    let mut process = Process::start(worker(), request, |_, _| Ok(())).unwrap();
    let mut public = Vec::new();
    loop {
        let event = process.next(Duration::from_secs(5)).unwrap();
        public.extend(serde_json::to_vec(&event).unwrap());
        if matches!(event, Event::Complete { success: true }) {
            break;
        }
    }
    process.close().unwrap();
    assert!(!String::from_utf8(public).unwrap().contains("synthetic-private"));
    let receipt: Value = serde_json::from_slice(&std::fs::read(state.path().join("process.json")).unwrap()).unwrap();
    assert_eq!(receipt["complete"], true);
    let log = std::fs::read_to_string(state.path().join("output.log")).unwrap();
    assert!(log.contains("synthetic-private-output") && log.contains("synthetic-private-error"));
}

#[test]
fn cancelling_one_backend_keeps_the_other_available() {
    let root = private_temp().unwrap();
    let a = private_temp().unwrap();
    let b = private_temp().unwrap();
    let mut first = Process::start(
        worker(),
        Request::new(root.path(), a.path(), backend(), Kind::Backend, 2, 10).unwrap(),
        |_, _| Ok(()),
    )
    .unwrap();
    let mut second = Process::start(
        worker(),
        Request::new(root.path(), b.path(), backend(), Kind::Backend, 2, 10).unwrap(),
        |_, _| Ok(()),
    )
    .unwrap();
    let first_port = ready(&mut first);
    let second_port = ready(&mut second);
    assert_ne!(first_port, second_port);
    first.close().unwrap();
    assert!(TcpStream::connect(("127.0.0.1", first_port)).is_err());
    assert!(TcpStream::connect(("127.0.0.1", second_port)).is_ok());
    second.close().unwrap();
}

#[test]
fn journal_failure_prevents_declared_command_execution() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let marker = root.path().join("should-not-exist");
    let request = Request::new(
        root.path(),
        state.path(),
        vec!["touch".into(), marker.to_string_lossy().into_owned()],
        Kind::Build,
        2,
        3,
    )
    .unwrap();
    assert_eq!(
        Process::start(worker(), request, |_, _| Err(Error::StateUnavailable)).err(),
        Some(Error::StateUnavailable)
    );
    assert!(!marker.exists());
}

#[test]
fn unstarted_guardian_persists_positive_cleanup_after_journal_callback_failure() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let request = Request::new(
        root.path(),
        state.path(),
        vec!["touch".into(), "forbidden".into()],
        Kind::Build,
        2,
        3,
    )
    .unwrap();
    assert!(matches!(
        Process::start(worker(), request, |_, _| Err(Error::StateUnavailable)),
        Err(Error::StateUnavailable)
    ));
    let receipt: Value = horizon_app_process::storage::Directory::open(state.path())
        .unwrap()
        .receipt()
        .unwrap();
    assert_eq!(receipt["complete"], true);
    assert!(receipt["child_pid"].is_null());
    assert!(!root.path().join("forbidden").exists());
}

#[test]
fn malformed_readiness_and_deadline_fail_without_public_process_diagnostics() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let request = Request::new(
        root.path(),
        state.path(),
        vec![
            "python3".into(),
            "-c".into(),
            "import json,sys,signal; signal.signal(signal.SIGTERM,lambda *args:None); print('secret-output',flush=True); request=json.loads(sys.stdin.read()); print(json.dumps({'native_backend_closed':1,'nonce':request['nonce']}),flush=True)".into(),
        ],
        Kind::Backend,
        1,
        2,
    )
    .unwrap();
    let mut process = Process::start(worker(), request, |_, _| Ok(())).unwrap();
    assert!(matches!(
        process.next(Duration::from_secs(5)).unwrap(),
        Event::Started {}
    ));
    let event = process.next(Duration::from_secs(5)).unwrap();
    assert!(matches!(&event, Event::Failed { code } if code == "app_process_failed"));
    process.close().unwrap();
    assert!(!serde_json::to_string(&event).unwrap().contains("secret-output"));
    let state = private_temp().unwrap();
    let request = Request::new(
        root.path(),
        state.path(),
        vec!["python3".into(), "-c".into(), "import json,sys,signal; signal.signal(signal.SIGTERM,lambda *args:None); request=json.loads(sys.stdin.read()); print(json.dumps({'native_backend_closed':1,'nonce':request['nonce']}),flush=True)".into()],
        Kind::Backend,
        1,
        10,
    )
    .unwrap();
    let mut process = Process::start(worker(), request, |_, _| Ok(())).unwrap();
    assert!(matches!(
        process.next(Duration::from_secs(5)).unwrap(),
        Event::Started {}
    ));
    assert!(
        matches!(process.next(Duration::from_secs(5)).unwrap(), Event::Failed { code } if code == "app_process_timeout")
    );
    process.close().unwrap();
    let output = std::fs::read_to_string(state.path().join("output.log")).unwrap();
    assert!(output.contains("Guardian: StartupExpired"));
    assert!(!output.contains("Guardian: LifetimeExpired"));
}

#[test]
fn losing_parent_pipe_stops_backend_and_records_completion() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let errors = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(state.path().join("guardian-error.log"))
        .unwrap();
    // Exercise the real guardian protocol without a client Drop/close callback.
    let mut guardian = Command::new(worker())
        .arg("--guard")
        .env_clear()
        .env("TMPDIR", std::env::temp_dir())
        .env("TEMP", std::env::temp_dir())
        .env("TMP", std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(errors))
        .spawn()
        .unwrap();
    let mut input = guardian.stdin.take().unwrap();
    let metadata = std::fs::metadata(root.path()).unwrap();
    let spec = serde_json::json!({"operation":uuid::Uuid::new_v4(), "root_device":metadata.dev(), "root_inode":metadata.ino(), "root":root.path(), "state":state.path(), "argv":backend(), "environment":{"PATH":std::env::var("PATH").unwrap()}, "kind":"backend", "startup_seconds":2, "lifetime_seconds":10, "deadline_millis":horizon_app_process::lifetime::deadline_after(Duration::from_secs(10)).unwrap()});
    writeln!(input, "{spec}").unwrap();
    let mut output = BufReader::new(guardian.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    assert!(matches!(serde_json::from_str::<Event>(&line).unwrap(), Event::Armed {}));
    input.write_all(b"start\n").unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    assert!(matches!(
        serde_json::from_str::<Event>(&line).unwrap(),
        Event::Started {}
    ));
    line.clear();
    output.read_line(&mut line).unwrap();
    let event = serde_json::from_str::<Event>(&line).unwrap_or_else(|error| {
        use std::io::Read as _;
        let mut diagnostics = Vec::new();
        for name in ["guardian-error.log", "output.log", "process.json"] {
            if let Ok(file) = std::fs::File::open(state.path().join(name)) {
                let _ = file.take(4096).read_to_end(&mut diagnostics);
            }
        }
        panic!(
            "guardian readiness failed: {error}; private synthetic diagnostics: {}",
            String::from_utf8_lossy(&diagnostics)
        );
    });
    let Event::Ready { port } = event else {
        panic!("not ready");
    };
    drop(input); // exactly the EOF produced by abrupt host death
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if guardian.try_wait().unwrap().is_some() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    let receipt: Value = serde_json::from_slice(&std::fs::read(state.path().join("process.json")).unwrap()).unwrap();
    assert_eq!(receipt["complete"], true);
}

#[test]
fn state_permissions_symlinks_and_reuse_are_refused() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let alias = root.path().join("state");
    symlink(state.path(), &alias).unwrap();
    assert_eq!(
        Request::new(root.path(), &alias, backend(), Kind::Backend, 1, 2).err(),
        Some(Error::StateUnavailable)
    );
    std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        Request::new(root.path(), state.path(), backend(), Kind::Backend, 1, 2).err(),
        Some(Error::StateUnavailable)
    );
    std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(state.path().join("initialized"), b"").unwrap();
    assert_eq!(
        Process::start(
            worker(),
            Request::new(root.path(), state.path(), backend(), Kind::Backend, 1, 2).unwrap(),
            |_, _| Ok(())
        )
        .err(),
        Some(Error::CleanupUncertain)
    );
}

#[test]
fn oversized_valid_argv_is_rejected_before_guardian_or_journal_side_effects() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let argv = vec!["a".repeat(4096); 16];
    assert_eq!(
        Request::new(root.path(), state.path(), argv, Kind::Build, 1, 2).err(),
        Some(Error::Invalid)
    );
    assert!(std::fs::read_dir(state.path()).unwrap().next().is_none());
}

#[test]
fn abrupt_host_death_still_cleans_its_backend_without_running_drop() {
    const ROOT: &str = "HORIZON_TEST_CRASH_ROOT";
    const STATE: &str = "HORIZON_TEST_CRASH_STATE";
    if let (Ok(root), Ok(state)) = (std::env::var(ROOT), std::env::var(STATE)) {
        let mut process = Process::start(
            worker(),
            Request::new(Path::new(&root), Path::new(&state), backend(), Kind::Backend, 2, 10).unwrap(),
            |_, _| Ok(()),
        )
        .unwrap();
        let port = ready(&mut process);
        std::fs::write(Path::new(&root).join("host-ready"), port.to_string()).unwrap();
        std::thread::sleep(Duration::from_secs(60));
        panic!("test parent did not kill its owned host");
    }
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let mut host = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "abrupt_host_death_still_cleans_its_backend_without_running_drop",
            "--nocapture",
        ])
        .env(ROOT, root.path())
        .env(STATE, state.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let port: u16 = loop {
        if let Ok(port) = std::fs::read_to_string(root.path().join("host-ready")) {
            break port.parse().unwrap();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    };
    host.kill().unwrap();
    host.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let receipt: Value =
            serde_json::from_slice(&std::fs::read(state.path().join("process.json")).unwrap()).unwrap();
        if receipt["complete"] == true {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
}

#[test]
fn missing_nested_cleanup_acknowledgement_keeps_receipt_uncertain() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let argv = vec!["python3".into(), "-c".into(), "import json,socket,sys; s=socket.socket(); s.bind(('127.0.0.1',0)); s.listen(); print(json.dumps({'native_backend_ready':1,'port':s.getsockname()[1]}),flush=True); sys.stdin.read()".into()];
    let mut process = Process::start(
        worker(),
        Request::new(root.path(), state.path(), argv, Kind::Backend, 2, 10).unwrap(),
        |_, _| Ok(()),
    )
    .unwrap();
    let port = ready(&mut process);
    assert_eq!(process.close(), Err(Error::CleanupUncertain));
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    let receipt: Value = serde_json::from_slice(&std::fs::read(state.path().join("process.json")).unwrap()).unwrap();
    assert_eq!(receipt["complete"], false);
    let task = Path::new(receipt["task"].as_str().unwrap());
    assert!(task.is_dir());
    assert!(!receipt["task_identity"].is_null());
    let predictable = format!(
        "horizon-native-command-{}",
        receipt["operation"].as_str().unwrap().replace('-', "")
    );
    assert_ne!(task.file_name().unwrap().to_string_lossy(), predictable);
    assert_eq!(
        task.file_name().unwrap().to_string_lossy().len(),
        predictable.len() + 33
    );
    assert!(
        task.file_name()
            .unwrap()
            .to_string_lossy()
            .contains(&receipt["operation"].as_str().unwrap().replace('-', ""))
    );
    // This fixture creates no nested children; its exact helper has exited and loopback socket is closed.
    std::fs::remove_dir_all(task).unwrap();
}

#[test]
fn premature_nested_cleanup_acknowledgement_never_authorizes_completion() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let argv = vec!["python3".into(), "-c".into(), "import json,socket,sys; s=socket.socket(); s.bind(('127.0.0.1',0)); s.listen(); print(json.dumps({'native_backend_ready':1,'port':s.getsockname()[1]}),flush=True); print(json.dumps({'native_backend_closed':1,'nonce':'11111111-1111-4111-8111-111111111111'}),flush=True); sys.stdin.read()".into()];
    let mut process = Process::start(
        worker(),
        Request::new(root.path(), state.path(), argv, Kind::Backend, 2, 10).unwrap(),
        |_, _| Ok(()),
    )
    .unwrap();
    let port = ready(&mut process);
    assert_eq!(process.close(), Err(Error::CleanupUncertain));
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    let receipt: Value = serde_json::from_slice(&std::fs::read(state.path().join("process.json")).unwrap()).unwrap();
    assert_eq!(receipt["complete"], false);
    let task = Path::new(receipt["task"].as_str().unwrap());
    assert!(task.is_dir());
    assert!(!receipt["task_identity"].is_null());
    assert!(
        task.file_name()
            .unwrap()
            .to_string_lossy()
            .contains(&receipt["operation"].as_str().unwrap().replace('-', ""))
    );
    // This fixture creates no nested children; its exact helper has exited and loopback socket is closed.
    std::fs::remove_dir_all(task).unwrap();
}

#[test]
fn missing_executable_reports_known_no_child_failure_with_complete_receipt() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let mut process = Process::start(
        worker(),
        Request::new(
            root.path(),
            state.path(),
            vec![root.path().join("missing-executable").to_string_lossy().into_owned()],
            Kind::Build,
            2,
            3,
        )
        .unwrap(),
        |_, _| Ok(()),
    )
    .unwrap();
    assert!(
        matches!(process.next(Duration::from_secs(5)).unwrap(), Event::Failed { code } if code == "app_process_start_failed")
    );
    process.close().unwrap();
    let receipt: Value = serde_json::from_slice(&std::fs::read(state.path().join("process.json")).unwrap()).unwrap();
    assert_eq!(receipt["complete"], true);
    assert!(receipt["child_pid"].is_null());
}

#[test]
fn declared_command_task_directory_does_not_contain_guardian_receipt_or_log() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let argv = vec!["python3".into(), "-c".into(), "import os,pathlib; p=pathlib.Path(os.environ['HORIZON_APP_BACKEND_DIR']); assert not (p/'process.json').exists(); assert not (p/'output.log').exists(); (p/'process.json').write_text('synthetic child-owned file'); print(p,flush=True)".into()];
    let mut process = Process::start(
        worker(),
        Request::new(root.path(), state.path(), argv, Kind::Build, 2, 3).unwrap(),
        |_, _| Ok(()),
    )
    .unwrap();
    assert!(matches!(
        process.next(Duration::from_secs(5)).unwrap(),
        Event::Started {}
    ));
    assert!(matches!(
        process.next(Duration::from_secs(5)).unwrap(),
        Event::Complete { success: true }
    ));
    process.close().unwrap();
    let receipt: Value = serde_json::from_slice(&std::fs::read(state.path().join("process.json")).unwrap()).unwrap();
    assert_eq!(receipt["complete"], true);
    let child_path = std::fs::read_to_string(state.path().join("output.log")).unwrap();
    assert!(!Path::new(child_path.trim()).exists());
}

#[test]
fn armed_command_keeps_its_held_directory_when_the_original_root_path_is_replaced() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let moved = root.path().with_extension("moved");
    let request = Request::new(
        root.path(),
        state.path(),
        vec!["touch".into(), "authorized".into()],
        Kind::Build,
        2,
        3,
    )
    .unwrap();
    let mut process = Process::start(worker(), request, |_, _| {
        std::fs::rename(root.path(), &moved).unwrap();
        std::fs::create_dir(root.path()).unwrap();
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        process.next(Duration::from_secs(5)).unwrap(),
        Event::Started {}
    ));
    assert!(matches!(
        process.next(Duration::from_secs(5)).unwrap(),
        Event::Complete { success: true }
    ));
    process.close().unwrap();
    assert!(moved.join("authorized").is_file());
    assert!(!root.path().join("authorized").exists());
    std::fs::remove_dir_all(moved).unwrap();
}
#[test]
fn request_cannot_bind_to_another_workspaces_directory() {
    let root = private_temp().unwrap();
    let other = private_temp().unwrap();
    let state = private_temp().unwrap();
    let request = Request::new(
        root.path(),
        state.path(),
        vec!["touch".into(), "forbidden".into()],
        Kind::Build,
        2,
        3,
    )
    .unwrap();
    assert!(matches!(
        request.bind_root(&std::fs::File::open(other.path()).unwrap()),
        Err(Error::Invalid)
    ));
}

#[test]
fn original_guardian_lifetime_includes_a_delayed_durable_handshake() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let start = Instant::now();
    let request = Request::new(root.path(), state.path(), backend(), Kind::Backend, 1, 4).unwrap();
    let mut process = Process::start(worker(), request, |_, _| {
        std::thread::sleep(Duration::from_secs(2));
        Ok(())
    })
    .unwrap();
    let port = ready(&mut process);
    loop {
        let event = process.next(Duration::from_secs(5)).unwrap();
        if matches!(event, Event::Failed { .. }) {
            break;
        }
    }
    // Original four seconds plus bounded graceful cleanup, not a fresh four-second lease.
    assert!(start.elapsed() < Duration::from_millis(5500));
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    process.close().unwrap();
}

#[test]
fn cooperative_backend_cleanup_can_acknowledge_after_more_than_two_seconds() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let argv = backend()
        .into_iter()
        .map(|arg| arg.replace("time.sleep(.1)", "time.sleep(3)"))
        .collect();
    let mut process = Process::start(
        worker(),
        Request::new(root.path(), state.path(), argv, Kind::Backend, 2, 10).unwrap(),
        |_, _| Ok(()),
    )
    .unwrap();
    let port = ready(&mut process);
    let started = Instant::now();
    process.close().unwrap();
    assert!(started.elapsed() >= Duration::from_secs(3));
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    let receipt: Value = serde_json::from_slice(&std::fs::read(state.path().join("process.json")).unwrap()).unwrap();
    assert_eq!(receipt["complete"], true);
    assert!(!Path::new(receipt["task"].as_str().unwrap()).exists());
}

#[test]
fn retained_capability_logs_first_host_cause_after_normal_close() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let mut log = None;
    let request = Request::new(root.path(), state.path(), backend(), Kind::Backend, 2, 10).unwrap();
    let mut process =
        Process::start_with_diagnostics(worker(), request, |_, _| Ok(()), |value| log = Some(value)).unwrap();
    ready(&mut process);
    process.close().unwrap();
    let cause = Cause::Io {
        operation: Operation::Guardian,
        kind: std::io::ErrorKind::BrokenPipe,
    };
    let log = log.unwrap();
    assert_eq!(log.record(cause, Duration::from_secs(1)), Ok(Retention::Recorded));
    assert_eq!(
        log.record(
            Cause::State {
                operation: Operation::Output,
                reason: Reason::TransportFailed
            },
            Duration::from_secs(1)
        ),
        Ok(Retention::AlreadyRecorded)
    );
    let output = std::fs::read_to_string(state.path().join("output.log")).unwrap();
    assert_eq!(
        output.matches("app_host_unavailable: Guardian: I/O BrokenPipe").count(),
        1
    );
    assert!(!output.contains("Output: TransportFailed"));
    let receipt: Value = serde_json::from_slice(&std::fs::read(state.path().join("process.json")).unwrap()).unwrap();
    assert_eq!(receipt["complete"], true);
    retain_evidence(state.path(), "after-close");
}

#[test]
fn startup_journal_failure_retains_capability_without_authorizing_child() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let mut log = None;
    let request = Request::new(
        root.path(),
        state.path(),
        vec!["touch".into(), "forbidden".into()],
        Kind::Build,
        2,
        3,
    )
    .unwrap();
    assert!(matches!(
        Process::start_with_diagnostics(
            worker(),
            request,
            |_, _| Err(Error::StateUnavailable),
            |value| log = Some(value)
        ),
        Err(Error::StateUnavailable)
    ));
    let cause = Cause::State {
        operation: Operation::Guardian,
        reason: Reason::MissingState,
    };
    assert_eq!(
        log.unwrap().record(cause, Duration::from_secs(1)),
        Ok(Retention::Recorded)
    );
    assert!(!root.path().join("forbidden").exists());
    assert!(
        std::fs::read_to_string(state.path().join("output.log"))
            .unwrap()
            .contains("app_host_unavailable: Guardian: MissingState")
    );
    retain_evidence(state.path(), "startup-failure");
}

#[test]
fn guardian_and_declared_child_use_the_trusted_private_temp_namespace() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let request = Request::new(
        root.path(),
        state.path(),
        vec![
            "python3".into(),
            "-c".into(),
            "import os; print(os.environ['TMPDIR']); print(os.environ['TEMP']); print(os.environ['TMP'])".into(),
        ],
        Kind::Build,
        2,
        3,
    )
    .unwrap();
    let mut process = Process::start(worker(), request, |_, _| Ok(())).unwrap();
    loop {
        if matches!(
            process.next(Duration::from_secs(5)).unwrap(),
            Event::Complete { success: true }
        ) {
            break;
        }
    }
    process.close().unwrap();
    let receipt: Value = serde_json::from_slice(&std::fs::read(state.path().join("process.json")).unwrap()).unwrap();
    let task = Path::new(receipt["task"].as_str().unwrap());
    assert_eq!(task.parent().unwrap(), std::env::temp_dir().canonicalize().unwrap());
    let output = std::fs::read_to_string(state.path().join("output.log")).unwrap();
    assert_eq!(output.lines().collect::<Vec<_>>(), vec![task.to_str().unwrap(); 3]);
}

#[test]
fn real_guardian_stdout_stderr_and_host_cause_share_one_physical_log_cap() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let script = r"import json,sys,threading; print(json.dumps({'native_backend_ready':1,'port':33327}),flush=True); flood=lambda stream:[(stream.write('x'*8192+'\n'),stream.flush()) for _ in range(400)]; a=threading.Thread(target=flood,args=(sys.stdout,)); b=threading.Thread(target=flood,args=(sys.stderr,)); a.start(); b.start(); request=json.loads(sys.stdin.read()); a.join(); b.join(); print(json.dumps({'native_backend_closed':1,'nonce':request['nonce']}),flush=True)";
    let request = Request::new(
        root.path(),
        state.path(),
        vec!["python3".into(), "-c".into(), script.into()],
        Kind::Backend,
        2,
        20,
    )
    .unwrap();
    let mut log = None;
    let mut process =
        Process::start_with_diagnostics(worker(), request, |_, _| Ok(()), |value| log = Some(value)).unwrap();
    assert_eq!(ready(&mut process), 33327);
    assert_eq!(
        log.unwrap().record(
            Cause::Io {
                operation: Operation::Guardian,
                kind: std::io::ErrorKind::BrokenPipe
            },
            Duration::from_secs(2)
        ),
        Ok(Retention::Recorded)
    );
    process.close().unwrap();
    let bytes = std::fs::read(state.path().join("output.log")).unwrap();
    assert!(bytes.len() <= 4 * 1024 * 1024);
    assert!(bytes.len() >= 4 * 1024 * 1024 - 1024);
    assert_eq!(
        String::from_utf8(bytes)
            .unwrap()
            .matches("app_host_unavailable: Guardian: I/O BrokenPipe")
            .count(),
        1
    );
    let receipt: Value = serde_json::from_slice(&std::fs::read(state.path().join("process.json")).unwrap()).unwrap();
    assert_eq!(receipt["complete"], true);
    retain_evidence(state.path(), "concurrent-cap");
}
