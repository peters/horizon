//! The live lane of the remote browser panel: a real `horizon-browser host` in a
//! temporary runtime root, reached over a real SSH transport, with the panel of
//! Horizon as its client.
//!
//! The test needs three things that a build machine does not have:
//!
//! - an SSH server on `127.0.0.1` that answers without a prompt;
//! - the `horizon-browser` binary of this build on the `PATH` of that login
//!   shell, or in `HORIZON_BROWSER_REMOTE_EXECUTABLE`;
//! - a local Chromium for the profile of the host file.
//!
//! Without the SSH server or the binary the test prints its reason and does
//! nothing, because a machine without them cannot show this property. It is
//! `#[ignore]`d because the browser of the host is a real one: run it with
//!
//! ```text
//! cargo test -p horizon-core --test remote_host_panel -- --ignored
//! ```
#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use horizon_core::browser::{
    BackendKind, BrowserCommand, BrowserConfig, BrowserInput, BrowserPanelState, BrowserStatus,
};

const HOST_FILE: &str =
    "browser:\n  profiles:\n    mail:\n      backend: chromium\n      args: [\"--disable-gpu\", \"--no-first-run\"]\n";
const RECORD: &str = "profile-host.json";

fn ssh_target() -> String {
    std::env::var("HORIZON_BROWSER_REMOTE_SSH_TARGET")
        .ok()
        .filter(|target| !target.trim().is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

fn ssh_reachable(target: &str) -> bool {
    Command::new("ssh")
        .args(["-Tq", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(target)
        .arg("true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The `horizon-browser` program that the login shell of the target finds, as an
/// absolute path that the test can name on the far side of the transport.
fn remote_program() -> Option<PathBuf> {
    if let Some(program) = std::env::var_os("HORIZON_BROWSER_REMOTE_EXECUTABLE") {
        return Some(PathBuf::from(program));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join("horizon-browser"))
        .find(|candidate| candidate.is_file())
}

/// The profile host of one temporary runtime root. It stops when the test ends,
/// so no host and no browser of this test stays.
struct Host(Option<Child>);

impl Drop for Host {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = Command::new("kill").args(["-TERM", &child.id().to_string()]).status();
            let deadline = Instant::now() + Duration::from_secs(30);
            while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn start_host(root: &Path, program: &Path) -> Host {
    let config = root.join("host.yaml");
    std::fs::write(&config, HOST_FILE).expect("host file");
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).expect("mode");
    let child = Command::new(program)
        .args(["host", "--config"])
        .arg(&config)
        .env("HORIZON_BROWSER_ROOT", root.join("state"))
        .env("RUST_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the profile host");
    // The guard owns the process, so a host that never writes its record is
    // stopped before this function reports the failure.
    let host = Host(Some(child));
    let record = root.join("state").join(RECORD);
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Ok(file) = std::fs::File::open(&record) {
            let mut line = String::new();
            if BufReader::new(file).read_line(&mut line).is_ok() && line.contains("host_instance") {
                return host;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(host);
    panic!("the profile host wrote no {RECORD}");
}

#[test]
#[ignore = "needs an SSH server on the target, the horizon-browser binary there and a local Chromium"]
fn a_panel_shows_the_page_of_a_browser_host_and_sends_its_input_there() {
    let target = ssh_target();
    if !ssh_reachable(&target) {
        eprintln!(
            "skipped: `ssh -Tq {target}` needs an SSH server that answers without a prompt, \
             with the horizon-browser binary of this build on its PATH"
        );
        return;
    }
    let Some(program) = remote_program() else {
        eprintln!("skipped: no horizon-browser program is on the PATH of this test");
        return;
    };
    let root = tempfile::tempdir().expect("temporary runtime root");
    // SAFETY: no other test of this binary reads these variables, and the test
    // process owns them for its whole run.
    unsafe {
        std::env::set_var("HORIZON_BROWSER_REMOTE_ROOT", root.path().join("state"));
        std::env::set_var("HORIZON_BROWSER_REMOTE_EXECUTABLE", &program);
    }
    let _running = start_host(root.path(), &program);
    let backend = horizon_core::browser::RemoteHostBackend::new(&target, Some("mail".to_string()))
        .expect("the SSH target of the test");
    let mut panel = BrowserPanelState::start_remote_host(
        "remote-host-panel",
        &BrowserConfig::default(),
        Some("https://example.test/".to_string()),
        &backend,
    )
    .expect("the channel to the profile host starts");
    assert!(panel.is_remote_host());
    assert_eq!(panel.remote_host_target(), Some(target.as_str()));

    let deadline = Instant::now() + Duration::from_secs(120);
    let ready = loop {
        panel.drain_events();
        match panel.status {
            BrowserStatus::Ready => break true,
            BrowserStatus::Error { ref message } => {
                eprintln!("the profile host did not open the page: {message}");
                break false;
            }
            _ => {}
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(ready, "the page of the profile host became ready");
    let backend_of_page = panel.backend();
    assert!(
        matches!(backend_of_page, BackendKind::ChromiumCdp | BackendKind::FirefoxBidi),
        "the panel reports the engine of the profile: {backend_of_page:?}"
    );
    let frame_deadline = Instant::now() + Duration::from_secs(60);
    while panel.frame_slot.latest().is_none() && Instant::now() < frame_deadline {
        panel.drain_events();
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        panel.frame_slot.latest().is_some(),
        "the panel shows the first frame of the page"
    );

    assert!(panel.try_send(BrowserCommand::Input(BrowserInput::KeyDown {
        physical_key: Some(horizon_core::browser::BrowserKey::Tab),
        key: horizon_core::browser::BrowserKey::Tab,
        text: None,
        modifiers: horizon_core::browser::BrowserModifiers::none(),
        repeat: false,
        edit_command: None,
    })));

    panel.stop();
    let stopped = Instant::now() + Duration::from_secs(30);
    while Instant::now() < stopped {
        panel.drain_events();
        if !panel.is_remote_host() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!panel.is_remote_host(), "the channel of the panel ended");
}
