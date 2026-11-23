use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::Duration;

use tungstenite::Message;

use super::*;
use crate::session::{BrowserEventWake, BrowserSessionConfig, CommittedUrl};
use crate::{BrowserConfig, BrowserKey};

#[test]
fn chromium_press_dispatches_trusted_key_events_to_the_page_session() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        let mut commands = Vec::new();
        while let Ok(Message::Text(text)) = socket.read() {
            let command: Value = serde_json::from_str(&text).unwrap();
            let response = json!({"id": command["id"], "result": {}});
            commands.push(command);
            socket.send(Message::Text(response.to_string().into())).unwrap();
        }
        commands
    });
    let slot = Arc::new(FrameSlot::new());
    let config = BrowserSessionConfig {
        browser: BrowserConfig::default(),
        panel_local_id: "press-test".into(),
        initial_url: None,
        width: 1280,
        height: 800,
        frame_slot: Arc::clone(&slot),
        coordination: None,
        capture_directory: None,
        video: Arc::default(),
        remote: None,
    };
    let mut state = DriverState::new(&config, &url, None, Arc::new(AtomicBool::new(false)));
    state.owner_seen = Some("agent".into());
    state.session_id = Some("page".into());
    let (tx, _rx) = mpsc::channel();
    let events = BrowserEventSender {
        tx,
        wake: BrowserEventWake::default(),
        committed_url: CommittedUrl::default(),
    };
    let mut link = crate::cdp::CdpLink::connect(&url).unwrap();
    let request = AgentAction {
        action_id: "press".into(),
        actor: "agent".into(),
        requested_at_millis: crate::navigation::now_millis(),
        action: BrowserControlAction::Press {
            key: BrowserKey::Tab,
            modifiers: BrowserModifiers {
                shift: true,
                ..BrowserModifiers::none()
            },
            count: 2,
        },
    };
    let (result, stop) = state.execute_agent_action(&mut link, &events, &slot, &request);
    assert_eq!(result, Ok(BrowserControlValue::Accepted));
    assert!(!stop);
    drop(link);
    let keys = server
        .join()
        .unwrap()
        .into_iter()
        .filter(|command| command["method"] == "Input.dispatchKeyEvent")
        .map(|command| {
            assert_eq!(command["sessionId"], "page");
            let params = &command["params"];
            (
                params["type"].as_str().unwrap().to_string(),
                params["key"].as_str().unwrap().to_string(),
                params["modifiers"].as_u64().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let press = [
        ("rawKeyDown".to_string(), "Tab".to_string(), 8),
        ("keyUp".to_string(), "Tab".to_string(), 0),
    ];
    assert_eq!(keys, [press.clone(), press].concat());
}

/// The fake page session that the dispatch tests share.
fn dispatch_session(
    slot: &Arc<FrameSlot>,
    name: &str,
) -> (String, std::thread::JoinHandle<Vec<Value>>, BrowserSessionConfig) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        let mut commands = Vec::new();
        while let Ok(Message::Text(text)) = socket.read() {
            let command: Value = serde_json::from_str(&text).unwrap();
            let response = json!({"id": command["id"], "result": {}});
            commands.push(command);
            socket.send(Message::Text(response.to_string().into())).unwrap();
        }
        commands
    });
    let config = BrowserSessionConfig {
        browser: BrowserConfig::default(),
        panel_local_id: name.into(),
        initial_url: None,
        width: 1280,
        height: 800,
        frame_slot: Arc::clone(slot),
        coordination: None,
        capture_directory: None,
        video: Arc::default(),
        remote: None,
    };
    (url, server, config)
}

fn mouse_events(commands: Vec<Value>) -> Vec<(String, f64, f64, u64)> {
    commands
        .into_iter()
        .filter(|command| command["method"] == "Input.dispatchMouseEvent")
        .map(|command| {
            let params = &command["params"];
            (
                params["type"].as_str().unwrap_or_default().to_string(),
                params["x"].as_f64().unwrap_or_default(),
                params["y"].as_f64().unwrap_or_default(),
                params["clickCount"].as_u64().unwrap_or_default(),
            )
        })
        .collect()
}

#[test]
fn chromium_click_point_dispatches_one_trusted_press_and_release_at_the_point() {
    let slot = Arc::new(FrameSlot::new());
    let (url, server, config) = dispatch_session(&slot, "click-point");
    // A frame of 800 by 600 CSS pixels is what the panel draws.
    let _ = slot.store_rgb(800, 600, vec![0; 800 * 600 * 3]);
    let mut state = DriverState::new(&config, &url, None, Arc::new(AtomicBool::new(false)));
    state.owner_seen = Some("agent".into());
    state.session_id = Some("page".into());
    let (tx, _rx) = mpsc::channel();
    let events = BrowserEventSender {
        tx,
        wake: BrowserEventWake::default(),
        committed_url: CommittedUrl::default(),
    };
    let mut link = crate::cdp::CdpLink::connect(&url).unwrap();
    let request = |x: f64, y: f64| AgentAction {
        action_id: "click".into(),
        actor: "agent".into(),
        requested_at_millis: crate::navigation::now_millis(),
        action: BrowserControlAction::ClickPoint { x, y, count: 1 },
    };

    let (result, _) = state.execute_agent_action(&mut link, &events, &slot, &request(120.0, 240.0));
    assert_eq!(result, Ok(BrowserControlValue::Accepted));
    drop(link);

    assert_eq!(
        mouse_events(server.join().unwrap()),
        [
            ("mousePressed".to_string(), 120.0, 240.0, 1),
            ("mouseReleased".to_string(), 120.0, 240.0, 1),
        ],
        "one press and one release, both at the point"
    );
}

#[test]
fn chromium_click_point_outside_the_page_viewport_is_refused_before_any_input() {
    let slot = Arc::new(FrameSlot::new());
    let (url, server, config) = dispatch_session(&slot, "click-point-outside");
    let _ = slot.store_rgb(800, 600, vec![0; 800 * 600 * 3]);
    let mut state = DriverState::new(&config, &url, None, Arc::new(AtomicBool::new(false)));
    state.owner_seen = Some("agent".into());
    state.session_id = Some("page".into());
    let (tx, _rx) = mpsc::channel();
    let events = BrowserEventSender {
        tx,
        wake: BrowserEventWake::default(),
        committed_url: CommittedUrl::default(),
    };
    let mut link = crate::cdp::CdpLink::connect(&url).unwrap();
    let request = AgentAction {
        action_id: "click".into(),
        actor: "agent".into(),
        requested_at_millis: crate::navigation::now_millis(),
        action: BrowserControlAction::ClickPoint {
            x: 5000.0,
            y: 10.0,
            count: 1,
        },
    };

    let (result, _) = state.execute_agent_action(&mut link, &events, &slot, &request);
    let error = result.expect_err("a point outside the viewport is refused");
    assert_eq!(error.code, "invalid_input");
    assert!(error.message.contains("5000"), "{}", error.message);
    drop(link);

    assert!(
        mouse_events(server.join().unwrap()).is_empty(),
        "a refused point sends no input to the page"
    );
}
