//! The panel side of a browser host against a scripted MCP peer. The peer speaks
//! the contract of a profile host: it answers the handshake, creates one page,
//! inlines one frame per screenshot, records every call and can refuse a call or
//! close the channel. No SSH server and no browser is needed for these tests, so
//! they prove the channel of a panel, not the machine that a person attaches.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use horizon_browser::{BrowserCommand, BrowserInput, BrowserKey, BrowserModifiers};
use serde_json::{Value, json};

use super::view::{HostChannel, HostOpener, HostPage, HostView};
use super::{PANEL_ACTOR, POINTER_NOTE, RemoteHostFailure, RemoteHostRefusal, RemoteHostSlot};
use crate::browser::{BackendKind, BrowserConfig, BrowserPanelState, BrowserStatus};

/// A 1x1 PNG, as a screenshot result inlines it.
const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";
const PAGE: &str = "profile-mail-0123456789ab";
const DEADLINE: Duration = Duration::from_secs(10);

/// What the scripted peer does, and what it was asked.
#[derive(Clone)]
struct Script {
    /// Calls the peer answers before it closes the channel.
    hang_up_after: Option<usize>,
    /// The one tool that the peer refuses, with its reason.
    refuse: Option<(&'static str, &'static str)>,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Script {
    fn new() -> Self {
        Self {
            hang_up_after: None,
            refuse: None,
            seen: Arc::default(),
        }
    }

    /// The tools that the peer answered, in order.
    fn tools(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter_map(|call| call.split_whitespace().next().map(str::to_string))
            .collect()
    }

    /// The arguments of the first call of `tool` that the peer answered.
    fn arguments(&self, tool: &str) -> Option<Value> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|call| call.starts_with(&format!("{tool} ")))
            .and_then(|call| serde_json::from_str(call.split_once(' ').map_or("", |(_, rest)| rest)).ok())
    }

    fn wait_for(&self, tool: &str) {
        self.wait_until(tool, |calls| {
            calls.iter().any(|call| call.starts_with(&format!("{tool} ")))
        });
    }

    /// The arguments of every answered call of `tool`.
    fn calls_of(&self, tool: &str) -> Vec<Value> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|call| call.starts_with(&format!("{tool} ")))
            .filter_map(|call| serde_json::from_str(call.split_once(' ').map_or("", |(_, rest)| rest)).ok())
            .collect()
    }

    /// Wait until one answered call satisfies `wanted`, then give every answered
    /// call of `tool`. The pump carries a queued command on its next tick, so a
    /// test waits for the call instead of assuming one tick.
    fn wait_until(&self, tool: &str, wanted: impl Fn(&[String]) -> bool) {
        let deadline = Instant::now() + DEADLINE;
        while Instant::now() < deadline {
            let calls = self
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if wanted(&calls) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the peer was never asked {tool}: {:?}", self.tools());
    }
}

/// One end of a socket, as the transport of a panel.
struct SocketChannel(Option<UnixStream>);

impl HostChannel for SocketChannel {
    fn take_streams(&mut self) -> Option<(Box<dyn Write + Send>, Box<dyn BufRead + Send>)> {
        let stream = self.0.take()?;
        let output = stream.try_clone().ok()?;
        Some((Box::new(stream), Box::new(BufReader::new(output))))
    }

    fn stop(&mut self) {
        self.0 = None;
    }
}

/// A panel whose page belongs to a scripted host.
fn panel(script: &Script) -> BrowserPanelState {
    let (client, peer) = UnixStream::pair().expect("socket pair");
    let responder = script.clone();
    std::thread::spawn(move || answer(peer, &responder));
    let mut state = BrowserPanelState::inert();
    state.panel_local_id = "panel-mail".to_string();
    state.status = BrowserStatus::Starting;
    state.loading = true;
    let view = HostView::start(
        peer_opener(Box::new(SocketChannel(Some(client)))),
        HostPage {
            url: Some("https://example.test/".to_string()),
            profile: Some("mail".to_string()),
        },
        BackendKind::ChromiumCdp,
        Arc::clone(&state.frame_slot),
    );
    state.remote_host = Some(RemoteHostSlot {
        view: Some(view),
        target: "127.0.0.1".to_string(),
        profile: Some("mail".to_string()),
        failure: None,
    });
    state
}

/// An opener that hands the view a channel that already exists.
fn peer_opener(channel: Box<dyn HostChannel>) -> HostOpener {
    Box::new(move || Ok(channel))
}

/// A panel whose channel never opens, because its opener keeps the worker thread
/// for `delay` and then reports the reason.
fn panel_with_slow_opener(delay: Duration, failure: RemoteHostFailure) -> BrowserPanelState {
    let mut state = BrowserPanelState::inert();
    state.panel_local_id = "panel-mail".to_string();
    state.status = BrowserStatus::Starting;
    state.loading = true;
    let view = HostView::start(
        Box::new(move || {
            std::thread::sleep(delay);
            Err(failure)
        }),
        HostPage {
            url: None,
            profile: Some("mail".to_string()),
        },
        BackendKind::ChromiumCdp,
        Arc::clone(&state.frame_slot),
    );
    state.remote_host = Some(RemoteHostSlot {
        view: Some(view),
        target: "127.0.0.1".to_string(),
        profile: Some("mail".to_string()),
        failure: None,
    });
    state
}

/// The panel state after the pump has published it, once `ready` holds or the
/// status is an error. A ready panel holds one frame of its page: the create
/// answer and the first screenshot are two calls of one tick.
fn settled(panel: &mut BrowserPanelState) -> BrowserStatus {
    let deadline = Instant::now() + DEADLINE;
    loop {
        panel.drain_events();
        let status = panel.status.clone();
        let ready = status == BrowserStatus::Ready && panel.frame_slot.latest().is_some();
        if ready || matches!(status, BrowserStatus::Error { .. }) {
            return status;
        }
        assert!(Instant::now() < deadline, "the panel never settled: {status:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One page of the contract that the peer serves, with the loop of a profile
/// host: one answer per request, and a closed channel after the scripted bound.
fn answer(peer: UnixStream, script: &Script) {
    let mut input = peer.try_clone().expect("socket input");
    let mut output = BufReader::new(peer);
    let mut answered = 0usize;
    loop {
        let mut line = String::new();
        match output.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = request.get("id").cloned();
        let method = request.get("method").and_then(Value::as_str).unwrap_or_default();
        if method == "notifications/initialized" {
            continue;
        }
        let result = if id.is_none() {
            None
        } else if method == "initialize" {
            Some(json!({ "protocolVersion": "2025-11-25", "capabilities": { "tools": {} } }))
        } else {
            let name = request["params"]["name"].as_str().unwrap_or_default().to_string();
            let arguments = request["params"]["arguments"].clone();
            script
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("{name} {arguments}"));
            answered += 1;
            if let Some((refused, reason)) = script.refuse
                && refused == name
            {
                Some(json!({
                    "content": [{ "type": "text", "text": reason }],
                    "isError": true,
                }))
            } else {
                Some(match name.as_str() {
                    // The shape of a real `browser_create`: the page arrives
                    // under `panel`, and the page id belongs to that object.
                    "browser_create" => json!({
                        "content": [{ "type": "text", "text": "created" }],
                        "structuredContent": {
                            "action_id": "create-1",
                            "navigation": "committed",
                            "panel": {
                                "panel_id": PAGE,
                                "url": "https://example.test/",
                                "title": "Example",
                                "backend": "chromium",
                            },
                            "startup_millis": 601,
                        },
                        "isError": false,
                    }),
                    "browser_screenshot" => json!({
                        "content": [
                            { "type": "text", "text": "{\"path\":\"/srv/state/captures/frame.png\"}" },
                            { "type": "image", "data": PIXEL, "mimeType": "image/png" },
                        ],
                        "structuredContent": { "panel_id": PAGE, "width": 1, "height": 1 },
                        "isError": false,
                    }),
                    "browser_panel" => json!({
                        "content": [{ "type": "text", "text": "panel" }],
                        "structuredContent": {
                            "panel_id": PAGE,
                            "url": "https://example.test/",
                            "title": "Example",
                            "backend": "chromium",
                        },
                        "isError": false,
                    }),
                    _ => json!({
                        "content": [{ "type": "text", "text": "accepted" }],
                        "structuredContent": { "panel_id": PAGE },
                        "isError": false,
                    }),
                })
            }
        };
        if let (Some(id), Some(result)) = (id, result) {
            let answer = json!({ "jsonrpc": "2.0", "id": id, "result": result });
            if writeln!(input, "{answer}").is_err() || input.flush().is_err() {
                return;
            }
            if script.hang_up_after.is_some_and(|limit| answered >= limit) {
                return;
            }
        }
    }
}

#[test]
fn a_panel_creates_its_page_on_the_host_and_shows_the_first_frame() {
    let script = Script::new();
    let mut panel = panel(&script);
    assert_eq!(settled(&mut panel), BrowserStatus::Ready);
    assert_eq!(
        script.arguments("browser_create"),
        Some(json!({ "target": "mail", "url": "https://example.test/" })),
        "the page belongs to the profile of the host and opens at the requested URL"
    );
    assert!(panel.frame_slot.latest().is_some(), "the first frame is in the slot");
    let frame = panel.frame_slot.latest().expect("frame");
    assert_eq!((frame.width, frame.height), (1, 1), "the inline PNG is decoded");
    assert_eq!(panel.url.as_deref(), Some("https://example.test/"));
    assert_eq!(panel.title, "Example");
    assert_eq!(
        panel.backend(),
        BackendKind::ChromiumCdp,
        "the panel reports the engine that the host runs"
    );
    assert_eq!(panel.remote_host_target(), Some("127.0.0.1"));
    assert_eq!(panel.remote_host_profile(), Some("mail"));
}

#[test]
fn a_key_press_and_a_page_scroll_reach_the_page_of_the_host() {
    let script = Script::new();
    let mut panel = panel(&script);
    assert_eq!(settled(&mut panel), BrowserStatus::Ready);
    assert!(panel.try_send(BrowserCommand::Input(BrowserInput::KeyDown {
        physical_key: Some(BrowserKey::Tab),
        key: BrowserKey::Tab,
        text: None,
        modifiers: BrowserModifiers::none(),
        repeat: false,
        edit_command: None,
    })));
    assert!(panel.try_send(BrowserCommand::Input(BrowserInput::InsertText {
        text: "A".to_string()
    })));
    assert!(panel.try_send(BrowserCommand::Input(BrowserInput::Wheel {
        x: 10.0,
        y: 20.0,
        delta_x: 0.0,
        delta_y: -120.0,
        modifiers: BrowserModifiers::none(),
    })));
    // The pump carries the queue on its next tick, so wait for the last call of
    // the three instead of assuming that one tick held them all.
    script.wait_until("browser_act", |calls| {
        let arguments = calls
            .iter()
            .filter(|call| call.starts_with("browser_act "))
            .filter_map(|call| serde_json::from_str::<Value>(call.split_once(' ').map_or("", |(_, rest)| rest)).ok())
            .collect::<Vec<Value>>();
        arguments.iter().any(|call| call["action"] == json!("scroll"))
            && arguments.iter().any(|call| call["key"] == json!("Tab"))
            && arguments.iter().any(|call| call["key"] == json!("Shift+a"))
    });
    let presses = script.calls_of("browser_act");
    assert!(
        presses.contains(&json!({ "action": "press", "key": "Tab", "count": 1, "panel_id": PAGE })),
        "a key reaches the focused element of the page: {presses:?}"
    );
    assert!(
        presses.contains(&json!({ "action": "press", "key": "Shift+a", "count": 1, "panel_id": PAGE })),
        "committed text keeps its US-layout base key: {presses:?}"
    );
    assert!(
        presses.contains(&json!({ "action": "scroll", "delta_x": 0.0, "delta_y": -120.0, "panel_id": PAGE })),
        "a wheel scrolls the page, and the pointer position does not cross: {presses:?}"
    );
}

#[test]
fn a_navigation_reaches_the_page_of_the_host() {
    let script = Script::new();
    let mut panel = panel(&script);
    assert_eq!(settled(&mut panel), BrowserStatus::Ready);
    assert!(panel.try_send(BrowserCommand::Navigate("https://example.test/next".to_string())));
    script.wait_for("browser_navigate");
    assert_eq!(
        script.arguments("browser_navigate"),
        Some(json!({ "panel_id": PAGE, "url": "https://example.test/next" }))
    );
}

#[test]
fn a_pointer_position_is_refused_with_a_typed_reason_and_the_panel_names_the_device_view() {
    let script = Script::new();
    let mut panel = panel(&script);
    assert_eq!(settled(&mut panel), BrowserStatus::Ready);
    let click = BrowserCommand::Input(BrowserInput::MousePress {
        x: 30.0,
        y: 40.0,
        button: horizon_browser::BrowserButton::Left,
        click_count: 1,
        buttons: 1,
        modifiers: BrowserModifiers::none(),
    });
    assert_eq!(
        panel.remote_host_refusal(&click),
        Some(RemoteHostRefusal::Pointer),
        "the contract has no coordinate pointer operation"
    );
    assert!(!panel.try_send(click), "a refused command never reaches the host");
    assert_eq!(
        RemoteHostRefusal::Pointer.code(),
        "remote_host_pointer_unsupported",
        "the reason is typed"
    );
    assert_eq!(
        panel.remote_host_note(),
        Some(POINTER_NOTE),
        "the panel shows one short line about the way to click the page"
    );
    assert!(
        POINTER_NOTE.contains("device panel"),
        "the line names the device view: {POINTER_NOTE}"
    );
}

#[test]
fn a_refusal_of_the_host_reaches_the_panel_with_the_reason_of_the_host() {
    let mut script = Script::new();
    script.refuse = Some((
        "browser_act",
        "browser action act-1 failed (unsupported_backend): the page takes no press",
    ));
    let mut panel = panel(&script);
    assert_eq!(settled(&mut panel), BrowserStatus::Ready);
    assert!(panel.try_send(BrowserCommand::Input(BrowserInput::KeyDown {
        physical_key: Some(BrowserKey::Enter),
        key: BrowserKey::Enter,
        text: None,
        modifiers: BrowserModifiers::none(),
        repeat: false,
        edit_command: None,
    })));
    let deadline = Instant::now() + DEADLINE;
    loop {
        panel.drain_events();
        if panel.navigation_error.is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the refusal of the host never reached the panel"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        panel.navigation_error.as_deref(),
        Some("browser action act-1 failed (unsupported_backend): the page takes no press"),
        "the panel shows the reason of the host, unchanged"
    );
    assert_eq!(
        panel.status,
        BrowserStatus::Ready,
        "a refused call of one command does not end the page"
    );
}

#[test]
fn a_closed_panel_closes_its_page_on_the_host() {
    let script = Script::new();
    let mut panel = panel(&script);
    assert_eq!(settled(&mut panel), BrowserStatus::Ready);
    panel.stop();
    script.wait_for("browser_close");
    assert!(
        panel.is_remote_host(),
        "a closed panel keeps the backend that it reports"
    );
    assert_eq!(
        script.arguments("browser_close"),
        Some(json!({ "panel_id": PAGE })),
        "the page of the host belongs to this panel, so it closes with it"
    );
}

#[test]
fn a_host_that_disappears_mid_session_ends_the_panel_with_a_typed_reason() {
    let mut script = Script::new();
    // The handshake, the create and one screenshot, then the channel closes.
    script.hang_up_after = Some(3);
    let mut panel = panel(&script);
    let deadline = Instant::now() + DEADLINE;
    let status = loop {
        panel.drain_events();
        if matches!(panel.status, BrowserStatus::Error { .. }) {
            break panel.status.clone();
        }
        assert!(Instant::now() < deadline, "the panel did not end: {:?}", panel.status);
        std::thread::sleep(Duration::from_millis(20));
    };
    let BrowserStatus::Error { message } = status else {
        unreachable!("the loop breaks on an error");
    };
    assert!(
        message.contains("channel to the browser host ended"),
        "the reason is the fixed text of the typed failure: {message}"
    );
    assert!(
        panel.is_remote_host(),
        "the panel keeps the backend that names its host, so a retry reaches that host again"
    );
    assert!(panel.can_retry(), "a stopped panel can open a new channel");
    assert!(
        panel.remote_host_target().is_some(),
        "the panel still reports the host that it uses"
    );
    assert!(
        panel
            .remote_status
            .as_deref()
            .is_some_and(|note| note.contains("127.0.0.1")),
        "the panel names the host that ended: {:?}",
        panel.remote_status
    );
}

#[test]
fn a_channel_that_never_opens_does_not_hold_the_thread_that_asked() {
    /// How long the stand-in opener holds its worker thread.
    const OPEN_DELAY: Duration = Duration::from_millis(1500);
    let started = Instant::now();
    let mut panel = panel_with_slow_opener(OPEN_DELAY, RemoteHostFailure::NO_ANSWER);
    let asked = started.elapsed();
    assert!(
        asked < Duration::from_millis(200),
        "creating the panel never waits for the host: {asked:?}"
    );
    assert!(
        panel.remote_host_failure().is_none(),
        "the panel starts with no reason and keeps painting"
    );
    let deadline = Instant::now() + DEADLINE;
    while panel.remote_host_failure().is_none() && Instant::now() < deadline {
        panel.drain_events();
        std::thread::sleep(Duration::from_millis(20));
    }
    panel.drain_events();
    assert_eq!(
        panel.remote_host_failure().map(|failure| failure.code),
        Some("remote_host_no_answer"),
        "the reason of the worker thread reaches the panel as a typed state"
    );
    assert!(matches!(panel.status, BrowserStatus::Error { .. }));
    assert!(
        panel.remote_host_target().is_some(),
        "the panel still reports the host that it uses"
    );
}

#[test]
fn a_relaunch_of_a_panel_whose_host_does_not_answer_returns_at_once() {
    let mut state = BrowserPanelState::inert();
    state.panel_local_id = "panel-mail".to_string();
    // A backend with no view is what a failed channel leaves behind, which is the
    // state in which the person sees the Retry button.
    state.remote_host = Some(RemoteHostSlot {
        view: None,
        target: "127.0.0.1".to_string(),
        profile: Some("mail".to_string()),
        failure: Some(RemoteHostFailure::UNREACHABLE),
    });
    state.status = BrowserStatus::Error {
        message: RemoteHostFailure::UNREACHABLE.message.to_string(),
    };
    let started = Instant::now();
    state.relaunch();
    let asked = started.elapsed();
    assert!(
        asked < Duration::from_millis(200),
        "the retry of the person never waits for ssh: {asked:?}"
    );
    assert_eq!(
        state.status,
        BrowserStatus::Starting,
        "the panel shows that it is starting while the host is contacted"
    );
    let deadline = Instant::now() + DEADLINE;
    while state.remote_host_failure().is_none() && Instant::now() < deadline {
        state.drain_events();
        std::thread::sleep(Duration::from_millis(20));
    }
    state.drain_events();
    assert!(
        state
            .remote_host_failure()
            .is_some_and(|failure| failure.code == "remote_host_unreachable"
                || failure.code == "remote_host_no_answer"),
        "a host that does not answer ends the retry with a typed reason: {:?}",
        state.remote_host_failure()
    );
}

#[test]
fn a_local_panel_keeps_its_own_backend_and_capabilities() {
    let panel = BrowserPanelState::inert();
    assert!(!panel.is_remote_host());
    assert_eq!(panel.remote_host_target(), None);
    assert_eq!(panel.remote_host_profile(), None);
    assert_eq!(panel.remote_host_note(), None);
    assert_eq!(
        panel.remote_host_refusal(&BrowserCommand::Input(BrowserInput::MousePress {
            x: 1.0,
            y: 1.0,
            button: horizon_browser::BrowserButton::Left,
            click_count: 1,
            buttons: 1,
            modifiers: BrowserModifiers::none(),
        })),
        None,
        "a local panel takes the pointer of the person"
    );
    assert_eq!(
        panel.backend_capabilities(),
        BackendKind::ChromiumCdp.capabilities(),
        "a local panel keeps the capabilities of its own backend"
    );
    assert_eq!(panel.backend(), BrowserConfig::default().backend);
}

#[test]
fn the_actor_of_the_person_is_a_horizon_identity_that_no_agent_uses() {
    assert_eq!(PANEL_ACTOR, "horizon:person:browser-panel");
    assert!(
        horizon_browser_control::manifest::actor_is_workspace_scoped(PANEL_ACTOR),
        "a profile host takes only a horizon identity"
    );
    let agent = "horizon:workspace-a/agent-1";
    assert_ne!(PANEL_ACTOR, agent, "the page of the person is not the page of an agent");
    assert!(
        !agent.starts_with("horizon:person:"),
        "no agent identity uses the reserved person prefix"
    );
}
