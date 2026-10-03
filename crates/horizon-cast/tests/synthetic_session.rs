//! Optional interoperability harness, against explicitly started loopback receivers only.
use horizon_cast::{CastSession, CastStatus, Orientation, PairingStore, Resolution, VideoFormat};
use std::{
    net::{IpAddr, SocketAddr},
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires two synthetic receivers on distinct loopback addresses"]
fn encodes_two_receivers_and_stops_each_independently() {
    let addresses: Vec<SocketAddr> = std::env::var("HORIZON_CAST_TEST_RECEIVERS")
        .expect("synthetic receiver addresses")
        .split(',')
        .map(|s| s.parse().expect("address"))
        .collect();
    assert_eq!(addresses.len(), 2);
    assert!(addresses.iter().all(|a| match a.ip() {
        IpAddr::V4(ip) => ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_loopback(),
    }));
    let orientations = [Orientation::Landscape, Orientation::Portrait];
    let sessions: Vec<_> = addresses
        .iter()
        .zip(orientations)
        .map(|(address, orientation)| {
            CastSession::start(
                *address,
                VideoFormat {
                    orientation,
                    resolution: Resolution::Hd720,
                },
            )
            .expect("start")
        })
        .collect();
    let started = Instant::now();
    let mut stopped_first = false;
    let mut second_frames_at_stop = 0;
    while started.elapsed() < Duration::from_secs(12) {
        for (index, session) in sessions.iter().enumerate() {
            match session.status() {
                CastStatus::PinRequired => session
                    .pair(zeroize::Zeroizing::new("1234".to_owned()))
                    .expect("synthetic PIN"),
                CastStatus::Streaming { .. } if index != 0 || !stopped_first => {
                    if let Ok(expected) = std::env::var("HORIZON_CAST_TEST_ENCODER") {
                        let selection = session.encoding().expect("encoder selected");
                        assert_eq!(selection.backend.as_str(), expected);
                        if expected == "libx264" && cfg!(feature = "nvenc") {
                            assert!(selection.fallback_reason.is_some());
                        }
                    }
                    let (w, h) = VideoFormat {
                        orientation: orientations[index],
                        resolution: Resolution::Hd720,
                    }
                    .dimensions();
                    let mut rgba = vec![0; usize::from(w) * usize::from(h) * 4];
                    for (at, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        pixel.copy_from_slice(&[
                            u8::try_from(at % 255).expect("range"),
                            u8::try_from(started.elapsed().as_millis() % 255).expect("range"),
                            80,
                            255,
                        ]);
                    }
                    session.submit(rgba).expect("submit");
                }
                CastStatus::Failed(error) => panic!("session {index}: {error}"),
                _ => {}
            }
        }
        if started.elapsed() > Duration::from_secs(6) && !stopped_first {
            assert!(matches!(sessions[0].status(),CastStatus::Streaming{frames} if frames>5));
            if let CastStatus::Streaming { frames } = sessions[1].status() {
                second_frames_at_stop = frames;
            }
            sessions[0].stop();
            stopped_first = true;
        }
        std::thread::sleep(Duration::from_millis(65));
    }
    assert_eq!(sessions[0].status(), CastStatus::Stopped);
    assert!(matches!(sessions[1].status(),CastStatus::Streaming{frames} if frames > second_frames_at_stop + 5));
    sessions[1].stop();
    let deadline = Instant::now() + Duration::from_secs(6);
    while !sessions.iter().all(CastSession::finished) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(sessions.iter().all(CastSession::finished));
    assert_eq!(sessions[1].status(), CastStatus::Stopped);
}

#[test]
#[ignore = "requires synthetic receivers on loopback addresses"]
fn missing_frames_fail_and_release_the_receiver() {
    let address: SocketAddr = std::env::var("HORIZON_CAST_TEST_RECEIVERS")
        .expect("synthetic receivers")
        .split(',')
        .next()
        .expect("first receiver")
        .parse()
        .expect("address");
    assert!(address.ip().is_loopback());
    let session = CastSession::start(
        address,
        VideoFormat {
            orientation: Orientation::Landscape,
            resolution: Resolution::Hd720,
        },
    )
    .expect("start");
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match session.status() {
            CastStatus::PinRequired => session
                .pair(zeroize::Zeroizing::new("1234".into()))
                .expect("synthetic PIN"),
            CastStatus::Failed(error) => {
                assert!(error.contains("capture stopped producing frames"), "{error}");
                break;
            }
            _ => {}
        }
        assert!(Instant::now() < deadline, "capture watchdog did not stop the worker");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(session.finished());
    let retry = CastSession::start(
        address,
        VideoFormat {
            orientation: Orientation::Portrait,
            resolution: Resolution::Hd720,
        },
    )
    .expect("retry");
    let deadline = Instant::now() + Duration::from_secs(3);
    while retry.status() != CastStatus::PinRequired {
        assert!(
            Instant::now() < deadline,
            "receiver reservation was not released: {:?}",
            retry.status()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    retry.stop();
    while !retry.finished() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(retry.status(), CastStatus::Stopped);
}

#[test]
#[ignore = "requires a loopback receiver that remembers paired controller identities"]
fn restarts_with_saved_pairing_and_requests_a_pin_only_after_forget() {
    let address: SocketAddr = std::env::var("HORIZON_CAST_TEST_RECEIVERS")
        .expect("receivers")
        .split(',')
        .next()
        .expect("first")
        .parse()
        .expect("address");
    assert!(address.ip().is_loopback());
    let home = tempfile::tempdir().expect("private home");
    let store = PairingStore::new(
        home.path().join("pairings"),
        "synthetic-tv".into(),
        "Remembered TV".into(),
    );
    for attempt in 0..2 {
        let session = CastSession::start_remembered(
            address,
            VideoFormat {
                orientation: Orientation::Portrait,
                resolution: Resolution::Hd720,
            },
            store.clone(),
        )
        .expect("start");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match session.status() {
                CastStatus::PinRequired => {
                    assert_eq!(attempt, 0, "saved pairing unexpectedly requested a second PIN");
                    session
                        .pair(zeroize::Zeroizing::new("1234".into()))
                        .expect("initial PIN");
                }
                CastStatus::Streaming { frames } => {
                    if frames > 8 {
                        break;
                    }
                    session.submit(vec![80; 720 * 1280 * 4]).expect("frame");
                }
                CastStatus::Failed(error) => panic!("session failed: {error}"),
                _ => {}
            }
            assert!(Instant::now() < deadline, "stream did not start");
            std::thread::sleep(Duration::from_millis(65));
        }
        session.stop();
        while !session.finished() {
            assert!(Instant::now() < deadline, "session did not release TV");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let devices = PairingStore::list(&home.path().join("pairings")).expect("list");
    assert_eq!(devices[0].name, "Remembered TV");
    store.forget().expect("forget");
    assert!(
        PairingStore::list(&home.path().join("pairings"))
            .expect("list after delete")
            .is_empty()
    );
    let session = CastSession::start_remembered(
        address,
        VideoFormat {
            orientation: Orientation::Landscape,
            resolution: Resolution::Hd720,
        },
        store,
    )
    .expect("start forgotten TV");
    let deadline = Instant::now() + Duration::from_secs(5);
    while session.status() != CastStatus::PinRequired {
        assert!(Instant::now() < deadline, "fresh PIN not requested");
        std::thread::sleep(Duration::from_millis(20));
    }
    session.stop();
    while !session.finished() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "requires a loopback receiver, usable CUDA/NVENC and independent H.264 decoding"]
fn gpu_scaling_preserves_each_output_canvas_across_source_geometry_changes() {
    let address: SocketAddr = std::env::var("HORIZON_CAST_TEST_RECEIVERS")
        .expect("receiver")
        .split(',')
        .next()
        .expect("first")
        .parse()
        .expect("address");
    assert!(address.ip().is_loopback());
    let home = tempfile::tempdir().expect("private state");
    let store = PairingStore::new(
        home.path().join("pairings"),
        "gpu-scaling-fixture".into(),
        "Synthetic GPU TV".into(),
    );
    for resolution in [Resolution::Hd720, Resolution::FullHd1080, Resolution::Uhd4k] {
        for orientation in [Orientation::Landscape, Orientation::Portrait] {
            let format = VideoFormat {
                orientation,
                resolution,
            };
            let session = CastSession::start_remembered(address, format, store.clone()).expect("start");
            let deadline = Instant::now() + Duration::from_secs(12);
            let mut submitted = 0;
            loop {
                match session.status() {
                    CastStatus::PinRequired => session
                        .pair(zeroize::Zeroizing::new("1234".into()))
                        .expect("synthetic PIN"),
                    CastStatus::Streaming { frames } => {
                        let selection = session.encoding().expect("backend");
                        assert_eq!(selection.backend.scaler(), "cuda");
                        assert!(selection.fallback_reason.is_none());
                        if frames >= 35 {
                            break;
                        }
                        let (width, height, pixel) = match submitted / 15 % 3 {
                            0 => (641, 361, [210, 40, 40, 255]),
                            1 => (1023, 767, [40, 210, 40, 255]),
                            _ => (719, 1279, [40, 40, 210, 255]),
                        };
                        session
                            .submit_source(width, height, pixel.repeat(usize::from(width) * usize::from(height)))
                            .expect("source crop");
                        submitted += 1;
                    }
                    CastStatus::Failed(error) => panic!("{error}"),
                    _ => {}
                }
                assert!(Instant::now() < deadline, "source stream did not advance");
                std::thread::sleep(Duration::from_millis(67));
            }
            session.stop();
            while !session.finished() {
                assert!(Instant::now() < deadline, "receiver lease remained busy");
                std::thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(session.status(), CastStatus::Stopped);
            println!(
                "source geometry qualification {}x{}: {} frames sent",
                format.dimensions().0,
                format.dimensions().1,
                session.frames_sent()
            );
        }
    }
    store.forget().expect("final receiver lease released");
}
