use std::io::{Read, Write};

use super::*;
use crate::test_egui::DiscardTextures;

#[test]
fn close_cancels_a_server_that_never_sends_its_greeting() -> Result<(), ViewError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let session = Session::start(
        listener.local_addr()?,
        Context::default(),
        ViewportId::ROOT,
        DeviceViewOptions::default(),
    )?;
    let accepted = std::time::Instant::now();
    let (mut stream, _) = loop {
        match listener.accept() {
            Ok(connection) => break connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(accepted.elapsed() < Duration::from_secs(2), "viewer did not connect");
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error.into()),
        }
    };
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    let start = std::time::Instant::now();
    drop(session);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(stream.read(&mut [0; 1])?, 0);
    Ok(())
}

#[test]
fn close_cancels_a_connected_server_with_an_incomplete_frame() -> Result<(), ViewError> {
    let (session, mut stream) = connected_session()?;
    // Begin one Raw rectangle but leave its pixel payload unfinished.
    stream.write_all(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 2, 0, 2, 0, 0, 0, 0, 1, 2, 3, 0])?;
    assert!(matches!(
        session.take_updates(ViewportId::ROOT).status,
        Some(Status::Connected)
    ));
    let closed = std::time::Instant::now();
    drop(session);
    assert!(closed.elapsed() < Duration::from_secs(1));
    // Drain any queued refresh requests and prove the connection was closed.
    let mut remaining = Vec::new();
    match stream.read_to_end(&mut remaining) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(error) => return Err(error.into()),
    }
    assert!(remaining.as_chunks::<10>().0.iter().all(|request| request[0] == 3));
    Ok(())
}
fn connected_session() -> Result<(Session, std::net::TcpStream), ViewError> {
    named_session_with_visibility("", true)
}

fn named_session_with_visibility(name: &str, visible: bool) -> Result<(Session, std::net::TcpStream), ViewError> {
    named_session_with_context(name, visible, Context::default())
}

fn named_session_with_context(
    name: &str,
    visible: bool,
    ctx: Context,
) -> Result<(Session, std::net::TcpStream), ViewError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let session = Session::start(
        listener.local_addr()?,
        ctx,
        ViewportId::ROOT,
        DeviceViewOptions::default(),
    )?;
    session.set_visible(visible);
    let started = std::time::Instant::now();
    let (mut stream, _) = loop {
        match listener.accept() {
            Ok(connection) => break connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(started.elapsed() < Duration::from_secs(2), "viewer did not connect");
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error.into()),
        }
    };
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(b"RFB 003.008\n")?;
    let mut version = [0; 12];
    stream.read_exact(&mut version)?;
    assert_eq!(&version, b"RFB 003.008\n");
    stream.write_all(&[1, 1])?;
    let mut byte = [0; 1];
    stream.read_exact(&mut byte)?;
    assert_eq!(byte, [1], "no-auth security selected");
    stream.write_all(&[0; 4])?;
    stream.read_exact(&mut byte)?;
    assert_eq!(byte, [1], "shared connection requested");
    // A 2x2 true-color desktop.
    stream.write_all(&[0, 2, 0, 2, 32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 0, 8, 16, 0, 0, 0])?;
    stream.write_all(&u32::try_from(name.len()).unwrap().to_be_bytes())?;
    stream.write_all(name.as_bytes())?;
    let mut pixel_format = [0; 20];
    stream.read_exact(&mut pixel_format)?;
    assert_eq!(pixel_format[0], 0);
    let mut encodings_header = [0; 4];
    stream.read_exact(&mut encodings_header)?;
    assert_eq!(encodings_header[0], 2);
    let count = usize::from(u16::from_be_bytes([encodings_header[2], encodings_header[3]]));
    assert_eq!(count, 5);
    stream.read_exact(&mut [0; 20])?;
    let mut refresh = [0; 10];
    stream.read_exact(&mut refresh)?;
    assert_eq!(refresh[0], 3, "handshake completed before cancellation");
    Ok((session, stream))
}

fn refresh_until(stream: &mut std::net::TcpStream, expected: [u8; 10]) -> Result<(), ViewError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let mut request = [0; 10];
        stream.read_exact(&mut request)?;
        assert_eq!(request[0], 3);
        if request == expected {
            return Ok(());
        }
        assert!(
            std::time::Instant::now() < deadline,
            "missing refresh {expected:?}; got {request:?}"
        );
    }
}

fn send_pixel(stream: &mut std::net::TcpStream) -> Result<(), ViewError> {
    stream.write_all(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 20, 180, 40, 0])?;
    Ok(())
}

fn wait_for_green_pixel(session: &Session, size: [usize; 2]) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(image) = session.take_updates(ViewportId::ROOT).image
            && image.size == size
            && image.pixels[0] == egui::Color32::from_rgb(20, 180, 40)
        {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "updated pixels never arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn resized_desktop_requests_all_pixels_before_incremental_updates() -> Result<(), ViewError> {
    let (session, mut stream) = connected_session()?;
    send_pixel(&mut stream)?;
    wait_for_green_pixel(&session, [2, 2]);
    // DesktopSize changes to 1x1 without resending the unchanged pixel. An
    // incremental request alone would leave the newly allocated image blank.
    stream.write_all(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1, 255, 255, 255, 33])?;
    refresh_until(&mut stream, [3, 0, 0, 0, 0, 0, 0, 1, 0, 1])?;
    send_pixel(&mut stream)?;
    wait_for_green_pixel(&session, [1, 1]);
    refresh_until(&mut stream, [3, 1, 0, 0, 0, 0, 0, 1, 0, 1])?;
    Ok(())
}

fn wait_for_sequence(session: &Session, minimum: u64) -> u64 {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let sequence = session.observation().received_frame_sequence;
        if sequence >= minimum {
            return sequence;
        }
        assert!(std::time::Instant::now() < deadline, "background pixels never arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn hidden_viewer_receives_first_image_and_retains_only_latest_without_repainting() -> Result<(), ViewError> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let ctx = Context::default();
    let (session, mut stream) = named_session_with_context("", true, ctx.clone())?;
    session.set_visible(false);
    // Drain the handshake repaint so coalescing cannot hide a background request.
    for _ in 0..3 {
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {}).discard_textures();
    }
    let repaints = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&repaints);
    ctx.set_request_repaint_callback(move |_| {
        count.fetch_add(1, Ordering::Relaxed);
    });
    assert_eq!(session.observation().received_frame_sequence, 0);
    send_pixel(&mut stream)?;
    let first = wait_for_sequence(&session, 1);
    refresh_until(&mut stream, [3, 1, 0, 0, 0, 0, 0, 2, 0, 2])?;
    // A different pixel supersedes the first image without a UI upload.
    stream.write_all(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 200, 10, 30, 0])?;
    let latest = wait_for_sequence(&session, first + 1);
    assert_eq!(session.observation().received_frame_sequence, latest);
    session.set_visible(true);
    // No more server output: returning must present the already received pixels.
    let updates = session.take_updates(ViewportId::ROOT);
    assert_eq!(updates.received_frame_sequence, latest);
    assert_eq!(updates.image.unwrap().pixels[0], egui::Color32::from_rgb(200, 10, 30));
    assert!(session.take_updates(ViewportId::ROOT).image.is_none());
    assert_eq!(
        session.latest_full().unwrap().pixels[0],
        egui::Color32::from_rgb(200, 10, 30)
    );
    assert_eq!(repaints.load(Ordering::Relaxed), 0);
    // A visible update must trigger the same callback: the negative check above
    // must not pass simply because egui is coalescing every repaint request.
    send_pixel(&mut stream)?;
    wait_for_sequence(&session, latest + 1);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while repaints.load(Ordering::Relaxed) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "visible update never requested repaint"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    session.set_visible(false);
    let closed = std::time::Instant::now();
    drop(session);
    assert!(
        closed.elapsed() < Duration::from_secs(1),
        "hidden worker failed to cancel"
    );
    Ok(())
}

#[test]
fn hidden_viewer_processes_resize_and_disconnect() -> Result<(), ViewError> {
    let (session, mut stream) = connected_session()?;
    session.set_visible(false);
    send_pixel(&mut stream)?;
    let first = wait_for_sequence(&session, 1);
    stream.write_all(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1, 255, 255, 255, 33])?;
    refresh_until(&mut stream, [3, 0, 0, 0, 0, 0, 0, 1, 0, 1])?;
    send_pixel(&mut stream)?;
    wait_for_sequence(&session, first + 1);
    assert_eq!(session.latest_full().unwrap().size, [1, 1]);
    drop(stream);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if matches!(session.observation().status, Some(Status::Disconnected(_))) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "hidden disconnect never observed");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(session.take_updates(ViewportId::ROOT).image.is_some());
    Ok(())
}

fn send_extended_size(stream: &mut std::net::TcpStream, width: u16, height: u16) -> Result<(), ViewError> {
    let mut packet = vec![0, 0, 0, 1, 0, 0, 0, 0];
    packet.extend(width.to_be_bytes());
    packet.extend(height.to_be_bytes());
    packet.extend((-308_i32).to_be_bytes());
    packet.extend([1, 0, 0, 0]);
    packet.extend(1_u32.to_be_bytes());
    packet.extend([0; 4]);
    packet.extend(width.to_be_bytes());
    packet.extend(height.to_be_bytes());
    packet.extend([0; 4]);
    stream.write_all(&packet)?;
    Ok(())
}

#[test]
fn extended_layout_announcements_do_not_start_full_refresh_loops() -> Result<(), ViewError> {
    let (session, mut stream) = connected_session()?;
    send_pixel(&mut stream)?;
    wait_for_green_pixel(&session, [2, 2]);
    refresh_until(&mut stream, [3, 1, 0, 0, 0, 0, 0, 2, 0, 2])?;
    for _ in 0..4 {
        send_extended_size(&mut stream, 2, 2)?;
        let mut request = [0; 10];
        stream.read_exact(&mut request)?;
        assert_eq!(request, [3, 1, 0, 0, 0, 0, 0, 2, 0, 2]);
    }
    // Extended resize invalidates pixels; the server supplies the new contents
    // in response to the next incremental request, without a full-refresh loop.
    send_extended_size(&mut stream, 3, 2)?;
    refresh_until(&mut stream, [3, 1, 0, 0, 0, 0, 0, 3, 0, 2])?;
    send_pixel(&mut stream)?;
    wait_for_green_pixel(&session, [3, 2]);
    Ok(())
}

#[test]
fn shrink_with_an_outside_crop_preserves_the_session() -> Result<(), ViewError> {
    let (session, mut stream) = connected_session()?;
    send_pixel(&mut stream)?;
    wait_for_green_pixel(&session, [2, 2]);
    session.set_options(DeviceViewOptions {
        viewport: Some(horizon_core::DeviceViewport {
            x: 1,
            y: 1,
            width: 1,
            height: 1,
        }),
        ..Default::default()
    });
    send_extended_size(&mut stream, 1, 1)?;
    refresh_until(&mut stream, [3, 1, 0, 0, 0, 0, 0, 1, 0, 1])?;
    send_pixel(&mut stream)?;
    wait_for_green_pixel(&session, [1, 1]);
    refresh_until(&mut stream, [3, 1, 0, 0, 0, 0, 0, 1, 0, 1])?;
    Ok(())
}

#[test]
fn handshake_name_is_observable_without_receiving_or_displaying_an_image() -> Result<(), ViewError> {
    for (raw, expected) in [
        ("Lab desktop ÆØÅ", Some("Lab desktop ÆØÅ")),
        ("", None),
        ("\nLab\0desktop", Some("Lab desktop")),
    ] {
        let (session, _stream) = named_session_with_visibility(raw, false)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !matches!(session.observation().status, Some(Status::Connected)) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let details = session.take_server_details();
        assert_eq!(details.name.as_deref(), expected);
        assert_eq!(details.desktop_size, Some([2, 2]));
        assert!(session.latest_full().is_none());
    }
    Ok(())
}

#[test]
fn resize_before_disconnect_retains_the_last_observed_desktop_size() -> Result<(), ViewError> {
    for extended in [false, true] {
        let (session, mut stream) = named_session_with_visibility("Lab desktop", false)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !matches!(session.observation().status, Some(Status::Connected)) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        if extended {
            send_extended_size(&mut stream, 3, 2)?;
        } else {
            stream.write_all(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 3, 0, 2, 255, 255, 255, 33])?;
        }
        stream.shutdown(std::net::Shutdown::Write)?;
        // Queue both events while the consumer is paused, then drain them together.
        std::thread::sleep(Duration::from_millis(50));
        session.set_visible(true);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !matches!(session.observation().status, Some(Status::Disconnected(_))) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(session.take_server_details().desktop_size, Some([3, 2]));
        assert!(session.latest_full().is_none());
    }
    Ok(())
}
