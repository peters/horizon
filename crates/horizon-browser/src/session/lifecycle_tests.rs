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
        state.attach_setup(&mut link, &events, &slot, "session", "target")
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
