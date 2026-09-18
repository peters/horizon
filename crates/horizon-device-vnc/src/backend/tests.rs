use super::*;
use std::io::{Read, Write};

enum Reply {
    Silent,
    Raw,
    DesktopSize,
    Resize,
}

fn layout_update(reason: u16, width: u16, height: u16) -> Vec<u8> {
    let mut update = vec![0, 0, 0, 1];
    for value in [reason, 0, width, height] {
        update.extend(value.to_be_bytes());
    }
    update.extend((-308i32).to_be_bytes());
    update.extend([1, 0, 0, 0]);
    update.extend(0u32.to_be_bytes());
    for value in [0, 0, width, height] {
        update.extend(value.to_be_bytes());
    }
    update.extend(0u32.to_be_bytes());
    update
}

fn server(reply: Reply, close: Option<std::sync::mpsc::Receiver<()>>) -> (Target, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        stream.write_all(b"RFB 003.008\n").unwrap();
        stream.read_exact(&mut [0; 12]).unwrap();
        stream.write_all(&[1, 1]).unwrap();
        stream.read_exact(&mut [0]).unwrap();
        stream.write_all(&[0; 4]).unwrap();
        stream.read_exact(&mut [0]).unwrap();
        let mut init = vec![0, 1, 0, 1, 32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 0, 8, 16, 0, 0, 0];
        init.extend(4u32.to_be_bytes());
        init.extend(b"test");
        stream.write_all(&init).unwrap();
        let mut refreshes = 0;
        let mut resize_requested = false;
        loop {
            let mut kind = [0];
            if stream.read_exact(&mut kind).is_err() {
                break;
            }
            match kind[0] {
                0 => {
                    stream.read_exact(&mut [0; 19]).unwrap();
                }
                2 => {
                    let mut header = [0; 3];
                    stream.read_exact(&mut header).unwrap();
                    let count = u16::from_be_bytes([header[1], header[2]]);
                    stream.read_exact(&mut vec![0; usize::from(count) * 4]).unwrap();
                }
                3 => {
                    refreshes += 1;
                    let expected = if resize_requested { 2 } else { 1 };
                    assert_eq!(refreshes, expected, "resize control must not poll framebuffers");
                    stream.read_exact(&mut [0; 9]).unwrap();
                    if let Some(close) = &close {
                        stream.write_all(&layout_update(0, 1, 1)).unwrap();
                        close.recv_timeout(Duration::from_secs(3)).unwrap();
                        return;
                    }
                    match reply {
                        Reply::Silent => {}
                        Reply::Resize => {
                            let update = if resize_requested {
                                layout_update(1, 2, 3)
                            } else {
                                layout_update(0, 1, 1)
                            };
                            stream.write_all(&update).unwrap();
                        }
                        Reply::Raw => stream
                            .write_all(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 20, 180, 40, 0])
                            .unwrap(),
                        Reply::DesktopSize => {
                            let mut update = vec![0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1];
                            update.extend((-223i32).to_be_bytes());
                            stream.write_all(&update).unwrap();
                        }
                    }
                }
                251 => {
                    assert!(matches!(reply, Reply::Resize));
                    assert!(!resize_requested);
                    let mut request = [0; 23];
                    stream.read_exact(&mut request).unwrap();
                    assert_eq!(
                        request,
                        [0, 0, 2, 0, 3, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 3, 0, 0, 0, 0]
                    );
                    resize_requested = true;
                }
                unexpected => panic!("unexpected client message {unexpected}"),
            }
        }
    });
    let target = Target {
        id: "fixture".into(),
        endpoint: horizon_device::Endpoint::LocalX11 { display: ":99".into() },
        desktop_resize: horizon_device::ResizeConfig {
            vnc_address: Some(address),
            ..Default::default()
        },
    };
    (target, server)
}

#[test]
fn stalled_negotiation_is_a_definite_timeout() {
    let (target, server) = server(Reply::Silent, None);
    assert!(matches!(
        connect_with_timeout(&target, Duration::from_millis(300)),
        Err(DeviceError::ResizeTimeout { uncertain: false })
    ));
    server.join().unwrap();
}

#[test]
fn responsive_legacy_server_is_unsupported_without_a_resize_request() {
    for reply in [Reply::Raw, Reply::DesktopSize] {
        let (target, server) = server(reply, None);
        let backend = connect_with_timeout(&target, Duration::from_millis(300)).unwrap();
        assert!(!backend.supported().unwrap());
        drop(backend);
        server.join().unwrap();
    }
}

#[test]
fn timeout_uncertainty_tracks_dispatch() {
    assert!(matches!(
        resize_error(&ResizeError::DispatchTimeout),
        DeviceError::ResizeTimeout { uncertain: false }
    ));
    assert!(matches!(
        resize_error(&ResizeError::Timeout),
        DeviceError::ResizeTimeout { uncertain: true }
    ));
}

#[test]
fn disconnect_after_negotiation_invalidates_cached_capability_and_dimensions() {
    let (close, closed) = std::sync::mpsc::channel();
    let (target, server) = server(Reply::Silent, Some(closed));
    let backend = connect_with_timeout(&target, Duration::from_secs(2)).unwrap();
    assert!(backend.supported().unwrap());
    assert_eq!(backend.dimensions().unwrap(), ImageDimensions { width: 1, height: 1 });
    close.send(()).unwrap();
    server.join().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while matches!(backend.supported(), Ok(true)) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(backend.supported(), Err(DeviceError::Unavailable(_))));
    assert!(matches!(backend.dimensions(), Err(DeviceError::Unavailable(_))));
}

#[test]
fn resize_returns_server_confirmed_dimensions() {
    let (target, server) = server(Reply::Resize, None);
    let mut backend = connect_with_timeout(&target, Duration::from_secs(2)).unwrap();
    assert!(backend.supported().unwrap());
    assert_eq!(backend.dimensions().unwrap(), ImageDimensions { width: 1, height: 1 });
    let requested = ImageDimensions { width: 2, height: 3 };
    assert_eq!(backend.resize(requested).unwrap(), requested);
    assert_eq!(backend.dimensions().unwrap(), requested);
    drop(backend);
    server.join().unwrap();
}
