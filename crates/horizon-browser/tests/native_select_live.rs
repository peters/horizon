//! Live Chromium check that a size-1 `<select>` click publishes a host overlay.
//! Ignored in CI; run with `cargo test -p horizon-browser --test native_select_live -- --ignored`.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use horizon_browser::{
    BrowserButton, BrowserCommand, BrowserConfig, BrowserEvent, BrowserInput, BrowserModifiers, BrowserSessionConfig,
    FrameSlot, start_session,
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
    thread::spawn(move || {
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
    });

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
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(loaded, "fixture page did not load");
    thread::sleep(Duration::from_millis(400));

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

    let popup_deadline = Instant::now() + Duration::from_secs(5);
    let mut popup = None;
    while Instant::now() < popup_deadline {
        popup = frame_slot.native_select_popup();
        if popup.is_some() {
            break;
        }
        let _ = session.event_rx.try_recv();
        thread::sleep(Duration::from_millis(50));
    }
    let popup = popup.expect("native select popup should be published");
    assert_eq!(popup.css_path, "#native-single");
    assert_eq!(popup.options.len(), 3);
    assert_eq!(popup.options[1].value, "bravo");
    assert!(popup.options[1].selected);

    assert!(session.send(BrowserCommand::NativeSelectChoose { index: 2 }));
    let applied_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < applied_deadline {
        if frame_slot.native_select_popup().is_none() {
            break;
        }
        let _ = session.event_rx.try_recv();
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        frame_slot.native_select_popup().is_none(),
        "overlay should close after choose"
    );
    assert!(session.send(BrowserCommand::Stop));
}
