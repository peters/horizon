//! Live Chromium check that a size-1 `<select>` click publishes a host overlay.
//! Ignored in CI; run with `cargo test -p horizon-browser --test native_select_live -- --ignored`.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use horizon_browser::{
    BrowserButton, BrowserCommand, BrowserConfig, BrowserEvent, BrowserInput, BrowserModifiers, BrowserSession,
    BrowserSessionConfig, FrameSlot, NativeSelectPopup, start_session,
};

const FIXTURE: &str = r#"<!doctype html><title>select live</title>
<select id="native-single" style="position:absolute;left:20px;top:20px;width:160px;height:28px">
<option value="alpha">Alpha</option>
<option value="bravo" selected>Bravo</option>
<option value="charlie">Charlie</option>
</select>"#;

#[test]
#[ignore = "requires a local Chromium browser"]
fn chromium_click_on_a_native_select_publishes_a_host_popup() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let addr = listener.local_addr().expect("addr");
    thread::spawn(move || serve_fixture(&listener));

    let temp = tempfile::tempdir().expect("temp");
    let frame_slot = Arc::new(FrameSlot::new());
    let session = start_session(BrowserSessionConfig {
        browser: BrowserConfig {
            profile_root: Some(temp.path().join("profiles")),
            ..BrowserConfig::default()
        },
        panel_local_id: "native-select-live".into(),
        initial_url: Some(format!("http://{addr}/")),
        width: 800,
        height: 600,
        frame_slot: Arc::clone(&frame_slot),
        coordination: None,
        capture_directory: None,
        video: Arc::new(horizon_browser::VideoCaptureHandle::default()),
        remote: None,
    })
    .expect("start");

    wait_until_loaded(&session, &frame_slot);
    click_select(&session);
    let popup = wait_for_popup(&session, &frame_slot).expect("native select popup should be published");
    assert_eq!(popup.css_path, "#native-single");
    assert_eq!(popup.options.len(), 3);
    assert_eq!(popup.options[1].value, "bravo");
    assert!(popup.options[1].selected);

    assert!(session.send(BrowserCommand::NativeSelectChoose { index: 2 }));
    wait_until(&session, Duration::from_secs(5), || {
        frame_slot.native_select_popup().is_none()
    });
    assert!(
        frame_slot.native_select_popup().is_none(),
        "overlay should close after choose"
    );
    click_select(&session);
    let reopened = wait_for_popup(&session, &frame_slot).expect("reopening after choose should publish the popup");
    let selected = reopened
        .options
        .iter()
        .find(|option| option.selected)
        .expect("selected option");
    assert_eq!(selected.value, "charlie");
    assert!(session.send(BrowserCommand::Stop));
}

fn serve_fixture(listener: &TcpListener) {
    listener.set_nonblocking(false).ok();
    while let Ok((mut stream, _)) = listener.accept() {
        let mut buf = [0_u8; 2048];
        let _ = stream.read(&mut buf);
        let body = FIXTURE.as_bytes();
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(body);
    }
}

fn wait_until_loaded(session: &BrowserSession, frame_slot: &FrameSlot) {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut loaded = false;
    while Instant::now() < deadline {
        while let Ok(event) = session.event_rx.try_recv() {
            match &event {
                BrowserEvent::UrlChanged(url) if url.contains("127.0.0.1") => loaded = true,
                BrowserEvent::Title(title) if title.contains("select live") => loaded = true,
                _ => {}
            }
        }
        if loaded && frame_slot.latest().is_some() {
            thread::sleep(Duration::from_millis(400));
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("fixture page did not load");
}

fn click_select(session: &BrowserSession) {
    assert!(session.send(BrowserCommand::Input(BrowserInput::MousePress {
        x: 80.0,
        y: 34.0,
        button: BrowserButton::Left,
        click_count: 1,
        buttons: 1,
        modifiers: BrowserModifiers::none(),
    })));
    assert!(session.send(BrowserCommand::Input(BrowserInput::MouseRelease {
        x: 80.0,
        y: 34.0,
        button: BrowserButton::Left,
        click_count: 1,
        buttons: 0,
        modifiers: BrowserModifiers::none(),
    })));
}

fn wait_for_popup(session: &BrowserSession, frame_slot: &FrameSlot) -> Option<Arc<NativeSelectPopup>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(popup) = frame_slot.native_select_popup() {
            return Some(popup);
        }
        let _ = session.event_rx.try_recv();
        thread::sleep(Duration::from_millis(50));
    }
    None
}

fn wait_until(session: &BrowserSession, timeout: Duration, predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        let _ = session.event_rx.try_recv();
        thread::sleep(Duration::from_millis(50));
    }
}
