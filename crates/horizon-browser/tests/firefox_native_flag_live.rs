//! Live Firefox check that minimization leaves a native `navigator.webdriver`
//! getter returning false. Ignored in CI; run with
//! `cargo test -p horizon-browser --test firefox_native_flag_live -- --ignored`.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use horizon_browser::{
    BackendKind, BrowserConfig, BrowserEvent, BrowserSession, BrowserSessionConfig, FrameSlot, start_session,
};

const FIXTURE: &str = r#"<!doctype html><title>pending</title>
<script>
const getter = Function.prototype.toString.call(
  Object.getOwnPropertyDescriptor(Navigator.prototype, "webdriver").get
);
const native = getter.includes("[native code]");
document.title = (navigator.webdriver ? "true" : "false") + (native ? " native" : " patched");
</script>"#;

#[test]
#[ignore = "requires a local Firefox browser and geckodriver"]
fn firefox_minimization_keeps_a_native_webdriver_getter_false() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let addr = listener.local_addr().expect("addr");
    thread::spawn(move || serve_fixture(&listener));

    let profiles = tempfile::tempdir().expect("profile root");
    let frame_slot = Arc::new(FrameSlot::new());
    let session = start_session(BrowserSessionConfig {
        browser: BrowserConfig {
            backend: BackendKind::FirefoxBidi,
            profile_root: Some(profiles.path().to_path_buf()),
            firefox_system_access: true,
            ..BrowserConfig::default()
        },
        panel_local_id: "firefox-native-webdriver-flag".into(),
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

    let (title, disclosure) = wait_for_title_and_disclosure(&session);
    let shutdown = session.shutdown_signal().with_profile_cleanup(profiles.keep());
    assert!(
        shutdown.wait(Duration::from_secs(15)) || shutdown.force_cleanup(Duration::from_secs(5)),
        "Firefox teardown and profile cleanup must complete"
    );
    assert_eq!(title, "false native", "page title reported {title}");
    assert_eq!(
        disclosure,
        Some(horizon_browser::AutomationDisclosureStatus::CommonSignalsMinimized),
        "native clear must publish common_signals_minimized"
    );
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

fn wait_for_title_and_disclosure(
    session: &BrowserSession,
) -> (String, Option<horizon_browser::AutomationDisclosureStatus>) {
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut title = None;
    let mut disclosure = None;
    while Instant::now() < deadline {
        while let Ok(event) = session.event_rx.try_recv() {
            match event {
                BrowserEvent::BackendReady(capabilities) => {
                    disclosure = Some(capabilities.automation_disclosure);
                }
                BrowserEvent::Title(value)
                    if value == "false native" || value.ends_with("patched") || value.starts_with("true") =>
                {
                    title = Some(value);
                }
                _ => {}
            }
        }
        if title.is_some() && disclosure.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    (title.unwrap_or_default(), disclosure)
}
