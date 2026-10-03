use super::*;
use crate::{Orientation, Resolution};
use std::net::TcpListener;
#[cfg(unix)]
use std::{
    io::{Read, Write},
    net::TcpStream,
};

#[cfg(unix)]
fn request(socket: &mut TcpStream) -> String {
    let mut header = Vec::new();
    let mut byte = [0];
    while !header.ends_with(b"\r\n\r\n") {
        socket.read_exact(&mut byte).expect("request header");
        header.push(byte[0]);
    }
    let text = String::from_utf8(header).expect("header");
    let length = text
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .expect("length")
        .parse::<usize>()
        .expect("length number");
    socket.read_exact(&mut vec![0; length]).expect("request body");
    text
}

// Persistent pairing uses private Unix files and advisory receiver leases.
#[cfg(unix)]
#[test]
fn stop_interrupts_submitted_pin_and_saved_verification_without_extra_exchanges() {
    let _serial = lock(&crate::pairing::TEST_RECEIVER);
    for remembered in [false, true] {
        let home = tempfile::tempdir().expect("home");
        let store = PairingStore::new(home.path().join("pairings"), "synthetic".into(), "Synthetic TV".into());
        if remembered {
            let mut credentials = crate::PairingCredentials::new().expect("identity");
            credentials.receiver_id = b"synthetic".to_vec();
            credentials.receiver_key = vec![3; 32];
            store.save(&credentials).expect("saved fixture");
        }
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let (send, receive) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            socket.set_read_timeout(Some(Duration::from_secs(2))).expect("timeout");
            if !remembered {
                assert!(request(&mut socket).starts_with("POST /pair-pin-start "));
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .expect("PIN prompt");
            }
            assert!(request(&mut socket).starts_with(if remembered {
                "POST /pair-verify "
            } else {
                "POST /pair-setup "
            }));
            send.send(()).expect("blocked exchange");
            let mut byte = [0];
            assert_eq!(socket.read(&mut byte).expect("cancel closes transport"), 0);
        });
        let mut session =
            CastSession::start_remembered(address, VideoFormat::default(), store.clone()).expect("session");
        if !remembered {
            let deadline = Instant::now() + Duration::from_secs(2);
            while session.status() != CastStatus::PinRequired {
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(5));
            }
            session.pair(Zeroizing::new("1234".into())).expect("PIN");
        }
        receive
            .recv_timeout(Duration::from_secs(2))
            .expect("authentication in flight");
        let started = Instant::now();
        session.stop();
        while !session.finished() {
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "cancellation must interrupt blocking I/O"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(session.status(), CastStatus::Stopped);
        session.reap();
        assert!(session.worker.is_none(), "finished worker must be joined");
        assert_eq!(store.load().expect("load").is_some(), remembered);
        assert!(store.reserve().is_ok(), "cross-process lease released");
        server.join().expect("server");
    }
}

#[cfg(not(unix))]
#[test]
fn persistent_pairing_fails_without_opening_a_receiver_connection() {
    let home = tempfile::tempdir().expect("home");
    let store = PairingStore::new(home.path().join("pairings"), "synthetic".into(), "Synthetic TV".into());
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    listener.set_nonblocking(true).expect("nonblocking");
    let mut session =
        CastSession::start_remembered(listener.local_addr().expect("address"), VideoFormat::default(), store)
            .expect("session");
    let deadline = Instant::now() + Duration::from_secs(2);
    while !session.finished() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        session.status(),
        CastStatus::Failed("invalid receiver message: persistent pairing requires Linux".into())
    );
    assert_eq!(
        listener.accept().expect_err("no receiver connection").kind(),
        std::io::ErrorKind::WouldBlock
    );
    session.reap();
    assert!(session.worker.is_none());
}
#[test]
fn raw_source_support_requires_nonzero_even_fitted_axes_for_the_selected_canvas() {
    for resolution in [Resolution::Hd720, Resolution::FullHd1080, Resolution::Uhd4k] {
        for orientation in [Orientation::Landscape, Orientation::Portrait] {
            let (width, height) = VideoFormat {
                orientation,
                resolution,
            }
            .dimensions();
            let canvas = (usize::from(width), usize::from(height));
            assert!(CastSession::supports_source_dimensions(canvas.0, canvas.1, canvas));
            for crop in [(641, 361), (1023, 767), (719, 1279)] {
                assert!(CastSession::supports_source_dimensions(crop.0, crop.1, canvas));
            }
            for crop in [(0, 1), (1, 0), (8192, 4), (4, 8192), (8193, 1), (8192, 8192)] {
                assert!(!CastSession::supports_source_dimensions(crop.0, crop.1, canvas));
            }
        }
    }
    assert!(!CastSession::supports_source_dimensions(4, 4, (0, 720)));
    assert!(!CastSession::supports_source_dimensions(4, 4, (1280, 0)));
    assert!(!CastSession::supports_source_dimensions(8192, 8, (1280, 720)));
    assert!(CastSession::supports_source_dimensions(8192, 8, (3840, 2160)));
}
