//! A mock browser that another process owns, and one live Chromium that this
//! file starts itself.

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::Message;

use crate::frames::FrameSlot;
use crate::process::ChromeProcessControl;
use crate::{AutomationDisclosurePolicy, BrowserCommand, BrowserConfig, BrowserEvent, DevtoolsEndpoint};

use super::super::{BrowserEventSender, BrowserEventWake, BrowserSessionConfig, CommittedUrl};
use super::initialize_driver;

const BROWSER_PATH: &str = "/devtools/browser/ours";

/// One loopback CDP browser that holds a sibling page of its person, so a
/// session that adopts one has something to adopt.
fn borrowed_browser() -> (DevtoolsEndpoint, std::thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the mock browser");
    let port = listener.local_addr().expect("mock address").port();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept the mock browser");
        let mut socket = tungstenite::accept(stream).expect("mock handshake");
        let mut commands = Vec::new();
        while let Ok(Message::Text(text)) = socket.read() {
            let command: Value = serde_json::from_str(&text).expect("command JSON");
            let result = match command["method"].as_str() {
                Some("Target.createTarget") => json!({ "targetId": "ours" }),
                Some("Target.attachToTarget") => json!({ "sessionId": "ours-session" }),
                Some("Target.closeTarget") => json!({ "success": true }),
                Some("Target.getTargets") => json!({ "targetInfos": [
                    { "targetId": "sibling", "type": "page", "attached": false }
                ] }),
                _ => json!({}),
            };
            commands.push(command.clone());
            let reply = json!({ "id": command["id"], "result": result });
            socket.send(Message::Text(reply.to_string().into())).expect("respond");
        }
        commands
    });
    let endpoint = DevtoolsEndpoint::from_active_port_file(&format!("41000\n{BROWSER_PATH}"), port)
        .expect("the endpoint of the mock browser");
    (endpoint, server)
}

fn attached_config(endpoint: DevtoolsEndpoint, frame_slot: &Arc<FrameSlot>) -> BrowserSessionConfig {
    BrowserSessionConfig {
        attach: Some(endpoint),
        resume_target: None,
        browser: BrowserConfig {
            automation_disclosure: AutomationDisclosurePolicy::BrowserDefault,
            profile_root: Some(std::path::PathBuf::from("/nonexistent/horizon-attach-profiles")),
            ..BrowserConfig::default()
        },
        panel_local_id: "attach-panel".to_string(),
        initial_url: None,
        width: 1280,
        height: 800,
        frame_slot: Arc::clone(frame_slot),
        coordination: None,
        capture_directory: None,
        video: Arc::new(crate::VideoCaptureHandle::default()),
        remote: None,
    }
}

fn event_sender() -> (BrowserEventSender, mpsc::Receiver<BrowserEvent>) {
    let (tx, rx) = mpsc::channel();
    (
        BrowserEventSender {
            tx,
            wake: BrowserEventWake::default(),
            committed_url: CommittedUrl::default(),
        },
        rx,
    )
}

#[test]
fn an_attached_session_drives_its_own_page_and_never_stops_the_browser() {
    let (endpoint, server) = borrowed_browser();
    let frame_slot = Arc::new(FrameSlot::new());
    let config = attached_config(endpoint, &frame_slot);
    let (event_tx, events) = event_sender();
    let stop_requested = AtomicBool::new(false);
    let control = ChromeProcessControl::default();

    let mut connection = initialize_driver(&config, &event_tx, &stop_requested, &control, None)
        .expect("a driver on the borrowed browser");
    assert_eq!(connection.target_id, "ours");
    assert_eq!(connection.session_id, "ours-session");

    connection
        .chrome
        .close_page(&mut connection.link, &connection.target_id);
    connection
        .chrome
        .close_page(&mut connection.link, &connection.target_id);
    drop(connection);
    let commands = server.join().expect("the mock browser");

    let methods: Vec<&str> = commands
        .iter()
        .filter_map(|command| command["method"].as_str())
        .collect();
    let created = commands
        .iter()
        .find(|command| command["method"] == "Target.createTarget")
        .expect("the session creates its own page");
    assert_eq!(created["params"]["url"], "about:blank");
    assert!(
        created["params"].get("newWindow").is_none(),
        "a page of the person's browser is the polite choice: {created}"
    );
    assert!(
        !methods.contains(&"Target.getTargets"),
        "an attached session asks for the pages of a sibling: {methods:?}"
    );
    assert!(
        !methods.contains(&"Browser.setDownloadBehavior"),
        "a download folder of this computer means nothing on that machine: {methods:?}"
    );
    assert!(
        !methods.contains(&"Browser.close"),
        "the browser belongs to another process: {methods:?}"
    );
    let attached: Vec<&Value> = commands
        .iter()
        .filter(|command| command["method"] == "Target.attachToTarget")
        .collect();
    assert_eq!(attached.len(), 1, "{methods:?}");
    assert_eq!(attached[0]["params"]["targetId"], "ours");
    let closed: Vec<&Value> = commands
        .iter()
        .filter(|command| command["method"] == "Target.closeTarget")
        .collect();
    assert_eq!(closed.len(), 1, "one target closes exactly once: {closed:?}");
    assert_eq!(closed[0]["params"]["targetId"], "ours");
    assert!(
        !commands
            .iter()
            .any(|command| command["params"]["targetId"] == "sibling"),
        "the page of a sibling is left alone: {commands:?}"
    );

    let seen: Vec<BrowserEvent> = events.try_iter().collect();
    assert!(
        seen.iter()
            .any(|event| matches!(event, BrowserEvent::Warning(note) if note == crate::ATTACHED_DOWNLOADS_GAP)),
        "the panel states that it reports no download: {seen:?}"
    );
    assert!(
        !seen.iter().any(|event| matches!(event, BrowserEvent::Stopped { .. })),
        "the session of a borrowed browser starts without an exit: {seen:?}"
    );
}

#[test]
fn an_attached_session_that_cannot_reach_its_browser_stops_with_a_reason() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a port");
    let port = listener.local_addr().expect("address").port();
    drop(listener);
    let endpoint = DevtoolsEndpoint::from_active_port_file(&format!("41000\n{BROWSER_PATH}"), port)
        .expect("the endpoint of a port that nothing listens on");
    let frame_slot = Arc::new(FrameSlot::new());
    let config = attached_config(endpoint, &frame_slot);
    let (event_tx, events) = event_sender();
    let stop_requested = AtomicBool::new(false);
    let control = ChromeProcessControl::default();

    let connection = initialize_driver(&config, &event_tx, &stop_requested, &control, None);

    assert!(connection.is_none(), "a link to nothing is no session");
    let seen: Vec<BrowserEvent> = events.try_iter().collect();
    assert!(
        seen.iter()
            .any(|event| matches!(event, BrowserEvent::Warning(reason) if reason.starts_with("CDP connect failed"))),
        "{seen:?}"
    );
    assert!(
        seen.iter().any(|event| matches!(event, BrowserEvent::Stopped { .. })),
        "{seen:?}"
    );
}

#[test]
fn a_file_of_this_computer_is_refused_by_a_session_of_another_machine() {
    let (endpoint, server) = borrowed_browser();
    let frame_slot = Arc::new(FrameSlot::new());
    let config = attached_config(endpoint, &frame_slot);
    let (event_tx, _events) = event_sender();
    let stop_requested = AtomicBool::new(false);
    let control = ChromeProcessControl::default();
    let mut connection = initialize_driver(&config, &event_tx, &stop_requested, &control, None)
        .expect("a driver on the borrowed browser");
    let state = super::super::DriverState::new(
        &config,
        &connection.ws_url,
        connection.native_user_agent_metadata.take(),
        Arc::new(AtomicBool::new(false)),
    );
    let refusal = state.local_file_refusal();

    connection
        .chrome
        .close_page(&mut connection.link, &connection.target_id);
    drop(connection);
    let _ = server.join();

    let refusal = refusal.expect("a session of another machine refuses a local file");
    assert_eq!(refusal.code, "remote_file_upload_unsupported");
    assert!(refusal.message.contains("another machine"), "{refusal:?}");

    let mut local = config.clone();
    local.attach = None;
    let local_state =
        super::super::DriverState::new(&local, "ws://127.0.0.1:1/", None, Arc::new(AtomicBool::new(false)));
    assert!(
        local_state.local_file_refusal().is_none(),
        "a local browser takes a file of this computer"
    );
}

/// One real Chromium that this test starts in a temporary profile, so the attach
/// path is proven against a browser rather than a mock. It is ignored because it
/// needs a Chromium program on this machine.
#[test]
#[ignore = "starts a real Chromium on this machine"]
fn attach_drives_a_real_local_chromium() {
    let Ok(binary) = crate::process::resolve_binary_or_default(&None) else {
        println!("skipped: this machine has no Chromium program");
        return;
    };
    // A browser of a snap package writes only under its own home rule, which is
    // the root this engine gives its own profiles.
    let command = BrowserConfig {
        command: Some(binary.to_string_lossy().into_owned()),
        ..BrowserConfig::default()
    };
    let root = crate::profile::effective_profile_root(&command, &crate::paths::default_root().join("profiles"));
    let profile = root.join(format!("horizon-attach-live-{}", std::process::id()));
    std::fs::create_dir_all(&profile).expect("create the temporary profile");
    let browser = std::process::Command::new(&binary)
        .args([
            "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1",
            &format!("--user-data-dir={}", profile.display()),
            "--headless=new",
            "--no-first-run",
            "--no-default-browser-check",
            "about:blank",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(browser) = browser else {
        println!("skipped: could not start {}", binary.display());
        return;
    };
    let mut browser = BrowserGuard(browser);
    let endpoint = wait_for_endpoint(&profile.join("DevToolsActivePort"))
        .unwrap_or_else(|| panic!("{} wrote no DevToolsActivePort", binary.display()));

    let frame_slot = Arc::new(FrameSlot::new());
    let config = attached_config(endpoint, &frame_slot);
    let session = crate::start_session(config).expect("start an attached session");
    assert!(session.send(BrowserCommand::Navigate(
        "data:text/html,<h1 id=title>attached chromium</h1>".to_string()
    )));
    let url = wait_for_url(&session, Duration::from_secs(20), "data:");
    assert!(url.is_some(), "the page commits its data: URL");
    let frame = wait_for_frame(&session, &frame_slot, Duration::from_secs(20));
    assert!(frame, "the session publishes a screencast frame of that page");

    assert!(session.send(BrowserCommand::Input(crate::BrowserInput::MousePress {
        x: 40.0,
        y: 30.0,
        button: crate::BrowserButton::Left,
        click_count: 1,
        buttons: 1,
        modifiers: crate::BrowserModifiers::none(),
    })));
    assert!(session.send(BrowserCommand::Input(crate::BrowserInput::MouseRelease {
        x: 40.0,
        y: 30.0,
        button: crate::BrowserButton::Left,
        click_count: 1,
        buttons: 0,
        modifiers: crate::BrowserModifiers::none(),
    })));
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !session
            .event_rx
            .try_iter()
            .any(|event| matches!(event, BrowserEvent::Stopped { .. })),
        "the session of a borrowed browser survives one click"
    );

    assert!(session.send(BrowserCommand::Stop));
    let _completion = session.shutdown_signal();
    // The browser must outlive the session: that is the whole point of attach.
    assert!(
        browser.0.try_wait().expect("state of the borrowed browser").is_none(),
        "the borrowed browser stays up when the session stops"
    );
    browser.kill();
    let _ = std::fs::remove_dir_all(&profile);
}

struct BrowserGuard(std::process::Child);

impl BrowserGuard {
    fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for BrowserGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

fn wait_for_endpoint(active: &std::path::Path) -> Option<DevtoolsEndpoint> {
    let port = wait_for_port(active)?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(endpoint) = DevtoolsEndpoint::from_port(port) {
            return Some(endpoint);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_port(active: &std::path::Path) -> Option<u16> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(active)
            && let Some(port) = text.lines().next().and_then(|line| line.trim().parse().ok())
        {
            return Some(port);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

fn wait_for_url(session: &crate::BrowserSession, timeout: Duration, prefix: &str) -> Option<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        while let Ok(event) = session.event_rx.try_recv() {
            if let BrowserEvent::UrlChanged(url) = event
                && url.starts_with(prefix)
            {
                return Some(url);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

fn wait_for_frame(session: &crate::BrowserSession, frame_slot: &FrameSlot, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let _ = session.event_rx.try_recv();
        if frame_slot.latest().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// One loopback CDP browser whose page list is `targets`, and whose
/// `Target.attachToTarget` answers `session`.
fn browser_with_targets(targets: &[&str], session: &str) -> (DevtoolsEndpoint, std::thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the mock browser");
    let port = listener.local_addr().expect("mock address").port();
    let target_infos: Vec<Value> = targets
        .iter()
        .map(|target| json!({ "targetId": target, "type": "page", "attached": false }))
        .collect();
    let session = session.to_string();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept the mock browser");
        let mut socket = tungstenite::accept(stream).expect("mock handshake");
        let mut commands = Vec::new();
        while let Ok(Message::Text(text)) = socket.read() {
            let command: Value = serde_json::from_str(&text).expect("command JSON");
            let result = match command["method"].as_str() {
                Some("Target.getTargets") => json!({ "targetInfos": target_infos }),
                Some("Target.attachToTarget") => json!({ "sessionId": session }),
                Some("Target.createTarget") => json!({ "targetId": "a-new-page" }),
                Some("Target.closeTarget") => json!({ "success": true }),
                _ => json!({}),
            };
            commands.push(command.clone());
            let reply = json!({ "id": command["id"], "result": result });
            socket.send(Message::Text(reply.to_string().into())).expect("respond");
        }
        commands
    });
    let endpoint = DevtoolsEndpoint::from_active_port_file(&format!("41000\n{BROWSER_PATH}"), port)
        .expect("the endpoint of the mock browser");
    (endpoint, server)
}

fn attaching(config: &mut BrowserSessionConfig, endpoint: DevtoolsEndpoint) {
    config.attach = Some(endpoint);
}

#[test]
fn a_lost_link_adopts_the_same_page_and_creates_no_target() {
    let (endpoint, server) = browser_with_targets(&["the-old-page"], "old-session");
    let frame_slot = Arc::new(FrameSlot::new());
    let mut config = attached_config(
        DevtoolsEndpoint::from_active_port_file(&format!("41000\n{BROWSER_PATH}"), 1).expect("a placeholder endpoint"),
        &frame_slot,
    );
    attaching(&mut config, endpoint);
    config.resume_target = Some("the-old-page".to_string());
    let (event_tx, _events) = event_sender();
    let stop_requested = AtomicBool::new(false);
    let control = ChromeProcessControl::default();

    let mut connection = initialize_driver(&config, &event_tx, &stop_requested, &control, None)
        .expect("a driver on the page the panel had");
    assert_eq!(connection.target_id, "the-old-page");
    assert_eq!(connection.session_id, "old-session");
    assert!(connection.resumed_page, "the page is the one the panel had");
    connection
        .chrome
        .close_page(&mut connection.link, &connection.target_id);
    drop(connection);
    let commands = server.join().expect("the mock browser");

    let methods: Vec<&str> = commands
        .iter()
        .filter_map(|command| command["method"].as_str())
        .collect();
    assert!(
        !methods.contains(&"Target.createTarget"),
        "the same page comes back, so no page is opened: {methods:?}"
    );
    let attached: Vec<&Value> = commands
        .iter()
        .filter(|command| command["method"] == "Target.attachToTarget")
        .collect();
    assert_eq!(attached.len(), 1, "{methods:?}");
    assert_eq!(attached[0]["params"]["targetId"], "the-old-page");
    assert_eq!(attached[0]["params"]["flatten"], true);
}

#[test]
fn a_resume_target_that_is_gone_opens_a_new_page_and_says_so() {
    let (endpoint, server) = browser_with_targets(&["a-page-of-the-person"], "new-session");
    let frame_slot = Arc::new(FrameSlot::new());
    let mut config = attached_config(
        DevtoolsEndpoint::from_active_port_file(&format!("41000\n{BROWSER_PATH}"), 1).expect("a placeholder endpoint"),
        &frame_slot,
    );
    attaching(&mut config, endpoint);
    config.resume_target = Some("the-page-of-a-browser-that-restarted".to_string());
    let (event_tx, _events) = event_sender();
    let stop_requested = AtomicBool::new(false);
    let control = ChromeProcessControl::default();

    let mut connection =
        initialize_driver(&config, &event_tx, &stop_requested, &control, None).expect("a driver on a new page");
    assert_eq!(connection.target_id, "a-new-page");
    assert!(
        !connection.resumed_page,
        "a gone page cannot keep its state, so the session reports a new page"
    );
    connection
        .chrome
        .close_page(&mut connection.link, &connection.target_id);
    drop(connection);
    let commands = server.join().expect("the mock browser");

    let methods: Vec<&str> = commands
        .iter()
        .filter_map(|command| command["method"].as_str())
        .collect();
    assert!(
        methods.contains(&"Target.createTarget"),
        "the fallback opens a page of the last address: {methods:?}"
    );
    assert!(
        !commands
            .iter()
            .any(|command| command["params"]["targetId"] == "a-page-of-the-person"),
        "the page of the person is left alone: {commands:?}"
    );
}
