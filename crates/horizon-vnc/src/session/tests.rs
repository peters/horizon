use std::io::{Read, Write};
use std::time::Duration;

use super::*;

fn direct(address: SocketAddr, options: ViewOptions) -> Config {
    Config {
        options,
        ..Config::new(Route::Direct(address))
    }
}

#[test]
fn close_cancels_a_server_that_never_sends_its_greeting() -> Result<(), Error> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let session = Session::start(direct(listener.local_addr()?, ViewOptions::default()), |_| {})?;
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
fn close_cancels_a_connected_server_with_an_incomplete_frame() -> Result<(), Error> {
    let (session, mut stream) = connected_session()?;
    // Begin one Raw rectangle but leave its pixel payload unfinished.
    stream.write_all(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 2, 0, 2, 0, 0, 0, 0, 1, 2, 3, 0])?;
    assert!(matches!(session.take_updates().status, Some(Status::Connected)));
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

fn connected_session() -> Result<(Session, std::net::TcpStream), Error> {
    named_session_with_visibility("", true)
}

fn named_session_with_visibility(name: &str, visible: bool) -> Result<(Session, std::net::TcpStream), Error> {
    named_session_with_wake(name, visible, |_| {})
}

fn named_session_with_wake(
    name: &str,
    visible: bool,
    wake: impl Fn(Wake) + Send + Sync + 'static,
) -> Result<(Session, std::net::TcpStream), Error> {
    session_with_options(name, visible, wake, ViewOptions::default())
}

fn session_with_options(
    name: &str,
    visible: bool,
    wake: impl Fn(Wake) + Send + Sync + 'static,
    options: ViewOptions,
) -> Result<(Session, std::net::TcpStream), Error> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let session = Session::start(direct(listener.local_addr()?, options), wake)?;
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

fn refresh_until(stream: &mut std::net::TcpStream, expected: [u8; 10]) -> Result<(), Error> {
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

fn send_pixel(stream: &mut std::net::TcpStream) -> Result<(), Error> {
    send_pixel_at(stream, 0, 0, [20, 180, 40])
}

/// One Raw rectangle of one pixel in a `FramebufferUpdate` message.
fn send_pixel_at(stream: &mut std::net::TcpStream, x: u8, y: u8, rgb: [u8; 3]) -> Result<(), Error> {
    let mut message = vec![0, 0, 0, 1, 0, x, 0, y, 0, 1, 0, 1, 0, 0, 0, 0];
    message.extend(rgb);
    message.push(0);
    stream.write_all(&message)?;
    Ok(())
}

const GREEN: [u8; 4] = [20, 180, 40, 255];

#[test]
fn decoded_frame_evidence_survives_observation_without_consuming_pixels() -> Result<(), Error> {
    let (session, mut stream) = connected_session()?;
    send_pixel(&mut stream)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while session.stream_evidence().sequence == 0 {
        assert!(Instant::now() < deadline, "decoder did not publish evidence");
        std::thread::sleep(Duration::from_millis(5));
    }
    let before = session.stream_evidence();
    assert!(before.last_frame.is_some());
    assert!(session.take_updates().image.is_some());
    assert_eq!(session.stream_evidence().sequence, before.sequence);
    session.set_visible(false);
    send_pixel(&mut stream)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while session.stream_evidence().sequence == before.sequence {
        assert!(Instant::now() < deadline, "offscreen sampling stays active");
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn wait_for_green_pixel(session: &Session, size: [usize; 2]) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(image) = session.take_updates().image
            && image.frame.size() == size
            && image.frame.rgba()[..4] == GREEN
        {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "updated pixels never arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn resized_desktop_requests_all_pixels_before_incremental_updates() -> Result<(), Error> {
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
fn hidden_viewer_receives_first_image_and_retains_only_latest_with_delayed_wakes() -> Result<(), Error> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let repaints = Arc::new(AtomicUsize::new(0));
    let delayed = Arc::new(AtomicUsize::new(0));
    let (count, count_delayed) = (Arc::clone(&repaints), Arc::clone(&delayed));
    let (session, mut stream) = named_session_with_wake("", true, move |wake| {
        let counter = match wake {
            Wake::Visible => &count,
            Wake::Hidden => &count_delayed,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    })?;
    session.set_visible(false);
    // The handshake woke the drawn viewer before its first refresh; count from here.
    repaints.store(0, Ordering::Relaxed);
    delayed.store(0, Ordering::Relaxed);
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
    let updates = session.take_updates();
    assert_eq!(updates.received_frame_sequence, latest);
    assert_eq!(updates.image.unwrap().frame.rgba()[..4], [200, 10, 30, 255]);
    assert!(session.take_updates().image.is_none());
    assert_eq!(session.latest_full().unwrap().rgba()[..4], [200, 10, 30, 255]);
    // Hidden frames only ask for a deferred wake; nothing wakes the host at
    // the stream rate.
    assert_eq!(repaints.load(Ordering::Relaxed), 0);
    assert!(
        delayed.load(Ordering::Relaxed) >= 1,
        "hidden frame never scheduled an upload"
    );
    // A visible update must trigger the same callback: the negative check above
    // must not pass simply because no wake arrives at all.
    send_pixel(&mut stream)?;
    wait_for_sequence(&session, latest + 1);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while repaints.load(Ordering::Relaxed) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "visible update never woke the viewer"
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
fn hidden_viewer_processes_resize_and_disconnect() -> Result<(), Error> {
    let (session, mut stream) = connected_session()?;
    session.set_visible(false);
    send_pixel(&mut stream)?;
    let first = wait_for_sequence(&session, 1);
    stream.write_all(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1, 255, 255, 255, 33])?;
    refresh_until(&mut stream, [3, 0, 0, 0, 0, 0, 0, 1, 0, 1])?;
    send_pixel(&mut stream)?;
    wait_for_sequence(&session, first + 1);
    assert_eq!(session.latest_full().unwrap().size(), [1, 1]);
    drop(stream);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if matches!(session.observation().status, Some(Status::Disconnected { .. })) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "hidden disconnect never observed");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(session.take_updates().image.is_some());
    Ok(())
}

fn extended_size_packet(width: u16, height: u16) -> Vec<u8> {
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
    packet
}

fn send_extended_size(stream: &mut std::net::TcpStream, width: u16, height: u16) -> Result<(), Error> {
    stream.write_all(&extended_size_packet(width, height))?;
    Ok(())
}

#[test]
fn wayvnc_layout_then_pixels_in_one_update_keeps_the_viewer_connected() -> Result<(), Error> {
    let (session, mut stream) = connected_session()?;
    let mut connected = false;
    for width in [2_u16, 3, 1, 2] {
        let mut packet = extended_size_packet(width, 2);
        packet[2..4].copy_from_slice(&2_u16.to_be_bytes());
        // A Raw pixel follows ExtendedDesktopSize in this same FramebufferUpdate.
        packet.extend([0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 20, 180, 40, 0]);
        stream.write_all(&packet)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let updates = session.take_updates();
            if let Some(status) = updates.status {
                assert!(
                    !matches!(status, Status::Disconnected { .. }),
                    "mixed update disconnected the viewer"
                );
                connected |= matches!(status, Status::Connected);
            }
            if let Some(image) = updates.image
                && image.frame.size() == [usize::from(width), 2]
                && image.frame.rgba()[..4] == GREEN
            {
                break;
            }
            assert!(Instant::now() < deadline, "mixed update pixels never arrived");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(connected);
        assert_eq!(
            session.take_server_details().desktop_size,
            Some([usize::from(width), 2])
        );
        refresh_until(&mut stream, [3, 1, 0, 0, 0, 0, 0, width.to_be_bytes()[1], 0, 2])?;
    }
    Ok(())
}

#[test]
fn extended_layout_announcements_do_not_start_full_refresh_loops() -> Result<(), Error> {
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
fn shrink_with_an_outside_crop_preserves_the_session() -> Result<(), Error> {
    let (session, mut stream) = connected_session()?;
    send_pixel(&mut stream)?;
    wait_for_green_pixel(&session, [2, 2]);
    session.set_options(ViewOptions {
        viewport: Some(Region {
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

/// The name is the server's untrusted text, as it sent it; a UI cleans it.
#[test]
fn handshake_name_is_observable_without_receiving_or_displaying_an_image() -> Result<(), Error> {
    for (raw, expected) in [
        ("Lab desktop ÆØÅ", Some("Lab desktop ÆØÅ")),
        ("", None),
        ("\nLab\0desktop", Some("\nLab\0desktop")),
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
fn resize_before_disconnect_retains_the_last_observed_desktop_size() -> Result<(), Error> {
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
        while !matches!(session.observation().status, Some(Status::Disconnected { .. })) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(session.take_server_details().desktop_size, Some([3, 2]));
        assert!(session.latest_full().is_none());
    }
    Ok(())
}

#[test]
fn queued_input_reaches_the_server_before_the_next_refresh() -> Result<(), Error> {
    // One frame a second: the next refresh request is a second away, so
    // anything the server reads first was forwarded while the worker waited.
    let options = ViewOptions {
        max_fps: 1,
        ..ViewOptions::default()
    };
    let (session, mut stream) = session_with_options("", true, |_| {}, options)?;
    session.send_input([
        InputEvent::Key {
            keysym: 0xff0d,
            down: true,
        },
        InputEvent::Pointer { x: 1, y: 0, buttons: 1 },
    ]);
    let mut key = [0; 8];
    stream.read_exact(&mut key)?;
    assert_eq!(key, [4, 1, 0, 0, 0, 0, 0xff, 0x0d], "RFB KeyEvent for Return, down");
    let mut pointer = [0; 6];
    stream.read_exact(&mut pointer)?;
    assert_eq!(pointer, [5, 1, 0, 1, 0, 0], "RFB PointerEvent, left button at (1, 0)");
    let mut refresh = [0; 10];
    stream.read_exact(&mut refresh)?;
    assert_eq!(refresh[0], 3, "the refresh follows the input");
    drop(session);
    Ok(())
}

#[test]
fn each_image_bounds_what_changed_since_the_image_taken_before() -> Result<(), Error> {
    let (session, mut stream) = connected_session()?;
    let next_image = |minimum: u64| {
        wait_for_sequence(&session, minimum);
        session.take_updates().image.expect("a published image")
    };
    send_pixel(&mut stream)?;
    let first = next_image(1);
    assert_eq!(
        first.changed,
        Region::whole([2, 2]),
        "the first image is new everywhere"
    );
    send_pixel_at(&mut stream, 1, 1, [1, 2, 3])?;
    let second = next_image(2);
    assert_eq!(
        second.changed,
        Region {
            x: 1,
            y: 1,
            width: 1,
            height: 1,
        }
    );
    assert_eq!(second.frame.rgba()[..4], GREEN, "the rest of the desktop stays");
    // Two images published before the UI takes one: the second covers both.
    send_pixel_at(&mut stream, 1, 0, [4, 5, 6])?;
    wait_for_sequence(&session, 3);
    send_pixel_at(&mut stream, 1, 1, [7, 8, 9])?;
    let merged = next_image(4);
    assert_eq!(
        merged.changed,
        Region {
            x: 1,
            y: 0,
            width: 1,
            height: 2,
        }
    );
    Ok(())
}
