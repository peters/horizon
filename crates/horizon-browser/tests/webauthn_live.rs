//! Live check of the `WebAuthn` bridge in real Chromium and Firefox panels.
//!
//! A local relying party serves the pages and verifies every credential on
//! the server side. This test binary is also the fake authenticator
//! command: `webauthn_live --fake-authenticator <dir>` keeps test P-256 keys
//! in `<dir>`. The browser part is skipped unless `--ignored` is passed:
//!
//! ```text
//! cargo test -p horizon-browser --test webauthn_live -- --ignored [chromium|firefox]
//! ```
//!
//! Set `HORIZON_LIVE_PROFILE_ROOT` to a directory that a Snap browser can
//! read when the browsers are Snap packages.

#[path = "webauthn_live/fake_authenticator.rs"]
mod fake_authenticator;
#[path = "webauthn_live/relying_party.rs"]
mod relying_party;

use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use horizon_browser::{
    BackendKind, BrowserCommand, BrowserConfig, BrowserEvent, BrowserSession, BrowserSessionConfig, FrameSlot,
    start_session,
};

use relying_party::RelyingParty;

const FAKE_AUTHENTICATOR_FLAG: &str = "--fake-authenticator";
const REPORT_TIMEOUT: Duration = Duration::from_secs(30);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(FAKE_AUTHENTICATOR_FLAG) {
        let Some(state) = args.get(2) else {
            std::process::exit(2);
        };
        std::process::exit(fake_authenticator::run(Path::new(state)));
    }
    if !args.iter().any(|arg| arg == "--ignored" || arg == "--include-ignored") {
        println!("webauthn_live: skipped; pass --ignored to run it against local Chromium and Firefox");
        return;
    }
    let only: Vec<&str> = args
        .iter()
        .skip(1)
        .map(String::as_str)
        .filter(|arg| matches!(*arg, "chromium" | "firefox"))
        .collect();
    for (name, backend) in [
        ("chromium", BackendKind::ChromiumCdp),
        ("firefox", BackendKind::FirefoxBidi),
    ] {
        if only.is_empty() || only.contains(&name) {
            println!("webauthn_live: {name}");
            run_engine(backend);
            println!("webauthn_live: {name} passed");
        }
    }
}

fn run_engine(backend: BackendKind) {
    let temp = tempfile::tempdir().expect("temp");
    let state = temp.path().join("authenticator");
    std::fs::create_dir_all(&state).expect("state dir");
    let profiles = std::env::var_os("HORIZON_LIVE_PROFILE_ROOT").map_or_else(
        || temp.path().join("profiles"),
        |root| Path::new(&root).join(uuid_like()),
    );
    let rp = RelyingParty::start();
    let exe = std::env::current_exe().expect("current exe");
    let frame_slot = Arc::new(FrameSlot::new());
    let session = LiveSession(Some(
        start_session(BrowserSessionConfig {
            browser: BrowserConfig {
                backend,
                profile_root: Some(profiles.clone()),
                webauthn_command: vec![
                    exe.display().to_string(),
                    FAKE_AUTHENTICATOR_FLAG.to_string(),
                    state.display().to_string(),
                ],
                ..BrowserConfig::default()
            },
            panel_local_id: "webauthn-live".into(),
            initial_url: None,
            width: 900,
            height: 700,
            frame_slot: Arc::clone(&frame_slot),
            coordination: None,
            capture_directory: None,
            video: Arc::new(horizon_browser::VideoCaptureHandle::default()),
            remote: None,
        })
        .expect("start session"),
    ));
    wait_ready(&session);

    navigate(&session, &rp.url("/create"));
    let created = rp.wait_report("create", REPORT_TIMEOUT, &session);
    assert_eq!(created.verdict, "verified", "create: {created:?}");
    let log = std::fs::read_to_string(state.join("calls.log")).unwrap_or_default();
    assert!(
        log.lines().next().is_some_and(|line| line.ends_with(" required")),
        "an unknown residentKey must fall back to requireResidentKey: {log}"
    );

    navigate(&session, &rp.url("/get"));
    let asserted = rp.wait_report("get", REPORT_TIMEOUT, &session);
    assert_eq!(asserted.verdict, "verified", "get: {asserted:?}");
    assert_eq!(calls(&state), 2);

    navigate(&session, &rp.url("/refusals"));
    for (scenario, expected) in [
        ("foreign_rp_id", "SecurityError"),
        ("empty_challenge", "TypeError"),
        ("malformed_options", "TypeError"),
        ("aborted_by_options", "AbortError"),
        ("conditional_available", "false"),
        ("conditional", "AbortError"),
        ("conditional_create", "AbortError"),
        ("cross_origin_iframe", "NotAllowedError"),
    ] {
        let report = rp.wait_report(scenario, REPORT_TIMEOUT, &session);
        assert_eq!(report.verdict, expected, "{scenario}: {report:?}");
        assert_eq!(calls(&state), 2, "{scenario} reached the command");
    }
    navigate(&session, &rp.insecure_url("/insecure"));
    let insecure = rp.wait_report("insecure_origin", REPORT_TIMEOUT, &session);
    assert_eq!(insecure.verdict, "SecurityError", "insecure: {insecure:?}");
    assert_eq!(calls(&state), 2, "insecure_origin reached the command");

    check_hanging_requests(&session, &rp, &frame_slot, &state);

    drop(session);
    if std::env::var_os("HORIZON_LIVE_PROFILE_ROOT").is_some() {
        let _ = std::fs::remove_dir_all(profiles);
    }
}

/// A navigation start and an abort each kill a command that waits.
fn check_hanging_requests(session: &BrowserSession, rp: &RelyingParty, frame_slot: &FrameSlot, state: &Path) {
    std::fs::write(state.join("mode"), "hang").expect("mode");
    let pid_file = state.join("hang.pid");
    navigate(session, &rp.url("/stalled-navigation"));
    wait_for(session, "the hanging command to start", || pid_file.exists());
    wait_for(session, "the panel to show a waiting passkey request", || {
        frame_slot.passkey_pending()
    });
    let pid = std::fs::read_to_string(&pid_file).expect("pid");
    rp.release_navigation();
    let stalled = rp.wait_report("stalled_navigation", REPORT_TIMEOUT, session);
    assert_eq!(
        stalled.verdict, "NotAllowedError",
        "a navigation that stops before it commits must settle the promise"
    );
    wait_for(session, "a navigation start to kill the command", || {
        !process_alive(pid.trim())
    });
    wait_for(session, "the waiting indicator to clear", || {
        !frame_slot.passkey_pending()
    });
    assert_eq!(calls(state), 3);
    std::fs::remove_file(&pid_file).expect("reset pid");

    navigate(session, &rp.url("/abort"));
    wait_for(session, "the hanging command to start", || pid_file.exists());
    wait_for(session, "the panel to show a waiting passkey request", || {
        frame_slot.passkey_pending()
    });
    let pid = std::fs::read_to_string(&pid_file).expect("pid");
    let same_document = rp.wait_report("same_document", REPORT_TIMEOUT, session);
    assert_eq!(same_document.verdict, "changed");
    thread::sleep(Duration::from_millis(500));
    drain(session);
    assert!(
        process_alive(pid.trim()),
        "a same-document navigation must not end the request"
    );
    assert!(frame_slot.passkey_pending());
    rp.release_abort();
    let aborted = rp.wait_report("abort", REPORT_TIMEOUT, session);
    assert_eq!(aborted.verdict, "AbortError", "abort: {aborted:?}");
    wait_for(session, "the aborted command to be killed", || {
        !process_alive(pid.trim())
    });
    wait_for(session, "the waiting indicator to clear", || {
        !frame_slot.passkey_pending()
    });
    assert_eq!(calls(state), 4);
}

/// Owns the browser session so a failing assertion still stops the browser:
/// the drop during unwinding waits until the driver killed and reaped it.
struct LiveSession(Option<BrowserSession>);

impl std::ops::Deref for LiveSession {
    type Target = BrowserSession;

    fn deref(&self) -> &BrowserSession {
        self.0.as_ref().expect("live session")
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        let Some(session) = self.0.take() else {
            return;
        };
        let shutdown = session.shutdown_signal();
        if !(shutdown.wait(Duration::from_secs(15)) || shutdown.force_cleanup(Duration::from_secs(5))) {
            eprintln!("webauthn_live: the browser did not stop; it may still run");
        }
    }
}

fn wait_ready(session: &BrowserSession) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        match session.event_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(BrowserEvent::Ready) => return,
            Ok(BrowserEvent::Stopped { .. }) => panic!("browser stopped during startup"),
            Ok(BrowserEvent::Warning(warning)) => println!("warning: {warning}"),
            _ => {}
        }
    }
    panic!("browser did not become ready");
}

fn navigate(session: &BrowserSession, url: &str) {
    assert!(
        session.send(BrowserCommand::Navigate(url.to_string())),
        "navigate {url}"
    );
}

fn wait_for(session: &BrowserSession, what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + REPORT_TIMEOUT;
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        drain(session);
        thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for {what}");
}

pub(crate) fn drain(session: &BrowserSession) {
    while let Ok(event) = session.event_rx.try_recv() {
        if let BrowserEvent::Warning(warning) = event {
            println!("warning: {warning}");
        }
    }
}

fn calls(state: &Path) -> usize {
    std::fs::read_to_string(state.join("calls.log")).map_or(0, |log| log.lines().count())
}

fn process_alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn uuid_like() -> String {
    format!(
        "webauthn-live-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos())
    )
}
