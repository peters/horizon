use super::*;
use std::io::{Read, Write};

fn server(responsive: bool) -> (Target, std::thread::JoinHandle<()>) {
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
                    assert_eq!(refreshes, 1, "resize control must not poll framebuffers");
                    stream.read_exact(&mut [0; 9]).unwrap();
                    if responsive {
                        // One raw pixel is an observed legacy framebuffer response.
                        stream
                            .write_all(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 20, 180, 40, 0])
                            .unwrap();
                    }
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
    let (target, server) = server(false);
    assert!(matches!(
        connect_with_timeout(&target, Duration::from_millis(300)),
        Err(DeviceError::ResizeTimeout { uncertain: false })
    ));
    server.join().unwrap();
}

#[test]
fn responsive_legacy_server_is_unsupported_without_a_resize_request() {
    let (target, server) = server(true);
    let backend = connect_with_timeout(&target, Duration::from_millis(300)).unwrap();
    assert!(!backend.supported().unwrap());
    drop(backend);
    server.join().unwrap();
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
