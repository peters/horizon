#![cfg(unix)]
use base64::Engine as _;
use horizon_app_provider::{
    Error,
    api::BrowserStack,
    tunnel::{LocalPort, VerifiedBinary},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;
// Linux guardian probes use monotonic polling; other platforms only use bounded waits.
#[cfg(target_os = "linux")]
use std::time::Instant;
use zeroize::Zeroizing;

const SCRIPT: &[u8] = b"#!/usr/bin/python3\nimport json,sys,time\np=sys.argv[sys.argv.index('--config-file')+1]\nkey=json.loads(open(p).read().partition(':')[2].strip())\nassert key=='synthetic-key' and key not in ' '.join(sys.argv)\nassert '--log-file' not in sys.argv\nprint(key+' You can now access your local server',flush=True)\ntime.sleep(60)\n";

struct State {
    _directory: tempfile::TempDir,
    path: PathBuf,
}
fn state() -> State {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().canonicalize().unwrap();
    State {
        _directory: directory,
        path,
    }
}
fn worker() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_horizon-app-provider"))
}
fn checksum(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes).iter().fold(String::new(), |mut text, byte| {
        write!(text, "{byte:02x}").unwrap();
        text
    })
}

fn binary() -> VerifiedBinary {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    let bytes = SCRIPT;
    file.write_all(bytes).unwrap();
    VerifiedBinary::capture(file.path(), &checksum(bytes)).unwrap()
}
fn provider() -> BrowserStack {
    let header = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("synthetic-user:synthetic-key")
    );
    BrowserStack::new("https://hub-cloud.browserstack.com", Zeroizing::new(header)).unwrap()
}
fn request(state: &State, ports: Vec<LocalPort>) -> horizon_app_provider::tunnel_guard::Request {
    horizon_app_provider::tunnel_guard::Request {
        worker: worker().to_owned(),
        state: state.path.clone(),
        binary: binary(),
        ports,
        operation: uuid::Uuid::new_v4(),
        lifetime: Duration::from_secs(30),
    }
}

fn receipt(state: &State) -> Value {
    serde_json::from_slice(&std::fs::read(state.path.join("process.json")).unwrap()).unwrap()
}

#[test]
fn cleanup_acknowledges_only_after_tunnel_and_private_files_are_retired() {
    let state = state();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut tunnel = provider()
        .guarded_tunnel(
            request(
                &state,
                vec![LocalPort {
                    address: listener.local_addr().unwrap(),
                    tls: false,
                }],
            ),
            |_, _| Ok(()),
        )
        .unwrap();
    let status = serde_json::to_string(&tunnel.status().unwrap()).unwrap();
    assert!(!status.contains("synthetic-key") && !status.contains("synthetic-user"));
    let before = receipt(&state);
    assert_eq!(before["complete"], false);
    let config = PathBuf::from(before["config"].as_str().unwrap());
    let executable = PathBuf::from(before["binary"].as_str().unwrap());
    tunnel.close().unwrap();
    tunnel.close().unwrap();
    assert_eq!(receipt(&state)["complete"], true);
    assert!(!config.exists() && !executable.exists());
    assert!(!tunnel.status().unwrap().ready);
}

#[test]
fn failed_journal_never_authorizes_a_tunnel_child() {
    let state = state();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let result = provider().guarded_tunnel(
        request(
            &state,
            vec![LocalPort {
                address: listener.local_addr().unwrap(),
                tls: false,
            }],
        ),
        |_, _| Err(Error::OwnershipRefused),
    );
    assert!(matches!(result, Err(Error::OwnershipRefused)));
    let record = receipt(&state);
    assert_eq!(record["child_pid"], Value::Null);
    assert_eq!(record["config"], Value::Null);
}

#[test]
fn cancelling_one_tunnel_does_not_close_the_other_worker() {
    let a = state();
    let b = state();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let ports = vec![LocalPort {
        address: listener.local_addr().unwrap(),
        tls: false,
    }];
    let mut first = provider()
        .guarded_tunnel(request(&a, ports.clone()), |_, _| Ok(()))
        .unwrap();
    let mut second = provider().guarded_tunnel(request(&b, ports), |_, _| Ok(())).unwrap();
    first.close().unwrap();
    assert!(second.status().unwrap().ready);
    second.close().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn killed_calling_host_without_destructors_still_retires_owned_tunnel() {
    use std::process::Command;
    let state = state();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let executable = state.path.join("fixture-binary");
    std::fs::write(&executable, SCRIPT).unwrap();
    // Test-only raw wire uses synthetic credentials; production wire stays private in the host adapter.
    let source = state.path.join("tunnel.py");
    let worker_wire = serde_json::json!({"operation":uuid::Uuid::new_v4(),"state":state.path,"binary":executable,"checksum":checksum(SCRIPT),"key":"synthetic-key","ports":[{"address":listener.local_addr().unwrap(),"tls":false}],"lifetime_seconds":30,"deadline_millis":horizon_app_process::lifetime::deadline_after(Duration::from_secs(30)).unwrap()});
    let program = format!(
        "import subprocess,json,time,pathlib\np=subprocess.Popen([{},'--tunnel-guard'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)\np.stdin.write(json.dumps(json.loads({}))+'\\n');p.stdin.flush()\nassert json.loads(p.stdout.readline())=='armed'\np.stdin.write('start\\n');p.stdin.flush()\nassert json.loads(p.stdout.readline())=='ready'\npathlib.Path({}).write_text('ready')\ntime.sleep(60)\n",
        serde_json::to_string(worker().to_str().unwrap()).unwrap(),
        serde_json::to_string(&worker_wire.to_string()).unwrap(),
        serde_json::to_string(state.path.join("ready").to_str().unwrap()).unwrap()
    );
    std::fs::write(&source, program).unwrap();
    let mut caller = Command::new("python3").arg(&source).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !state.path.join("ready").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    caller.kill().unwrap();
    caller.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while receipt(&state)["complete"] != true {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let record = receipt(&state);
    assert!(!Path::new(record["config"].as_str().unwrap()).exists());
}

#[test]
fn file_retirement_failure_keeps_receipt_incomplete_and_returns_uncertainty() {
    let state = state();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut tunnel = provider()
        .guarded_tunnel(
            request(
                &state,
                vec![LocalPort {
                    address: listener.local_addr().unwrap(),
                    tls: false,
                }],
            ),
            |_, _| Ok(()),
        )
        .unwrap();
    let before = receipt(&state);
    let config = PathBuf::from(before["config"].as_str().unwrap());
    let executable = PathBuf::from(before["binary"].as_str().unwrap());
    // Replace this fixture's credential path with a directory to force unlink failure.
    std::fs::remove_file(&config).unwrap();
    std::fs::create_dir(&config).unwrap();
    assert_eq!(tunnel.close(), Err(Error::TunnelCleanupUncertain));
    assert_eq!(receipt(&state)["complete"], false);
    assert!(executable.is_file());
    std::fs::remove_dir(&config).unwrap();
    std::fs::remove_file(&executable).unwrap();
}

#[test]
fn failed_startup_completes_only_after_exact_files_and_child_are_retired() {
    let state = state();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut requested = request(
        &state,
        vec![LocalPort {
            address: listener.local_addr().unwrap(),
            tls: false,
        }],
    );
    let mut source = tempfile::NamedTempFile::new().unwrap();
    let failed = b"#!/bin/sh\nexit 1\n";
    source.write_all(failed).unwrap();
    requested.binary = VerifiedBinary::capture(source.path(), &checksum(failed)).unwrap();
    let outcome = provider().guarded_tunnel(requested, |_, _| Ok(()));
    assert!(matches!(outcome, Err(Error::TunnelStartFailed)));
    let observed = receipt(&state);
    assert_eq!(observed["complete"], true);
    assert!(!Path::new(observed["config"].as_str().unwrap()).exists());
    assert!(!Path::new(observed["binary"].as_str().unwrap()).exists());
}
