//! Live checks that nested scroll containers stay usable in browser panels:
//! Chromium drags its native scrollbar, and Firefox publishes the host
//! indicator its screenshots omit while its native gutter still drags.
//! Ignored in CI; run with `cargo test -p horizon-browser --test scrollbar_live -- --ignored`.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use horizon_browser::{
    BackendKind, BrowserButton, BrowserCommand, BrowserConfig, BrowserEvent, BrowserInput, BrowserModifiers,
    BrowserSession, BrowserSessionConfig, FrameSlot, NestedScrollbar, start_session,
};

const FIXTURE: &str = r#"<!doctype html><title>scroll live</title>
<style>html,body{height:100%;margin:0;overflow:hidden}
header{height:56px}main{position:absolute;top:56px;bottom:0;left:0;right:0;overflow:auto}
div{height:60px}</style><header></header><main id="list"></main>
<script>
const list = document.getElementById('list');
for (let i = 0; i < 120; i++) list.appendChild(document.createElement('div')).textContent = 'Item ' + i;
const header = document.querySelector('header');
list.addEventListener('scroll', () => { header.style.background = list.scrollTop > 1000 ? '#00ff00' : ''; });
</script>"#;

#[test]
#[ignore = "requires a local Chromium browser"]
fn chromium_drags_a_nested_native_scrollbar() {
    let (session, frame_slot, _profiles) = start(BackendKind::ChromiumCdp);
    wait_until_loaded(&session, &frame_slot);
    // Chromium paints its own nested scrollbar against the right edge.
    drag(&session, 793.0, 80.0, 400.0);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut scrolled = false;
    while Instant::now() < deadline && !scrolled {
        // The fixture paints its header green once the container passed 1000px.
        scrolled = frame_slot.latest().is_some_and(|frame| {
            let offset = usize::try_from(frame.width * 20 + 20).unwrap_or(usize::MAX) * 3;
            frame
                .rgb
                .get(offset..offset + 3)
                .is_some_and(|pixel| pixel[0] < 80 && pixel[1] > 200 && pixel[2] < 80)
        });
        let _ = session.event_rx.try_recv();
        thread::sleep(Duration::from_millis(50));
    }
    assert!(scrolled, "dragging the native thumb should scroll the container");
    assert!(session.send(BrowserCommand::Stop));
}

#[test]
#[ignore = "requires a local Firefox browser and geckodriver"]
fn firefox_publishes_and_drags_a_nested_scrollbar() {
    let (session, frame_slot, _profiles) = start(BackendKind::FirefoxBidi);
    wait_until_loaded(&session, &frame_slot);
    let bar = wait_for_bar(&session, &frame_slot, |_| true).expect("nested scrollbar should be published");
    assert!(bar.track_width >= 1.0);
    assert!((bar.track_x + bar.track_width - 800.0).abs() < 1.0, "{bar:?}");
    assert!((bar.track_y - 56.0).abs() < 1.0, "{bar:?}");
    assert!((bar.track_height - 544.0).abs() < 1.0, "{bar:?}");
    assert!((bar.visible_top - bar.track_y).abs() < 1.0, "{bar:?}");
    assert!((bar.visible_bottom - 600.0).abs() < 1.0, "{bar:?}");
    assert!(bar.scroll_top.abs() < f32::EPSILON);

    let Some((thumb_top, thumb_height)) = bar.thumb() else {
        panic!("published bar should be scrollable");
    };
    let x = f64::from(bar.track_x + bar.track_width / 2.0);
    let y = f64::from(bar.track_y + thumb_top + thumb_height / 2.0);
    drag(&session, x, y, y + 300.0);
    let scrolled = wait_for_bar(&session, &frame_slot, |bar| bar.scroll_top > 1_000.0);
    assert!(
        scrolled.is_some(),
        "dragging the gutter should scroll and refresh the indicator"
    );
    assert!(session.send(BrowserCommand::Stop));
}

fn start(backend: BackendKind) -> (BrowserSession, Arc<FrameSlot>, tempfile::TempDir) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let addr = listener.local_addr().expect("addr");
    thread::spawn(move || serve_fixture(&listener));
    let temp = tempfile::tempdir().expect("temp");
    let frame_slot = Arc::new(FrameSlot::new());
    let session = start_session(BrowserSessionConfig {
        browser: BrowserConfig {
            backend,
            profile_root: Some(temp.path().join("profiles")),
            ..BrowserConfig::default()
        },
        panel_local_id: "scrollbar-live".into(),
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
    (session, frame_slot, temp)
}

fn drag(session: &BrowserSession, x: f64, from_y: f64, to_y: f64) {
    let modifiers = BrowserModifiers::none();
    assert!(session.send(BrowserCommand::Input(BrowserInput::MouseMove {
        x,
        y: from_y,
        buttons: 0,
        modifiers,
    })));
    assert!(session.send(BrowserCommand::Input(BrowserInput::MousePress {
        x,
        y: from_y,
        button: BrowserButton::Left,
        click_count: 1,
        buttons: 1,
        modifiers,
    })));
    let mut y = from_y;
    while y < to_y {
        y = (y + 20.0).min(to_y);
        assert!(session.send(BrowserCommand::Input(BrowserInput::MouseMove {
            x,
            y,
            buttons: 1,
            modifiers,
        })));
        thread::sleep(Duration::from_millis(30));
    }
    assert!(session.send(BrowserCommand::Input(BrowserInput::MouseRelease {
        x,
        y: to_y,
        button: BrowserButton::Left,
        click_count: 1,
        buttons: 0,
        modifiers,
    })));
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
                BrowserEvent::Title(title) if title.contains("scroll live") => loaded = true,
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

fn wait_for_bar(
    session: &BrowserSession,
    frame_slot: &FrameSlot,
    accept: impl Fn(&NestedScrollbar) -> bool,
) -> Option<NestedScrollbar> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(bar) = frame_slot.nested_scrollbars().iter().find(|bar| accept(bar)) {
            return Some(*bar);
        }
        let _ = session.event_rx.try_recv();
        thread::sleep(Duration::from_millis(50));
    }
    None
}
