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

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use horizon_core::browser::{
    BackendKind, BrowserCommand, BrowserConfig, BrowserInput, BrowserPanelState, BrowserStatus,
};
use horizon_core::{BrowserHostConfig, Panel, PanelId, PanelKind, PanelOptions, WorkspaceId};

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
    let state = root.join("state");
    let record = state.join(RECORD);
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        // The record is pretty-printed JSON, so it is read whole and parsed, not
        // by its first line.
        if let Some(instance) = recorded_instance(&record) {
            eprintln!("profile host {instance} wrote {}", record.display());
            return host;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Name what the host left behind, so a failure says why it never became
    // ready instead of only that the record was absent.
    let found = std::fs::read_dir(&state)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    drop(host);
    panic!(
        "the profile host wrote no readable {RECORD} within 30 seconds; {} holds {found:?}",
        state.display()
    );
}

/// Print the first real frame of the page, send a key press and a page scroll,
/// and give the frame counter before and after them. The host answers a call of
/// the contract only when the page took it, and a refused call reaches the panel
/// note instead of the page.
fn show_frame_and_send_input(panel: &mut BrowserPanelState) -> (u64, u64) {
    let frame_deadline = Instant::now() + Duration::from_secs(60);
    while panel.frame_slot.latest().is_none() && Instant::now() < frame_deadline {
        panel.drain_events();
        std::thread::sleep(Duration::from_millis(50));
    }
    let first = panel
        .frame_slot
        .latest()
        .expect("the panel shows the first frame of the page");
    eprintln!(
        "real frame: {}x{} in RGB, sequence {}, url {:?}, title {:?}, engine {:?}",
        first.width,
        first.height,
        first.seq,
        panel.url.as_deref().unwrap_or_default(),
        panel.title,
        panel.backend()
    );
    let before = first.seq;
    assert!(
        panel.try_send(BrowserCommand::Input(BrowserInput::KeyDown {
            physical_key: Some(horizon_core::browser::BrowserKey::Tab),
            key: horizon_core::browser::BrowserKey::Tab,
            text: None,
            modifiers: horizon_core::browser::BrowserModifiers::none(),
            repeat: false,
            edit_command: None,
        })),
        "the panel queued the key press"
    );
    assert!(
        panel.try_send(BrowserCommand::Input(BrowserInput::Wheel {
            x: 100.0,
            y: 100.0,
            delta_x: 0.0,
            delta_y: -240.0,
            modifiers: horizon_core::browser::BrowserModifiers::none(),
        })),
        "the panel queued the page scroll"
    );
    let input_deadline = Instant::now() + Duration::from_secs(30);
    while panel.frame_slot.latest().map(|frame| frame.seq) == Some(before) && Instant::now() < input_deadline {
        panel.drain_events();
        std::thread::sleep(Duration::from_millis(50));
    }
    panel.drain_events();
    assert!(
        panel.navigation_error.is_none(),
        "the host refused the input: {:?}",
        panel.navigation_error
    );
    let after = panel.frame_slot.latest().map(|frame| frame.seq).unwrap_or_default();
    assert!(
        after > before,
        "the page kept producing frames after the input ({before} -> {after})"
    );
    (before, after)
}

/// The host instance of a record that a running host wrote, when the file is
/// there and holds one.
fn recorded_instance(record: &Path) -> Option<String> {
    let text = std::fs::read_to_string(record).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("host_instance")
        .and_then(serde_json::Value::as_str)
        .filter(|instance| !instance.is_empty())
        .map(str::to_string)
}

/// The variable of the channel of a panel, so one lane holds its own root.
static LIVE_LANE: Mutex<()> = Mutex::new(());

/// A temporary runtime root for one live lane, with the variables that the
/// channel of a panel reads set to it. The lane keeps the returned guard for its
/// whole run, so the two lanes of this file never share a root.
fn live_root(program: &Path) -> (MutexGuard<'static, ()>, tempfile::TempDir) {
    let lane = LIVE_LANE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // A confined browser can not read a path with a hidden folder on it, so the
    // caller may name a visible folder to hold the runtime root of the test.
    let root = match std::env::var_os("HORIZON_CORE_REMOTE_HOST_ROOT") {
        Some(folder) => {
            let folder = PathBuf::from(folder);
            std::fs::create_dir_all(&folder).expect("root folder");
            tempfile::Builder::new()
                .prefix("panel-live-")
                .tempdir_in(&folder)
                .expect("temporary runtime root")
        }
        None => tempfile::tempdir().expect("temporary runtime root"),
    };
    // SAFETY: the guard serialises the live lanes of this file, each lane sets
    // both variables before it starts its host, and no other test of this binary
    // reads them.
    unsafe {
        std::env::set_var("HORIZON_BROWSER_REMOTE_ROOT", root.path().join("state"));
        std::env::set_var("HORIZON_BROWSER_REMOTE_EXECUTABLE", program);
    }
    (lane, root)
}

/// Wait until the page of the host is ready, naming why the host gave up when it
/// is not.
fn wait_for_ready(panel: &mut BrowserPanelState) -> bool {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        panel.drain_events();
        match panel.status {
            BrowserStatus::Ready => return true,
            BrowserStatus::Error { ref message } => {
                eprintln!(
                    "the profile host did not open the page: {message} | host note: {:?} | status: {:?}",
                    panel.navigation_error, panel.remote_status
                );
                return false;
            }
            _ => {}
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
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
    let (_lane, root) = live_root(&program);
    let _running = start_host(root.path(), &program);
    let backend = horizon_core::browser::RemoteHostBackend::new(&target, Some("mail".to_string()))
        .expect("the SSH target of the test");
    let mut panel = BrowserPanelState::start_remote_host(
        "remote-host-panel",
        &BrowserConfig::default(),
        Some("https://example.test/".to_string()),
        &backend,
    );
    assert!(panel.is_remote_host());
    assert_eq!(panel.remote_host_target(), Some(target.as_str()));

    assert!(wait_for_ready(&mut panel), "the page of the profile host became ready");
    let backend_of_page = panel.backend();
    assert!(
        matches!(backend_of_page, BackendKind::ChromiumCdp | BackendKind::FirefoxBidi),
        "the panel reports the engine of the profile: {backend_of_page:?}"
    );
    let (before, after) = show_frame_and_send_input(&mut panel);
    assert!(
        panel.remote_host_failure().is_none(),
        "the channel of the panel is live after the input: {:?}",
        panel.remote_host_failure()
    );
    eprintln!("real input round trip: key press and page scroll accepted, frames {before} -> {after}");

    panel.stop();
    let stopped = Instant::now() + Duration::from_secs(30);
    while Instant::now() < stopped {
        panel.drain_events();
        if !panel.remote_host_is_live() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !panel.remote_host_is_live(),
        "the channel of the panel ended with the panel"
    );
    assert!(
        panel.is_remote_host(),
        "the panel still reports the backend that it was created with"
    );
    println!("=== the live lane of the remote browser panel passed");
}

#[test]
#[ignore = "needs an SSH server on the target, the horizon-browser binary there and a local Chromium"]
fn a_panel_created_from_the_settings_uses_the_named_browser_host() {
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
    let (_lane, root) = live_root(&program);
    let _running = start_host(root.path(), &program);
    // The entry that the start of Horizon reads, before any panel of the test
    // exists. The first call wins, and this lane is the only caller.
    horizon_core::configure_browser_host(Some(BrowserHostConfig {
        ssh_target: Some(target.clone()),
        profile: Some("mail".to_string()),
    }));
    assert_eq!(
        horizon_core::browser_host().and_then(BrowserHostConfig::ssh_target),
        Some(target.as_str()),
        "the settings name the host before a panel is created"
    );

    // The route that a person takes: the preset picker creates the panel with
    // `follows_browser_host`, and the panel reads the named host instead of
    // starting the local browser.
    let mut panel = Panel::spawn(
        PanelId(1),
        WorkspaceId(1),
        PanelOptions {
            kind: PanelKind::Browser,
            command: Some("https://example.test/".to_string()),
            follows_browser_host: true,
            ..PanelOptions::default()
        },
    )
    .expect("a browser panel");
    let browser = panel.browser_mut().expect("a browser panel");
    assert!(
        browser.is_remote_host(),
        "a new panel that a person picks goes to the named host"
    );
    assert_eq!(browser.remote_host_target(), Some(target.as_str()));
    assert_eq!(browser.remote_host_profile(), Some("mail"));

    assert!(
        wait_for_ready(browser),
        "the page of the named host became ready in the panel of the route"
    );
    let (before, after) = show_frame_and_send_input(browser);
    assert!(
        browser.remote_host_failure().is_none(),
        "the channel of the routed panel is live after the input: {:?}",
        browser.remote_host_failure()
    );
    eprintln!("the routed panel painted and took input, frames {before} -> {after}");

    panel.restart().expect("the restart of a routed panel");
    let browser = panel.browser_mut().expect("a browser panel");
    assert!(
        browser.is_remote_host(),
        "a restart of the panel keeps the host that it was created with"
    );
    assert_eq!(browser.remote_host_target(), Some(target.as_str()));
    assert_eq!(browser.remote_host_profile(), Some("mail"));
    assert!(
        wait_for_ready(browser),
        "the restarted panel shows a page of the host again"
    );

    browser.stop();
    println!("=== the live lane of the browser panel route passed");
}
