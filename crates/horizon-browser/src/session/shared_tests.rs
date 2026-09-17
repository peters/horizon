use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::{Value, json};
use tungstenite::Message;

use super::*;
use crate::cdp::CdpLink;
use crate::process::{ChromeProcessControl, ProcessLifecycle};

fn pages() -> (
    SharedBrowserSession,
    SharedDriverReservation,
    SharedDriverReservation,
    DriverProcess,
    DriverProcess,
) {
    let group = SharedBrowserSession::new("profile".into());
    let first = group.clone().reserve(Arc::new(false.into()));
    let second = group.clone().reserve(Arc::new(false.into()));
    group.state.lock().expect("state").pages = 2;
    let page = || {
        DriverProcess::Shared(Arc::new(PageLifecycle {
            state: Arc::clone(&group.state),
            target: Mutex::new(None),
            closing: false.into(),
            released: false.into(),
            last_page: false.into(),
            process: ChromeProcessControl::default(),
            drivers: Arc::clone(&group.drivers),
            stops: Arc::clone(&group.stops),
        }))
    };
    let left = page();
    let right = page();
    (group, first, second, left, right)
}

fn connection() -> (CdpLink, std::thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let address = listener.local_addr().expect("address");
    let task = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("fixture connection");
        stream.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout");
        let mut socket = tungstenite::accept(stream).expect("handshake");
        let mut commands = Vec::new();
        while let Ok(Message::Text(text)) = socket.read() {
            let command: Value = serde_json::from_str(&text).expect("command");
            socket
                .send(Message::Text(
                    json!({"id": command["id"], "result": {"success": true}})
                        .to_string()
                        .into(),
                ))
                .expect("response");
            commands.push(command);
        }
        commands
    });
    (
        CdpLink::connect(&format!("ws://{address}/")).expect("fixture transport"),
        task,
    )
}

#[test]
fn closing_original_only_closes_its_target_and_last_page_closes_browser() {
    let (group, first, second, mut left, mut right) = pages();
    let (mut link, server) = connection();
    left.close_page(&mut link, "original");
    drop(first);
    assert!(!group.is_idle());
    right.close_page(&mut link, "duplicate");
    drop(second);
    assert!(group.is_idle());
    drop(link);
    let commands = server.join().expect("fixture stopped");
    assert_eq!(
        commands
            .iter()
            .map(|value| value["method"].as_str().expect("method"))
            .collect::<Vec<_>>(),
        ["Target.closeTarget", "Target.closeTarget", "Browser.close"]
    );
    assert_eq!(commands[0]["params"]["targetId"], "original");
    assert_eq!(commands[1]["params"]["targetId"], "duplicate");
}

#[test]
fn release_is_idempotent_and_dropping_a_sibling_keeps_group_busy() {
    let (group, first, second, mut left, right) = pages();
    assert!(left.kill());
    assert!(left.kill());
    drop(first);
    assert_eq!(group.state.lock().expect("state").pages, 1);
    assert!(!group.is_idle());
    drop(right);
    drop(second);
    assert!(group.is_idle());
}

#[test]
fn a_pending_driver_reserves_profile_before_acquiring_a_page() {
    let group = SharedBrowserSession::new("profile".into());
    let pending = group.clone().reserve(Arc::new(false.into()));
    assert!(!group.is_idle());
    drop(pending);
    assert!(group.is_idle());
}

#[test]
fn forced_cleanup_refuses_to_terminate_siblings() {
    let (_group, first, second, mut left, mut right) = pages();
    let DriverProcess::Shared(control) = &left else {
        panic!("shared page");
    };
    assert!(!control.terminate(Duration::ZERO));
    assert!(!control.is_reaped());
    left.kill();
    drop(first);
    let DriverProcess::Shared(control) = &left else {
        panic!("shared page");
    };
    assert!(control.is_reaped());
    assert!(control.terminate(Duration::ZERO));
    right.kill();
    drop(second);
}

#[test]
fn last_page_waits_for_a_pending_driver_before_closing_browser() {
    let group = SharedBrowserSession::new("profile".into());
    let active = group.clone().reserve(Arc::new(false.into()));
    let pending = group.clone().reserve(Arc::new(false.into()));
    group.state.lock().expect("state").pages = 1;
    let page = Arc::new(PageLifecycle {
        state: Arc::clone(&group.state),
        target: Mutex::new(None),
        closing: false.into(),
        released: false.into(),
        last_page: false.into(),
        process: ChromeProcessControl::default(),
        drivers: Arc::clone(&group.drivers),
        stops: Arc::clone(&group.stops),
    });
    let (mut link, server) = connection();
    page.release(Some((&mut link, "only-active-page")));
    drop(active);
    assert!(!group.is_idle());
    assert!(!page.last_page.load(Ordering::Acquire));
    drop(link);
    assert_eq!(server.join().expect("fixture stopped").len(), 1);
    drop(pending);
    assert!(group.is_idle());
}

#[test]
fn shutdown_delegation_does_not_reap_a_live_sibling() {
    let (_group, first, second, left, right) = pages();
    let DriverProcess::Shared(page) = &left else {
        panic!("shared page");
    };
    let control = ChromeProcessControl::default();
    control.delegate(page.clone());
    assert!(!control.is_reaped());
    assert!(!control.terminate(Duration::ZERO));
    drop(left);
    drop(first);
    assert!(control.is_reaped());
    drop(right);
    drop(second);
}

#[test]
fn application_shutdown_can_force_cleanup_only_after_every_sibling_requests_stop() {
    let (group, first, second, left, right) = pages();
    let DriverProcess::Shared(page) = &left else {
        panic!("shared page");
    };
    page.process.mark_registration_settled();
    first.stop.store(true, Ordering::Release);
    assert!(!page.terminate(Duration::ZERO));
    second.stop.store(true, Ordering::Release);
    assert!(page.terminate(Duration::from_millis(10)));
    drop(left);
    drop(first);
    drop(right);
    drop(second);
    assert!(group.is_idle());
}

struct UnreapedProcess(AtomicBool);

impl ProcessLifecycle for UnreapedProcess {
    fn is_reaped(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    fn terminate(&self, _timeout: Duration) -> bool {
        self.is_reaped()
    }
}

#[test]
fn failed_startup_retains_exact_control_and_refuses_profile_reuse_until_reaped() {
    let group = SharedBrowserSession::new("failed-profile".into());
    let reservation = group.clone().reserve(Arc::new(false.into()));
    let panel = ChromeProcessControl::default();
    let child = Arc::new(UnreapedProcess(false.into()));
    let result = group.acquire_with(&reservation.stop, &panel, |control| {
        control.delegate(child.clone());
        assert!(!panel.is_reaped(), "panel tracks the child before startup returns");
        Err("injected startup failure with failed reap".into())
    });
    assert!(result.is_err());
    assert!(!panel.is_reaped());
    assert!(!panel.terminate(Duration::ZERO));
    assert!(
        group
            .acquire_with(&reservation.stop, &ChromeProcessControl::default(), |_| {
                panic!("must not retry startup while the previous exact child is retained")
            })
            .is_err()
    );
    child.0.store(true, Ordering::Release);
    assert!(panel.is_reaped());
    drop(reservation);
    assert!(group.is_idle());
}

#[test]
fn refused_close_remains_pending_and_does_not_release_live_sibling() {
    let (group, first, second, mut left, right) = pages();
    let DriverProcess::Shared(page) = &left else {
        panic!("shared page")
    };
    let page = Arc::clone(page);
    let child = Arc::new(UnreapedProcess(false.into()));
    page.process.delegate(child.clone());
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let mut socket = tungstenite::accept(stream).expect("handshake");
        for method in ["Target.closeTarget", "Target.getTargets"] {
            let Message::Text(text) = socket.read().expect("command") else {
                panic!("text")
            };
            let command: Value = serde_json::from_str(&text).expect("json");
            assert_eq!(command["method"], method);
            socket
                .send(Message::Text(
                    json!({"id": command["id"], "result": {
                        "success": false, "targetInfos": [{"targetId":"original"}]
                    }})
                    .to_string()
                    .into(),
                ))
                .expect("reply");
        }
    });
    let mut link = CdpLink::connect(&format!("ws://{address}/")).expect("link");
    left.close_page(&mut link, "original");
    server.join().expect("fixture");
    assert!(!page.is_reaped());
    assert_eq!(group.state.lock().expect("state").pages, 2);
    assert!(!page.terminate(Duration::from_millis(10)));
    assert!(!child.is_reaped(), "a refused tab close must not terminate its sibling");
    child.0.store(true, Ordering::Release);
    assert!(page.is_reaped(), "process exit also proves the target is gone");
    drop(left);
    drop(first);
    drop(right);
    drop(second);
    assert!(group.is_idle());
}

#[test]
fn disconnected_page_transport_reconnects_to_close_only_its_target() {
    let (group, first, second, mut left, right) = pages();
    let (dead_link, dead_server) = connection();
    // Closing this fixture leaves a disconnected URL; a fresh connection is
    // provided as the group endpoint for page-only teardown.
    drop(dead_link);
    dead_server.join().expect("old transport stopped");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    group.state.lock().expect("state").endpoint = format!("ws://{address}/");
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let mut socket = tungstenite::accept(stream).expect("handshake");
        let Message::Text(text) = socket.read().expect("command") else {
            panic!("text")
        };
        let command: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(command["method"], "Target.closeTarget");
        assert_eq!(command["params"]["targetId"], "original");
        socket
            .send(Message::Text(
                json!({"id":command["id"],"result":{"success":true}}).to_string().into(),
            ))
            .expect("reply");
    });
    left.register_target("original");
    assert!(left.kill());
    server.join().expect("reconnection");
    assert_eq!(group.state.lock().expect("state").pages, 1);
    drop(first);
    drop(right);
    drop(second);
    assert!(group.is_idle());
}

#[test]
fn last_driver_drop_rechecks_reservations_after_waiting_for_state() {
    let group = SharedBrowserSession::new("profile".into());
    let old = group.clone().reserve(Arc::new(false.into()));
    let state = group.state.lock().expect("hold state while the old driver drops");
    let dropping = std::thread::spawn(move || drop(old));
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while group.drivers.load(Ordering::Acquire) != 0 {
        assert!(std::time::Instant::now() < deadline, "drop reached state lock");
        std::thread::yield_now();
    }
    let new = group.clone().reserve(Arc::new(false.into()));
    drop(state);
    dropping.join().expect("old driver dropped");
    assert!(!group.state.lock().expect("state").retiring);
    assert!(!group.is_idle());
    drop(new);
    assert!(group.is_idle());
}

#[test]
fn emergency_deadline_never_waits_for_a_page_transport_or_target_lock() {
    let (group, first, second, left, right) = pages();
    let listener = TcpListener::bind("127.0.0.1:0").expect("stalled endpoint");
    listener.set_nonblocking(true).expect("nonblocking listener");
    group.state.lock().expect("state").endpoint = format!("ws://{}/", listener.local_addr().expect("address"));
    left.register_target("original");
    let DriverProcess::Shared(page) = &left else {
        panic!("shared page")
    };
    let target_guard = page.target.lock().expect("target lock held by stalled driver");
    let started = std::time::Instant::now();
    assert!(!page.terminate(Duration::ZERO));
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(
        listener.accept().expect_err("no emergency network attempt").kind(),
        std::io::ErrorKind::WouldBlock
    );
    drop(target_guard);
    // The target-lock assertion above is the only stalled operation. Avoid
    // starting a real network retry during the synthetic fixture's teardown.
    *page.target.lock().expect("target") = None;
    drop(left);
    drop(first);
    drop(right);
    drop(second);
}
