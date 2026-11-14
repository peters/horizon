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
use crate::session::PageIsolation;
use crate::{AutomationDisclosurePolicy, BrowserCommand, BrowserConfig, BrowserEvent, DevtoolsEndpoint};

use super::super::{BrowserEventSender, BrowserEventWake, BrowserSessionConfig, CommittedUrl};
use super::initialize_driver;

/// One loopback CDP browser that holds a sibling page of its person, so a
/// session that adopts one has something to adopt.
fn borrowed_browser() -> (DevtoolsEndpoint, std::thread::JoinHandle<Vec<Value>>) {
    let browser = mock_browser(|_command| None);
    (browser.endpoint, browser.server)
}

/// A mock CDP browser on a loopback port, which answers the startup commands of
/// a session and keeps every command that it received.
struct MockBrowser {
    endpoint: DevtoolsEndpoint,
    server: std::thread::JoinHandle<Vec<Value>>,
}

impl MockBrowser {
    /// The commands that the session sent, after the link closed.
    fn commands(self) -> Vec<Value> {
        self.server.join().expect("the mock browser")
    }
}

const BROWSER_PATH: &str = "/devtools/browser/ours";

/// A mock CDP browser that answers `override_answer` for a command when it
/// answers `Some`, and the ordinary answers of a borrowed browser otherwise.
fn mock_browser(override_answer: impl Fn(&Value) -> Option<Value> + Send + 'static) -> MockBrowser {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the mock browser");
    let port = listener.local_addr().expect("mock address").port();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept the mock browser");
        let mut socket = tungstenite::accept(stream).expect("mock handshake");
        let mut commands = Vec::new();
        while let Ok(Message::Text(text)) = socket.read() {
            let command: Value = serde_json::from_str(&text).expect("command JSON");
            let result = override_answer(&command).unwrap_or_else(|| match command["method"].as_str() {
                Some("Target.createBrowserContext") => json!({ "browserContextId": "ctx-private" }),
                Some("Target.createTarget") => json!({ "targetId": "ours" }),
                Some("Target.attachToTarget") => json!({ "sessionId": "ours-session" }),
                Some("Target.closeTarget") => json!({ "success": true }),
                Some("Target.getBrowserContexts") => json!({ "browserContextIds": [] }),
                Some("Target.getTargets") => json!({ "targetInfos": [
                    { "targetId": "sibling", "type": "page", "attached": false }
                ] }),
                _ => json!({}),
            });
            commands.push(command.clone());
            let reply = json!({ "id": command["id"], "result": result });
            socket.send(Message::Text(reply.to_string().into())).expect("respond");
        }
        commands
    });
    let endpoint = DevtoolsEndpoint::from_active_port_file(&format!("41000\n{BROWSER_PATH}"), port)
        .expect("the endpoint of the mock browser");
    MockBrowser { endpoint, server }
}

/// One answer that a mock browser sends instead of the ordinary one.
fn failing_answer(method: &'static str) -> impl Fn(&Value) -> Option<Value> + Send + 'static {
    move |command| (command["method"].as_str() == Some(method)).then(|| json!({}))
}

fn attached_config(endpoint: DevtoolsEndpoint, frame_slot: &Arc<FrameSlot>) -> BrowserSessionConfig {
    BrowserSessionConfig {
        attach: Some(endpoint),
        resume_target: None,
        resume_context: None,
        page_isolation: crate::PageIsolationReport::default(),
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

/// A private page of a browser that another process owns is a page of an
/// isolated browser context of that browser: it is made in a context of its own,
/// so it reads none of the cookies of the profile of that machine.
#[test]
fn a_private_page_of_a_borrowed_browser_is_made_in_an_isolated_context() {
    let (endpoint, server) = borrowed_browser();
    let frame_slot = Arc::new(FrameSlot::new());
    let mut config = attached_config(endpoint, &frame_slot);
    config.browser.isolated_page = true;
    let (event_tx, _events) = event_sender();
    let stop_requested = AtomicBool::new(false);
    let control = ChromeProcessControl::default();

    let connection = initialize_driver(&config, &event_tx, &stop_requested, &control, None)
        .expect("a private page of the borrowed browser");
    assert_eq!(connection.target_id, "ours");
    assert_eq!(
        connection.chrome.context().as_deref(),
        Some("ctx-private"),
        "the page carries the context that its close disposes"
    );
    drop(connection);
    let commands = server.join().expect("the mock browser");

    let position = |method: &str| {
        commands
            .iter()
            .position(|command| command["method"] == method)
            .unwrap_or_else(|| panic!("the session sends {method}"))
    };
    let context = position("Target.createBrowserContext");
    let created = position("Target.createTarget");
    assert!(context < created, "the context comes before the page that it carries");
    assert_eq!(
        commands[created]["params"]["browserContextId"], "ctx-private",
        "the page of a private panel is a page of that context: {}",
        commands[created]
    );
    assert!(
        !commands.iter().any(|command| command["method"] == "Target.getTargets"),
        "a private page adopts no page of the profile of that machine"
    );
}

/// A private page of a browser that another process owns reports the fact that
/// allows a panel to call it private: a page of a browser context of its own.
#[test]
fn a_private_page_of_a_borrowed_browser_reports_its_isolated_context() {
    let browser = mock_browser(|_command| None);
    let frame_slot = Arc::new(FrameSlot::new());
    let mut config = attached_config(browser.endpoint.clone(), &frame_slot);
    config.browser.isolated_page = true;
    let (event_tx, _events) = event_sender();
    let stop_requested = AtomicBool::new(false);
    let control = ChromeProcessControl::default();

    let connection = initialize_driver(&config, &event_tx, &stop_requested, &control, None)
        .expect("a private page of the borrowed browser");

    assert_eq!(
        config.page_isolation.snapshot(),
        Some(PageIsolation::IsolatedContext),
        "the session proves the context that makes the page private"
    );
    assert!(config.page_isolation.isolation().keeps_nothing());
    drop(connection);
    let _ = browser.commands();
}

/// A shared page of a borrowed browser is a page of its default context, so it
/// sends no browser-context command at all and claims nothing about privacy.
#[test]
fn a_shared_page_of_a_borrowed_browser_makes_no_browser_context() {
    let browser = mock_browser(|_command| None);
    let frame_slot = Arc::new(FrameSlot::new());
    let config = attached_config(browser.endpoint.clone(), &frame_slot);
    let (event_tx, _events) = event_sender();
    let stop_requested = AtomicBool::new(false);
    let control = ChromeProcessControl::default();

    let connection =
        initialize_driver(&config, &event_tx, &stop_requested, &control, None).expect("a page of the borrowed browser");
    assert_eq!(
        config.page_isolation.snapshot(),
        Some(PageIsolation::Shared),
        "a page of the default context keeps the profile that it named"
    );
    drop(connection);
    let commands = browser.commands();
    for forbidden in [
        "Target.createBrowserContext",
        "Target.disposeBrowserContext",
        "Target.getBrowserContexts",
    ] {
        assert!(
            !commands.iter().any(|command| command["method"] == forbidden),
            "a shared page sends no {forbidden}: {commands:?}"
        );
    }
    assert!(
        !commands
            .iter()
            .any(|command| command["method"] == "Target.createTarget"
                && command["params"].get("browserContextId").is_some()),
        "the page of a shared panel carries no context: {commands:?}"
    );
}

/// A private page that comes back after a lost link is the page of the link
/// before inside its own context, and the session sends no new context and
/// adopts no page of the default context.
#[test]
fn a_private_page_of_a_lost_link_comes_back_inside_its_own_context() {
    let browser = mock_browser(|command| match command["method"].as_str() {
        Some("Target.getBrowserContexts") => Some(json!({ "browserContextIds": ["ctx-private"] })),
        Some("Target.getTargets") => Some(json!({ "targetInfos": [
            { "targetId": "a-page-of-the-profile", "type": "page", "attached": false },
            { "targetId": "its-page", "type": "page", "browserContextId": "ctx-private" }
        ] })),
        _ => None,
    });
    let frame_slot = Arc::new(FrameSlot::new());
    let mut config = attached_config(browser.endpoint.clone(), &frame_slot);
    config.browser.isolated_page = true;
    config.resume_target = Some("its-page".to_string());
    config.resume_context = Some("ctx-private".to_string());
    let (event_tx, _events) = event_sender();
    let stop_requested = AtomicBool::new(false);
    let control = ChromeProcessControl::default();

    let connection = initialize_driver(&config, &event_tx, &stop_requested, &control, None)
        .expect("the private page of the lost link");
    assert_eq!(connection.target_id, "its-page");
    assert!(connection.resumed_page, "the page of the link before came back");
    assert_eq!(connection.chrome.context().as_deref(), Some("ctx-private"));
    assert_eq!(config.page_isolation.snapshot(), Some(PageIsolation::IsolatedContext));
    drop(connection);
    let commands = browser.commands();
    assert!(
        !commands
            .iter()
            .any(|command| command["method"] == "Target.createBrowserContext"),
        "the context of the page came back, so no context is made: {commands:?}"
    );
    let attached = commands
        .iter()
        .find(|command| command["method"] == "Target.attachToTarget")
        .expect("the session attaches to its page");
    assert_eq!(attached["params"]["targetId"], "its-page");
}

/// A private page whose context the browser no longer holds gets a new isolated
/// context, and never a page of the default context.
#[test]
fn a_private_page_of_a_browser_that_restarted_gets_a_new_context() {
    let browser = mock_browser(|command| match command["method"].as_str() {
        Some("Target.getBrowserContexts") => Some(json!({ "browserContextIds": [] })),
        Some("Target.getTargets") => Some(json!({ "targetInfos": [
            { "targetId": "its-page", "type": "page", "attached": false }
        ] })),
        Some("Target.createBrowserContext") => Some(json!({ "browserContextId": "ctx-new" })),
        Some("Target.createTarget") => Some(json!({ "targetId": "its-new-page" })),
        _ => None,
    });
    let frame_slot = Arc::new(FrameSlot::new());
    let mut config = attached_config(browser.endpoint.clone(), &frame_slot);
    config.browser.isolated_page = true;
    config.resume_target = Some("its-page".to_string());
    config.resume_context = Some("ctx-gone".to_string());
    let (event_tx, _events) = event_sender();
    let stop_requested = AtomicBool::new(false);
    let control = ChromeProcessControl::default();

    let connection = initialize_driver(&config, &event_tx, &stop_requested, &control, None)
        .expect("a new private page of that browser");
    assert_eq!(connection.target_id, "its-new-page");
    assert!(!connection.resumed_page, "the page of the link before is gone");
    assert_eq!(
        connection.chrome.context().as_deref(),
        Some("ctx-new"),
        "the new private page is a page of a context of its own"
    );
    drop(connection);
    let commands = browser.commands();
    let created = commands
        .iter()
        .position(|command| command["method"] == "Target.createTarget")
        .expect("the session makes a page");
    assert_eq!(
        commands[created]["params"]["browserContextId"], "ctx-new",
        "the new page is a page of the new context: {}",
        commands[created]
    );
    assert!(
        !commands
            .iter()
            .any(|command| command["method"] == "Target.attachToTarget" && command["params"]["targetId"] == "its-page"),
        "a page of the default context of that browser is never adopted: {commands:?}"
    );
}

/// The close of a private page disposes its browser context, so nothing of the
/// page stays in the browser of that machine.
#[test]
fn the_close_of_a_private_page_disposes_its_browser_context() {
    let browser = mock_browser(|_command| None);
    let frame_slot = Arc::new(FrameSlot::new());
    let mut config = attached_config(browser.endpoint.clone(), &frame_slot);
    config.browser.isolated_page = true;
    let (event_tx, _events) = event_sender();
    let stop_requested = AtomicBool::new(false);
    let control = ChromeProcessControl::default();

    let mut connection = initialize_driver(&config, &event_tx, &stop_requested, &control, None)
        .expect("a private page of the borrowed browser");
    let target = connection.target_id.clone();
    connection.chrome.close_page(&mut connection.link, &target);
    drop(connection);
    let commands = browser.commands();
    let closed = commands
        .iter()
        .position(|command| command["method"] == "Target.closeTarget")
        .expect("the page closes");
    let disposed = commands
        .iter()
        .position(|command| command["method"] == "Target.disposeBrowserContext")
        .expect("the context of the page goes with it");
    assert!(closed < disposed, "the page goes before its context: {commands:?}");
    assert_eq!(commands[disposed]["params"]["browserContextId"], "ctx-private");
    assert!(
        !commands.iter().any(|command| command["method"] == "Browser.close"),
        "the browser belongs to another process: {commands:?}"
    );
}

/// A browser that cannot give a private panel a context of its own gets a
/// refusal, and the session opens no page of the shared profile in its place.
#[test]
fn a_browser_that_refuses_a_context_opens_no_page_of_the_shared_profile() {
    let browser = mock_browser(failing_answer("Target.createBrowserContext"));
    let frame_slot = Arc::new(FrameSlot::new());
    let mut config = attached_config(browser.endpoint.clone(), &frame_slot);
    config.browser.isolated_page = true;
    let (event_tx, events) = event_sender();
    let stop_requested = AtomicBool::new(false);
    let control = ChromeProcessControl::default();

    let connection = initialize_driver(&config, &event_tx, &stop_requested, &control, None);

    assert!(
        connection.is_none(),
        "a private page that cannot be isolated is no session"
    );
    assert_eq!(
        config.page_isolation.snapshot(),
        Some(PageIsolation::Shared),
        "the session proves nothing, so nothing calls the page private"
    );
    let seen: Vec<BrowserEvent> = events.try_iter().collect();
    let refusal = seen
        .iter()
        .find_map(|event| match event {
            BrowserEvent::Warning(message) => Some(message.clone()),
            _ => None,
        })
        .expect("the panel reads why its private page could not be made");
    assert!(refusal.starts_with(crate::PRIVATE_PAGE_UNAVAILABLE), "{refusal}");
    assert!(refusal.contains("opened no page in the shared profile"), "{refusal}");
    assert!(
        seen.iter().any(|event| matches!(event, BrowserEvent::Stopped { .. })),
        "the session stops: {seen:?}"
    );
    let commands = browser.commands();
    assert!(
        !commands
            .iter()
            .any(|command| command["method"] == "Target.createTarget"),
        "no page of the shared profile is opened in its place: {commands:?}"
    );
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

    let downloads_note = connection.downloads_note.clone();
    assert!(!connection.downloads_reporting);
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
        !seen.iter().any(|event| matches!(event, BrowserEvent::Warning(_))),
        "a downloads setting that does not apply is no failure of the session: {seen:?}"
    );
    assert!(
        !seen.iter().any(|event| matches!(event, BrowserEvent::Stopped { .. })),
        "the session of a borrowed browser starts without an exit: {seen:?}"
    );
    assert_eq!(
        downloads_note.as_deref(),
        Some(crate::ATTACHED_DOWNLOADS_GAP),
        "the reason belongs to the downloads answer of this panel"
    );
}

#[test]
fn a_downloads_refusal_is_an_answer_to_the_downloads_question_and_not_a_ready_page_error() {
    // The order that showed a fatal error for a page that was fine: the
    // session reached Ready, and a downloads warning arrived after it (a second
    // panel of the same browser host). The warning replaced Ready with Error,
    // so the body showed the downloads sentence with Retry instead of the page.
    let (endpoint, server) = borrowed_browser();
    let frame_slot = Arc::new(FrameSlot::new());
    let config = attached_config(endpoint, &frame_slot);
    let (event_tx, events) = event_sender();
    let stop_requested = AtomicBool::new(false);
    let control = ChromeProcessControl::default();
    let mut connection = initialize_driver(&config, &event_tx, &stop_requested, &control, None)
        .expect("a driver on the borrowed browser");

    let mut downloads = crate::downloads::DownloadState::new("/tmp/panel-downloads", false);
    if let Some(note) = connection.downloads_note.take() {
        downloads = downloads.with_note(note);
    }
    let list = downloads.list();
    assert!(!list.reporting, "the answer says that it reports nothing");
    assert_eq!(
        list.note.as_deref(),
        Some(crate::ATTACHED_DOWNLOADS_GAP),
        "the downloads answer carries the reason, and no event does"
    );
    let seen: Vec<BrowserEvent> = events.try_iter().collect();
    assert!(
        !seen.iter().any(|event| matches!(event, BrowserEvent::Warning(_))),
        "the page of that browser is fine, so nothing reports a failure: {seen:?}"
    );

    connection
        .chrome
        .close_page(&mut connection.link, &connection.target_id);
    drop(connection);
    let _ = server.join();
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
    let root = crate::profile::effective_profile_root(&command, &crate::paths::profile_root());
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

#[cfg(target_os = "macos")]
#[path = "attach_tests/private_judge.rs"]
mod private_judge;
