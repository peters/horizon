#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use horizon_app_process::{Error, Event, Kind, Request, client::Process};
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

fn worker() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_horizon-app-process"))
}

fn backend() -> Vec<String> {
    vec!["python3".into(), "-c".into(), "import json,socket,sys,signal; signal.signal(signal.SIGTERM,lambda *args:None); s=socket.socket(); s.bind(('127.0.0.1',0)); s.listen(); print(json.dumps({'native_backend_ready':1,'port':s.getsockname()[1]}),flush=True); sys.stdin.read(); s.close(); print(json.dumps({'native_backend_closed':1}),flush=True)".into()]
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
fn malformed_readiness_and_deadline_fail_without_public_process_diagnostics() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    let request = Request::new(
        root.path(),
        state.path(),
        vec![
            "python3".into(),
            "-c".into(),
            "import json,sys,signal; signal.signal(signal.SIGTERM,lambda *args:None); print('secret-output',flush=True); sys.stdin.read(); print(json.dumps({'native_backend_closed':1}),flush=True)".into(),
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
        vec!["python3".into(), "-c".into(), "import json,sys,signal; signal.signal(signal.SIGTERM,lambda *args:None); sys.stdin.read(); print(json.dumps({'native_backend_closed':1}),flush=True)".into()],
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
    assert!(
        matches!(process.next(Duration::from_secs(5)).unwrap(), Event::Failed { code } if code == "app_process_timeout")
    );
    process.close().unwrap();
}

#[test]
fn losing_parent_pipe_stops_backend_and_records_completion() {
    let root = private_temp().unwrap();
    let state = private_temp().unwrap();
    // Exercise the real guardian protocol without a client Drop/close callback.
    let mut guardian = Command::new(worker())
        .arg("--guard")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = guardian.stdin.take().unwrap();
    let spec = serde_json::json!({"operation":uuid::Uuid::new_v4(), "root":root.path(), "state":state.path(), "argv":backend(), "environment":{"PATH":"/usr/bin:/bin"}, "kind":"backend", "startup_seconds":2, "lifetime_seconds":10});
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
    let Event::Ready { port } = serde_json::from_str::<Event>(&line).unwrap() else {
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
}
