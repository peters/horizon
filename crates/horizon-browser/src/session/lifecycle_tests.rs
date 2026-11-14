use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use serde_json::{Value, json};
use tungstenite::Message;

use super::*;
use crate::BrowserConfig;
use crate::session::{BrowserEventWake, BrowserSessionConfig, CommittedUrl};

struct SetupOutcome {
    ready: bool,
    session: Option<String>,
    commands: Vec<Value>,
    events: Vec<BrowserEvent>,
}

#[test]
fn reattach_exhaustion_ends_the_driver_but_recoverable_attempts_do_not() {
    let config = BrowserSessionConfig {
        attach: None,
        resume_target: None,
        resume_context: None,
        page_isolation: crate::PageIsolationReport::default(),
        browser: BrowserConfig::default(),
        panel_local_id: "reattach-cleanup".into(),
        initial_url: None,
        width: 800,
        height: 600,
        frame_slot: Arc::default(),
        coordination: None,
        capture_directory: None,
        video: Arc::default(),
        remote: None,
    };
    let stop = Arc::new(AtomicBool::new(false));
    let mut state = DriverState::new(&config, "ws://127.0.0.1/test", None, Arc::clone(&stop));
    let (tx, rx) = mpsc::channel();
    let events = BrowserEventSender {
        tx,
        wake: BrowserEventWake::default(),
        committed_url: CommittedUrl::default(),
    };
    for _ in 0..4 {
        state.note_reattach_failure(&events, "unavailable");
        assert!(!stop.load(Ordering::Acquire));
        assert!(state.pending_reattach);
    }
    assert!(rx.try_recv().is_err());
    state.note_reattach_failure(&events, "unavailable");
    assert!(stop.load(Ordering::Acquire));
    assert!(!state.pending_reattach);
    assert!(matches!(rx.try_recv(), Ok(BrowserEvent::Warning(_))));
}

fn run_setup(failure: Option<&'static str>, malformed_id: bool, confirm_state: &str) -> SetupOutcome {
    run_setup_with_browser(
        BrowserConfig {
            headless: false,
            hide_native_window: true,
            automation_disclosure: AutomationDisclosurePolicy::BrowserDefault,
            ..BrowserConfig::default()
        },
        failure,
        malformed_id,
        confirm_state,
    )
}

fn run_setup_with_browser(
    browser: BrowserConfig,
    failure: Option<&'static str>,
    malformed_id: bool,
    confirm_state: &str,
) -> SetupOutcome {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let address = listener.local_addr().expect("mock address");
    let expect_failure = failure.is_some() || malformed_id || !matches!(confirm_state, "minimized" | "delayed");
    let confirm_state = confirm_state.to_owned();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept mock");
        stream.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout");
        let mut socket = tungstenite::accept(stream).expect("mock handshake");
        let mut commands = Vec::new();
        while let Ok(Message::Text(text)) = socket.read() {
            let command: Value = serde_json::from_str(&text).expect("command JSON");
            let method = command["method"].as_str().expect("method");
            let result = match method {
                "Page.getFrameTree" => json!({"frameTree": {"frame": {"id": "frame"}}}),
                "Browser.getWindowForTarget" if malformed_id => json!({"windowId": "invalid"}),
                "Browser.getWindowForTarget" => json!({"windowId": 7}),
                "Browser.getWindowBounds" => {
                    let state = if confirm_state == "delayed" {
                        if commands
                            .iter()
                            .any(|command: &Value| command["method"] == "Browser.getWindowBounds")
                        {
                            "minimized"
                        } else {
                            "normal"
                        }
                    } else {
                        &confirm_state
                    };
                    json!({"bounds": {"windowState": state}})
                }
                _ => json!({}),
            };
            let response = if failure == Some(method) {
                json!({"id": command["id"], "error": {"code": -32000, "message": "mock refusal"}})
            } else {
                json!({"id": command["id"], "result": result})
            };
            commands.push(command);
            socket
                .send(Message::Text(response.to_string().into()))
                .expect("respond");
        }
        commands
    });
    let slot = Arc::new(FrameSlot::default());
    let config = BrowserSessionConfig {
        attach: None,
        resume_target: None,
        resume_context: None,
        page_isolation: crate::PageIsolationReport::default(),
        browser,
        panel_local_id: "native-window-test".to_string(),
        initial_url: Some("https://fixture.invalid/".to_string()),
        width: 1280,
        height: 800,
        frame_slot: Arc::clone(&slot),
        coordination: None,
        capture_directory: None,
        video: Arc::default(),
        remote: None,
    };
    let url = format!("ws://{address}/");
    let mut link = CdpLink::connect(&url).expect("connect mock");
    let mut state = DriverState::new(&config, &url, None, Arc::new(AtomicBool::new(false)));
    let (tx, rx) = mpsc::channel();
    let events = BrowserEventSender {
        tx,
        wake: BrowserEventWake::default(),
        committed_url: CommittedUrl::default(),
    };
    let ready = if expect_failure {
        state.attach_setup(&mut link, &events, &slot, "session", "target", false)
    } else {
        state.session_id = Some("session".to_string());
        state.hide_embedded_native_window(&mut link, &events, &slot, "session", "target")
    };
    drop(link);
    SetupOutcome {
        ready,
        session: state.session_id,
        commands: server.join().expect("mock server"),
        events: rx.try_iter().collect(),
    }
}

fn assert_setup_refused(outcome: &SetupOutcome) {
    assert!(!outcome.ready);
    assert!(outcome.session.is_none());
    assert!(
        outcome
            .commands
            .iter()
            .any(|command| { command["method"] == "Fetch.disable" && command["sessionId"] == "session" })
    );
    assert!(
        outcome
            .events
            .iter()
            .any(|event| matches!(event, BrowserEvent::Warning(_)))
    );
    assert!(!outcome.events.iter().any(|event| matches!(event, BrowserEvent::Ready)));
    assert!(
        !outcome
            .commands
            .iter()
            .any(|command| command["method"] == "Page.navigate")
    );
    assert!(
        !outcome
            .commands
            .iter()
            .any(|command| command["method"] == "Page.startScreencast")
    );
}

#[test]
fn native_window_failures_refuse_navigation_and_ready() {
    for method in [
        "Emulation.setFocusEmulationEnabled",
        "Browser.getWindowForTarget",
        "Browser.setWindowBounds",
        "Browser.getWindowBounds",
    ] {
        assert_setup_refused(&run_setup(Some(method), false, "minimized"));
    }
}

#[test]
fn malformed_window_id_refuses_navigation_and_ready() {
    assert_setup_refused(&run_setup(None, true, "minimized"));
}

#[test]
fn acknowledged_but_unminimized_window_refuses_navigation_and_ready() {
    assert_setup_refused(&run_setup(None, false, "normal"));
}

#[test]
fn native_window_keeps_page_active_before_minimizing_and_confirms_state() {
    let outcome = run_setup(None, false, "minimized");
    assert!(outcome.ready);
    assert_eq!(outcome.commands.len(), 4);
    assert_eq!(outcome.commands[0]["method"], "Emulation.setFocusEmulationEnabled");
    assert_eq!(outcome.commands[0]["params"]["enabled"], true);
    assert_eq!(outcome.commands[0]["sessionId"], "session");
    assert_eq!(outcome.commands[2]["params"]["bounds"]["windowState"], "minimized");
    assert_eq!(outcome.commands[3]["method"], "Browser.getWindowBounds");
}

#[test]
fn native_window_setup_is_opt_in_and_never_overrides_headless() {
    for browser in [
        BrowserConfig::default(),
        BrowserConfig {
            hide_native_window: true,
            ..BrowserConfig::default()
        },
        BrowserConfig {
            headless: false,
            ..BrowserConfig::default()
        },
    ] {
        let outcome = run_setup_with_browser(browser, None, false, "minimized");
        assert!(outcome.ready);
        assert!(outcome.commands.is_empty());
    }
}

#[test]
fn native_window_confirmation_accepts_an_asynchronous_state_change() {
    let outcome = run_setup(None, false, "delayed");
    assert!(outcome.ready);
    assert_eq!(
        outcome
            .commands
            .iter()
            .filter(|command| command["method"] == "Browser.getWindowBounds")
            .count(),
        2
    );
}

/// A mock CDP browser that answers a read only on its second request, and never
/// answers a key press, so a test reads the one more send of a far browser and
/// the refusal of a second send of a change.
struct ReadRetry {
    result: Result<i64, String>,
    read_requests: usize,
    key_requests: usize,
}

fn run_read_retry(attach: bool) -> ReadRetry {
    use crate::DevtoolsEndpoint;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let address = listener.local_addr().expect("mock address");
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept mock");
        stream.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
        let mut socket = tungstenite::accept(stream).expect("mock handshake");
        let (mut read_requests, mut key_requests) = (0, 0);
        while let Ok(Message::Text(text)) = socket.read() {
            let command: Value = serde_json::from_str(&text).expect("command JSON");
            let method = command["method"].as_str().unwrap_or_default();
            if method == "Runtime.evaluate" {
                read_requests += 1;
                if read_requests == 1 {
                    // A far browser that is busy enough to miss the first bound.
                    continue;
                }
            }
            if method == "Input.dispatchKeyEvent" {
                key_requests += 1;
                continue;
            }
            let result = if method == "Runtime.evaluate" {
                json!({ "result": { "value": 7 } })
            } else {
                json!({})
            };
            socket
                .send(Message::Text(
                    json!({ "id": command["id"], "result": result }).to_string().into(),
                ))
                .expect("respond");
        }
        (read_requests, key_requests)
    });
    let config = BrowserSessionConfig {
        attach: attach.then(|| {
            DevtoolsEndpoint::from_active_port_file("9222\n/devtools/browser/6a0c1f4e", 40000)
                .expect("a forwarded endpoint")
        }),
        resume_target: None,
        resume_context: None,
        page_isolation: crate::PageIsolationReport::default(),
        browser: BrowserConfig::default(),
        panel_local_id: "read-retry".into(),
        initial_url: None,
        width: 800,
        height: 600,
        frame_slot: Arc::default(),
        coordination: None,
        capture_directory: None,
        video: Arc::default(),
        remote: None,
    };
    let url = format!("ws://{address}/");
    let mut link = CdpLink::connect(&url).expect("connect mock");
    let stop = Arc::new(AtomicBool::new(false));
    let mut state = DriverState::new(&config, &url, None, Arc::clone(&stop));
    state.session_id = Some("session".to_string());
    let (tx, _rx) = mpsc::channel();
    let events = BrowserEventSender {
        tx,
        wake: BrowserEventWake::default(),
        committed_url: CommittedUrl::default(),
    };
    let slot = Arc::new(FrameSlot::default());
    let bound = Duration::from_millis(150);
    let result = state
        .evaluate_json_within(&mut link, &events, &slot, "1 + 6", bound)
        .map_err(|error| format!("{}: {}", error.code, error.message))
        .and_then(|value| value.as_i64().ok_or_else(|| format!("no number in {value}")));
    // A key press changes the page, so a timeout must never send it twice.
    let key = state.send_page_command_within(
        &mut link,
        &events,
        &slot,
        "Input.dispatchKeyEvent",
        &json!({ "type": "keyDown" }),
        bound,
    );
    assert!(key.is_err(), "a key press with no answer is a failure: {key:?}");
    drop(link);
    let (read_requests, key_requests) = server.join().expect("mock server");
    ReadRetry {
        result,
        read_requests,
        key_requests,
    }
}

#[test]
fn a_read_of_a_far_browser_is_sent_once_more_but_a_key_press_is_not() {
    let far = run_read_retry(true);
    assert_eq!(far.result, Ok(7), "the second send of the read answered");
    assert_eq!(
        far.read_requests, 2,
        "a far browser that misses the bound answers the same read once more"
    );
    assert_eq!(far.key_requests, 1, "a call that changes the page is never sent twice");

    let local = run_read_retry(false);
    assert!(local.result.is_err(), "a loopback read keeps its bound");
    assert_eq!(
        local.read_requests, 1,
        "a loopback browser that misses its bound has a real problem"
    );
    assert_eq!(local.key_requests, 1);
}

/// A mock browser that answers every call of a page attach and records what it
/// received, so a test can say which listeners a reattach sets and whether a
/// navigation runs.
fn run_attach(outcome_resume: Option<bool>) -> SetupOutcome {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let address = listener.local_addr().expect("mock address");
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept mock");
        stream.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
        let mut socket = tungstenite::accept(stream).expect("mock handshake");
        let mut commands = Vec::new();
        while let Ok(Message::Text(text)) = socket.read() {
            let command: Value = serde_json::from_str(&text).expect("command JSON");
            let method = command["method"].as_str().unwrap_or_default();
            let result = match method {
                "Page.getFrameTree" => json!({"frameTree": {"frame": {"id": "frame"}}}),
                "Target.getTargetInfo" => json!({"targetInfo": {"targetId": "page", "title": "Kept form"}}),
                _ => json!({}),
            };
            commands.push(command.clone());
            // The title question is answered after the test dropped the link.
            let _ = socket.send(Message::Text(
                json!({ "id": command["id"], "result": result }).to_string().into(),
            ));
        }
        commands
    });
    let slot = Arc::new(FrameSlot::default());
    let config = BrowserSessionConfig {
        attach: Some(
            crate::DevtoolsEndpoint::from_active_port_file("9222\n/devtools/browser/kept", 40000)
                .expect("a forwarded endpoint"),
        ),
        resume_target: outcome_resume.map(|_| "page".to_string()),
        resume_context: None,
        page_isolation: crate::PageIsolationReport::default(),
        browser: BrowserConfig {
            automation_disclosure: AutomationDisclosurePolicy::BrowserDefault,
            ..BrowserConfig::default()
        },
        panel_local_id: "resume-attach".to_string(),
        initial_url: Some("https://example.test/inbox".to_string()),
        width: 1280,
        height: 800,
        frame_slot: Arc::clone(&slot),
        coordination: None,
        capture_directory: None,
        video: Arc::default(),
        remote: None,
    };
    let url = format!("ws://{address}/");
    let mut link = CdpLink::connect(&url).expect("connect mock");
    let mut state = DriverState::new(&config, &url, None, Arc::new(AtomicBool::new(false)));
    let (tx, rx) = mpsc::channel();
    let events = BrowserEventSender {
        tx,
        wake: BrowserEventWake::default(),
        committed_url: CommittedUrl::default(),
    };
    let ready = state.attach_setup(
        &mut link,
        &events,
        &slot,
        "session",
        "page",
        outcome_resume.unwrap_or(false),
    );
    let session = state.session_id.clone();
    drop(link);
    SetupOutcome {
        ready,
        session,
        commands: server.join().expect("mock server"),
        events: rx.try_iter().collect(),
    }
}

#[test]
fn a_resumed_page_runs_no_navigation_and_still_sets_every_listener_once() {
    let fresh = run_attach(None);
    assert!(fresh.ready, "the first attach of a page is ready");
    let navigations: Vec<&Value> = fresh
        .commands
        .iter()
        .filter(|command| command["method"] == "Page.navigate")
        .collect();
    assert_eq!(navigations.len(), 1, "a new session opens its first page");
    assert_eq!(navigations[0]["params"]["url"], "https://example.test/inbox");

    let resumed = run_attach(Some(true));
    assert!(resumed.ready, "a resumed page is ready");
    assert!(
        !resumed
            .commands
            .iter()
            .any(|command| command["method"] == "Page.navigate"),
        "a resumed page keeps the document it had: {:?}",
        resumed.commands
    );
    // Everything that a new session sets, the resumed page sets again: one
    // listener of each kind, in the same place as a fresh attach.
    for method in [
        "Network.enable",
        "Page.enable",
        "Page.setLifecycleEventsEnabled",
        "Page.setInterceptFileChooserDialog",
        "Target.setAutoAttach",
        "Emulation.setDeviceMetricsOverride",
        "Page.startScreencast",
    ] {
        let fresh_count = fresh
            .commands
            .iter()
            .filter(|command| command["method"] == method)
            .count();
        let resumed_count = resumed
            .commands
            .iter()
            .filter(|command| command["method"] == method)
            .count();
        assert_eq!(fresh_count, 1, "a new session sets {method} once");
        assert_eq!(
            resumed_count, 1,
            "the reattach sets {method} again, exactly once, because a new driver owns none of it"
        );
    }
    assert!(
        resumed.events.iter().any(|event| matches!(event, BrowserEvent::Ready)),
        "a resumed page reports that it is ready: {:?}",
        resumed.events
    );
}

#[test]
fn the_title_question_never_holds_the_driver_for_a_round_trip() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let address = listener.local_addr().expect("mock address");
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept mock");
        stream.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
        let mut socket = tungstenite::accept(stream).expect("mock handshake");
        while let Ok(Message::Text(text)) = socket.read() {
            let command: Value = serde_json::from_str(&text).expect("command JSON");
            if command["method"] == "Target.getTargetInfo" {
                // A far browser that answers its title question slowly.
                std::thread::sleep(Duration::from_millis(300));
                let reply = json!({
                    "id": command["id"],
                    "result": {"targetInfo": {"targetId": "page", "title": "Inbox"}}
                });
                socket.send(Message::Text(reply.to_string().into())).expect("respond");
            }
        }
    });
    let slot = Arc::new(FrameSlot::default());
    let config = BrowserSessionConfig {
        attach: None,
        resume_target: None,
        resume_context: None,
        page_isolation: crate::PageIsolationReport::default(),
        browser: BrowserConfig::default(),
        panel_local_id: "title".to_string(),
        initial_url: None,
        width: 800,
        height: 600,
        frame_slot: Arc::clone(&slot),
        coordination: None,
        capture_directory: None,
        video: Arc::default(),
        remote: None,
    };
    let url = format!("ws://{address}/");
    let mut link = CdpLink::connect(&url).expect("connect mock");
    let (tx, rx) = mpsc::channel();
    let events = BrowserEventSender {
        tx,
        wake: BrowserEventWake::default(),
        committed_url: CommittedUrl::default(),
    };
    let mut state = DriverState::new(&config, &url, None, Arc::new(AtomicBool::new(false)));
    state.target_id = Some("page".to_string());
    state.title_fetch_at = Some(Instant::now());

    let asked = Instant::now();
    state.tick_title_fetch(&mut link, &events, &slot);
    let asking = asked.elapsed();
    assert!(
        asking < Duration::from_millis(100),
        "the driver asked for the title without waiting for it, and took {asking:?}"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut title = None;
    while title.is_none() && Instant::now() < deadline {
        if let Ok(Some(message)) = link.read_one() {
            state.handle_message(&mut link, &events, &slot, message);
        }
        title = rx.try_iter().find_map(|event| match event {
            BrowserEvent::Title(title) => Some(title),
            _ => None,
        });
    }
    assert_eq!(title.as_deref(), Some("Inbox"), "the answer still arrives");
    drop(link);
    let _ = server.join();
}

#[test]
fn a_picture_asks_for_scroll_geometry_only_when_its_size_changed() {
    let config = BrowserSessionConfig {
        attach: None,
        resume_target: None,
        resume_context: None,
        page_isolation: crate::PageIsolationReport::default(),
        browser: BrowserConfig::default(),
        panel_local_id: "scrollbar-layout".to_string(),
        initial_url: None,
        width: 800,
        height: 600,
        frame_slot: Arc::default(),
        coordination: None,
        capture_directory: None,
        video: Arc::default(),
        remote: None,
    };
    let mut state = DriverState::new(&config, "ws://127.0.0.1/test", None, Arc::new(AtomicBool::new(false)));

    state.note_frame_size_for_layout((800, 600));
    assert!(
        state.scrollbar_layout.refresh_at.is_some(),
        "the first picture of a session asks for the geometry once"
    );
    state.scrollbar_layout.refresh_at = None;
    state.note_frame_size_for_layout((800, 600));
    assert!(
        state.scrollbar_layout.refresh_at.is_none(),
        "a picture of an unchanged size asks nothing: the request would share the link with every frame"
    );
    state.note_frame_size_for_layout((900, 600));
    assert!(
        state.scrollbar_layout.refresh_at.is_some(),
        "a reflow asks again at once"
    );

    state.scrollbar_layout.painted_since_refresh = true;
    state.scrollbar_layout.refresh_at = None;
    state.note_scrollbar_layout_answer();
    let due = state.scrollbar_layout.refresh_at.expect("a painting page re-reads");
    assert!(
        due >= Instant::now() + Duration::from_millis(240),
        "the slow cadence, not one read per picture"
    );
    assert!(!state.scrollbar_layout.painted_since_refresh);

    state.scrollbar_layout.refresh_at = None;
    state.note_scrollbar_layout_answer();
    assert!(
        state.scrollbar_layout.refresh_at.is_none(),
        "a still page costs nothing"
    );
}
