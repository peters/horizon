//! Control flow against a synthetic receiver over loopback TLS.
use crate::{
    CastClient, DEFAULT_MEDIA_RECEIVER, Error, LiveCast, LiveOptions, LiveState, MediaLoad, MediaStatus, StreamType,
    Transport,
    client::{NS_CONNECTION, NS_HEARTBEAT, PLATFORM_RECEIVER},
    live::STALL_GRACE,
    media::NS_MEDIA,
    proto::{self, CastMessage, Payload},
    receiver::NS_RECEIVER,
};
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

type ServerStream = StreamOwned<ServerConnection, TcpStream>;

fn listen() -> (SocketAddr, TcpListener, Arc<ServerConfig>) {
    let certified = rcgen::generate_simple_self_signed(vec!["receiver".to_owned()]).unwrap();
    let cert = CertificateDer::from(certified.cert.der().to_vec());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der()));
    let config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    (listener.local_addr().unwrap(), listener, Arc::new(config))
}

fn accept(listener: &TcpListener, config: Arc<ServerConfig>) -> ServerStream {
    let (socket, _) = listener.accept().unwrap();
    socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    StreamOwned::new(ServerConnection::new(config).unwrap(), socket)
}

fn send(stream: &mut ServerStream, source: &str, namespace: &str, payload: &Value) {
    let frame = CastMessage::text(source, "sender-0", namespace, payload.to_string())
        .encode_frame()
        .unwrap();
    stream.write_all(&frame).unwrap();
    stream.flush().unwrap();
}

fn reply(request: &Value, mut payload: Value) -> Value {
    payload["requestId"] = request["requestId"].clone();
    payload
}

fn app_status(running: bool) -> Value {
    let applications = if running {
        json!([{
            "appId": DEFAULT_MEDIA_RECEIVER,
            "displayName": "Default Media Receiver",
            "sessionId": "session-1",
            "transportId": "transport-1",
            "namespaces": [{"name": NS_MEDIA}]
        }])
    } else {
        json!([])
    };
    json!({"type": "RECEIVER_STATUS", "status": {"applications": applications, "volume": {"level": 0.5}}})
}

/// Fetches `url` until the body shows a playlist or an MP4 header; live MP4
/// responses never end, so this does not read to the end.
fn fetch(url: &str) -> String {
    let rest = url.strip_prefix("http://").unwrap();
    let (host, path) = rest.split_at(rest.find('/').unwrap());
    let mut socket = TcpStream::connect(host).unwrap();
    socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(socket, "GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
    let mut response = Vec::new();
    let mut chunk = [0; 4096];
    while !(response.windows(7).any(|w| w == b"#EXTM3U") || response.windows(4).any(|w| w == b"ftyp")) {
        let read = socket.read(&mut chunk).unwrap();
        assert!(read > 0, "response ended early: {}", String::from_utf8_lossy(&response));
        response.extend_from_slice(&chunk[..read]);
    }
    let response = String::from_utf8_lossy(&response).into_owned();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    response
}

/// Serves one sender and returns `namespace destination type` for every message it saw.
fn receiver(listener: TcpListener, config: Arc<ServerConfig>) -> JoinHandle<Vec<String>> {
    receiver_with(listener, config, false)
}

/// Like [`receiver`], with the media receiver optionally already running.
fn receiver_with(listener: TcpListener, config: Arc<ServerConfig>, running: bool) -> JoinHandle<Vec<String>> {
    std::thread::spawn(move || {
        let mut running = running;
        let mut stream = accept(&listener, config);
        let mut log = Vec::new();
        let mut inbound = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            let read = stream.read(&mut chunk).unwrap();
            if read == 0 {
                return log;
            }
            inbound.extend_from_slice(&chunk[..read]);
            for message in proto::drain_frames(&mut inbound).unwrap() {
                let Payload::Text(text) = &message.payload else {
                    panic!("binary payload");
                };
                let request: Value = serde_json::from_str(text).unwrap();
                let kind = request["type"].as_str().unwrap_or_default().to_owned();
                log.push(format!("{} {} {kind}", message.namespace, message.destination));
                let to = message.destination.as_str();
                match (message.namespace.as_str(), kind.as_str()) {
                    (NS_RECEIVER, "LAUNCH") if !running => {
                        // A status notice sent before the launch completed can
                        // reach the sender ahead of the LAUNCH reply.
                        let stale = json!({"type": "RECEIVER_STATUS", "requestId": 0, "status": {"applications": [], "volume": {"level": 0.5}}});
                        send(&mut stream, PLATFORM_RECEIVER, NS_RECEIVER, &stale);
                        running = true;
                        send(&mut stream, to, NS_RECEIVER, &reply(&request, app_status(running)));
                    }
                    (NS_RECEIVER, "GET_STATUS" | "STOP" | "LAUNCH") => {
                        running = match kind.as_str() {
                            "LAUNCH" => true,
                            "STOP" => false,
                            _ => running,
                        };
                        send(&mut stream, to, NS_RECEIVER, &reply(&request, app_status(running)));
                    }
                    (NS_MEDIA, "LOAD") => {
                        assert_eq!(request["media"]["streamType"], "LIVE");
                        if let Some(url) = request["media"]["contentId"]
                            .as_str()
                            .filter(|u| u.contains("127.0.0.1"))
                        {
                            let body = fetch(url);
                            let kind = request["media"]["contentType"].as_str().unwrap_or_default();
                            let what = if body.contains("ftyp") { "mp4" } else { "playlist" };
                            log.push(format!("fetched {kind} {what}"));
                        }
                        let loaded = json!({"type": "MEDIA_STATUS", "status": [{"mediaSessionId": 9, "playerState": "BUFFERING"}]});
                        send(&mut stream, to, NS_MEDIA, &reply(&request, loaded));
                        // Receivers can report a bare IDLE for the new item before it loads.
                        let fresh = json!({"type": "MEDIA_STATUS", "requestId": 0, "status": [{"mediaSessionId": 9, "playerState": "IDLE"}]});
                        send(&mut stream, to, NS_MEDIA, &fresh);
                        // TVs report volume changes without an application list.
                        let volume =
                            json!({"type": "RECEIVER_STATUS", "requestId": 0, "status": {"volume": {"level": 0.1}}});
                        send(&mut stream, PLATFORM_RECEIVER, NS_RECEIVER, &volume);
                        let playing = json!({"type": "MEDIA_STATUS", "requestId": 0, "status": [{"mediaSessionId": 9, "playerState": "PLAYING"}]});
                        send(&mut stream, to, NS_MEDIA, &playing);
                        // TVs interleave short BUFFERING reports while playback advances.
                        let blip = json!({"type": "MEDIA_STATUS", "requestId": 0, "status": [{"mediaSessionId": 9, "playerState": "BUFFERING"}]});
                        send(&mut stream, to, NS_MEDIA, &blip);
                        // The item this LOAD replaced reports its end; it must not end our session.
                        let replaced = json!({"type": "MEDIA_STATUS", "requestId": 0, "status": [{"mediaSessionId": 8, "playerState": "IDLE", "idleReason": "INTERRUPTED"}]});
                        send(&mut stream, to, NS_MEDIA, &replaced);
                        send(&mut stream, PLATFORM_RECEIVER, NS_HEARTBEAT, &json!({"type": "PING"}));
                    }
                    (NS_MEDIA, "GET_STATUS" | "SET_PLAYBACK_RATE") => {
                        let rate = request["playbackRate"].as_f64().unwrap_or(1.0);
                        let status = json!({"type": "MEDIA_STATUS", "status": [{"mediaSessionId": 9, "playerState": "PLAYING", "currentTime": 0.0, "playbackRate": rate}]});
                        send(&mut stream, to, NS_MEDIA, &reply(&request, status));
                    }
                    (NS_CONNECTION, "CLOSE") if to == PLATFORM_RECEIVER => return log,
                    _ => {}
                }
            }
        }
    })
}

#[test]
fn launches_loads_and_follows_media_status() {
    let (address, listener, config) = listen();
    let server = receiver(listener, config);
    let client = CastClient::connect(address).unwrap();
    assert!(client.receiver_status().unwrap().applications.is_empty());
    let app = client.launch(DEFAULT_MEDIA_RECEIVER).unwrap();
    assert_eq!(app.transport_id, "transport-1");
    let mut media = client.media(&app);
    let loaded = media
        .load(&MediaLoad {
            url: "http://192.0.2.1/live.m3u8".to_owned(),
            content_type: "application/x-mpegurl".to_owned(),
            stream_type: StreamType::Live,
            ..MediaLoad::default()
        })
        .unwrap();
    assert_eq!(
        (loaded.media_session_id, loaded.player_state.as_str()),
        (9, "BUFFERING")
    );
    let playing = std::iter::from_fn(|| client.next_event(Duration::from_secs(5)).unwrap())
        .flat_map(|event| MediaStatus::from_event(&event))
        .find(|status| status.player_state != "IDLE")
        .unwrap();
    assert_eq!(playing.player_state, "PLAYING");
    client.stop_application(&app.session_id).unwrap();
    drop(client);

    let log = server.join().unwrap();
    for expected in [
        format!("{NS_CONNECTION} receiver-0 CONNECT"),
        format!("{NS_CONNECTION} transport-1 CONNECT"),
        format!("{NS_MEDIA} transport-1 LOAD"),
        format!("{NS_HEARTBEAT} receiver-0 PONG"),
        format!("{NS_RECEIVER} receiver-0 LAUNCH"),
        format!("{NS_CONNECTION} transport-1 CLOSE"),
        format!("{NS_CONNECTION} receiver-0 CLOSE"),
    ] {
        assert!(log.contains(&expected), "missing {expected:?} in {log:#?}");
    }
}

#[test]
fn launch_joins_a_running_application_instead_of_restarting_it() {
    let (address, listener, config) = listen();
    let server = receiver_with(listener, config, true);
    let client = CastClient::connect(address).unwrap();
    let app = client.launch(DEFAULT_MEDIA_RECEIVER).unwrap();
    assert_eq!(app.session_id, "session-1");
    assert!(matches!(client.set_volume(f64::NAN), Err(Error::Protocol(_))));
    drop(client);
    let log = server.join().unwrap();
    assert!(!log.contains(&format!("{NS_RECEIVER} receiver-0 LAUNCH")), "{log:#?}");
    assert!(
        log.contains(&format!("{NS_CONNECTION} transport-1 CONNECT")),
        "{log:#?}"
    );
}

#[test]
fn reports_closed_when_receiver_disconnects() {
    let (address, listener, config) = listen();
    let server = std::thread::spawn(move || {
        let mut stream = accept(&listener, config);
        let mut chunk = [0; 4096];
        // Complete the handshake and read CONNECT, then hang up.
        let _ = stream.read(&mut chunk);
        stream.conn.send_close_notify();
        let _ = stream.flush();
    });
    let client = CastClient::connect(address).unwrap();
    server.join().unwrap();
    assert!(matches!(client.receiver_status(), Err(Error::Closed | Error::Io(_))));
    assert!(!client.is_open());
}

/// SPS and PPS of an x264 1280x720 stream, so progressive init segments build.
const SPS: [u8; 24] = [
    0x67, 0x64, 0x00, 0x1f, 0xac, 0xb2, 0x00, 0xa0, 0x0b, 0x76, 0x02, 0x20, 0x00, 0x00, 0x03, 0x00, 0x20, 0x00, 0x00,
    0x07, 0x81, 0xe3, 0x06, 0x49,
];
const PPS: [u8; 6] = [0x68, 0xeb, 0xc3, 0xcb, 0x22, 0xc0];

fn access_unit(keyframe: bool) -> Vec<u8> {
    let mut unit = Vec::new();
    if keyframe {
        for set in [&SPS[..], &PPS[..]] {
            unit.extend_from_slice(&[0, 0, 0, 1]);
            unit.extend_from_slice(set);
        }
    }
    unit.extend_from_slice(&[0, 0, 0, 1, if keyframe { 0x65 } else { 0x41 }, 0x88]);
    unit
}

/// Casts with frames pushed ten times faster than real time on a separate
/// thread, so a receiver stuck at 0 s falls further behind; `settle` keeps
/// the cast running after PLAYING.
fn cast_live(transport: Transport, settle: Duration) -> Vec<String> {
    let (address, listener, config) = listen();
    let server = receiver(listener, config);
    let options = LiveOptions {
        transport,
        ..LiveOptions::default()
    };
    let mut live = LiveCast::start(address, options).unwrap();
    assert!(live.url().starts_with("http://127.0.0.1:"));
    let pushing = AtomicBool::new(true);
    let states = std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut frame = 0u64;
            while pushing.load(Ordering::Relaxed) {
                let pts = Duration::from_millis(frame * 100);
                let keyframe = live.wants_keyframe(pts);
                live.push_annexb(&access_unit(keyframe), pts, keyframe);
                frame += 1;
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        for _ in 0..100 {
            if live.state() == LiveState::Playing {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let reached = live.state();
        std::thread::sleep(Duration::from_millis(300).max(settle));
        let settled = live.state();
        // Stop the producer before asserting, or a failure would leave the scope waiting on it.
        pushing.store(false, Ordering::Relaxed);
        (reached, settled)
    });
    assert_eq!(states.0, LiveState::Playing);
    assert_eq!(
        states.1,
        LiveState::Playing,
        "a brief BUFFERING report must not flap the state"
    );
    live.stop();
    assert_eq!(live.state(), LiveState::Ended);
    let log = server.join().unwrap();
    assert!(log.contains(&format!("{NS_RECEIVER} receiver-0 STOP")), "{log:#?}");
    log
}

#[test]
fn live_cast_loads_the_served_playlist_and_tracks_playback() {
    let log = cast_live(Transport::Hls, Duration::ZERO);
    assert!(
        log.contains(&"fetched application/x-mpegurl playlist".to_owned()),
        "{log:#?}"
    );
    assert!(
        !log.iter().any(|line| line.ends_with("SET_PLAYBACK_RATE")),
        "HLS never changes the rate"
    );
}

#[test]
fn progressive_cast_streams_mp4_and_catches_up_with_live() {
    let log = cast_live(Transport::Progressive, Duration::from_millis(2500));
    assert!(log.contains(&"fetched video/mp4 mp4".to_owned()), "{log:#?}");
    // Frames keep arriving while the receiver reports 0 s, so it falls behind.
    assert!(
        log.contains(&format!("{NS_MEDIA} transport-1 SET_PLAYBACK_RATE")),
        "{log:#?}"
    );
}

#[test]
fn live_cast_rejects_options_that_could_never_play() {
    let address: SocketAddr = "127.0.0.1:9".parse().unwrap();
    for options in [
        LiveOptions {
            segment: Duration::ZERO,
            ..LiveOptions::default()
        },
        LiveOptions {
            transport: Transport::Hls,
            preroll: 0,
            ..LiveOptions::default()
        },
        LiveOptions {
            transport: Transport::Hls,
            window: 2,
            preroll: 3,
            ..LiveOptions::default()
        },
    ] {
        assert!(matches!(
            LiveCast::start(address, options),
            Err(Error::InvalidOptions(_))
        ));
    }
}

#[test]
fn live_cast_ends_without_load_when_the_application_closes_during_pre_roll() {
    let (address, listener, config) = listen();
    let server = std::thread::spawn(move || {
        let mut stream = accept(&listener, config);
        let (mut log, mut inbound, mut chunk) = (Vec::new(), Vec::new(), [0; 4096]);
        loop {
            let read = stream.read(&mut chunk).unwrap_or(0);
            if read == 0 {
                return log;
            }
            inbound.extend_from_slice(&chunk[..read]);
            for message in proto::drain_frames(&mut inbound).unwrap() {
                let Payload::Text(text) = &message.payload else {
                    continue;
                };
                let request: Value = serde_json::from_str(text).unwrap();
                let kind = request["type"].as_str().unwrap_or_default().to_owned();
                log.push(format!("{} {} {kind}", message.namespace, message.destination));
                match (message.namespace.as_str(), kind.as_str()) {
                    (NS_RECEIVER, "GET_STATUS") => {
                        send(
                            &mut stream,
                            PLATFORM_RECEIVER,
                            NS_RECEIVER,
                            &reply(&request, app_status(false)),
                        );
                    }
                    (NS_RECEIVER, "LAUNCH") => {
                        send(
                            &mut stream,
                            PLATFORM_RECEIVER,
                            NS_RECEIVER,
                            &reply(&request, app_status(true)),
                        );
                        send(&mut stream, "transport-1", NS_CONNECTION, &json!({"type": "CLOSE"}));
                    }
                    (NS_CONNECTION, "CLOSE") if message.destination == PLATFORM_RECEIVER => return log,
                    _ => {}
                }
            }
        }
    });
    let live = LiveCast::start(address, LiveOptions::default()).unwrap();
    for _ in 0..100 {
        if live.state() == LiveState::Ended {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(live.state(), LiveState::Ended);
    drop(live);
    let log = server.join().unwrap();
    assert!(!log.iter().any(|line| line.ends_with(" LOAD")), "{log:#?}");
    assert!(!log.iter().any(|line| line.ends_with(" STOP")), "{log:#?}");
}

fn media_event(source: &str, statuses: &Value) -> crate::Event {
    crate::Event {
        namespace: NS_MEDIA.to_owned(),
        source: source.to_owned(),
        payload: json!({"type": "MEDIA_STATUS", "requestId": 0, "status": statuses.clone()}),
    }
}

#[test]
fn live_session_ends_on_bare_idle_and_on_a_newer_load_but_not_on_the_replaced_item() {
    let session = crate::live::test_session();
    let follow =
        |statuses: Value| session.follow(&media_event("transport-1", &statuses), "session-1", "transport-1", 9);
    // The item our LOAD replaced may still report its end.
    assert!(follow(json!([{"mediaSessionId": 8, "playerState": "IDLE", "idleReason": "INTERRUPTED"}])).unwrap());
    assert!(follow(json!([{"mediaSessionId": 8, "playerState": "IDLE"}])).unwrap());
    // A bare IDLE right after LOAD, before the item has played, is not the end.
    assert!(follow(json!([{"mediaSessionId": 9, "playerState": "BUFFERING"}])).unwrap());
    assert!(follow(json!([{"mediaSessionId": 9, "playerState": "IDLE"}])).unwrap());
    assert!(follow(json!([{"mediaSessionId": 9, "playerState": "PLAYING"}])).unwrap());
    // Once it has played, a bare IDLE ends the cast.
    assert!(!follow(json!([{"mediaSessionId": 9, "playerState": "IDLE"}])).unwrap());

    let replaced = crate::live::test_session();
    let event = media_event(
        "transport-1",
        &json!([{"mediaSessionId": 10, "playerState": "BUFFERING"}]),
    );
    assert!(
        !replaced.follow(&event, "session-1", "transport-1", 9).unwrap(),
        "another sender loaded over us"
    );
}

#[test]
fn live_cast_ends_without_stop_when_a_confirmed_takeover_replaces_the_application() {
    let other = json!({"type": "RECEIVER_STATUS", "status": {"applications": [{
        "appId": "OTHER", "displayName": "Other", "sessionId": "session-2", "transportId": "transport-2", "namespaces": []
    }]}});
    let (address, listener, config) = listen();
    let server = std::thread::spawn(move || {
        let mut stream = accept(&listener, config);
        let (mut log, mut inbound, mut chunk, mut launched) = (Vec::new(), Vec::new(), [0; 4096], false);
        loop {
            let read = stream.read(&mut chunk).unwrap_or(0);
            if read == 0 {
                return log;
            }
            inbound.extend_from_slice(&chunk[..read]);
            for message in proto::drain_frames(&mut inbound).unwrap() {
                let Payload::Text(text) = &message.payload else {
                    continue;
                };
                let request: Value = serde_json::from_str(text).unwrap();
                let kind = request["type"].as_str().unwrap_or_default().to_owned();
                log.push(format!("{} {} {kind}", message.namespace, message.destination));
                match (message.namespace.as_str(), kind.as_str()) {
                    (NS_RECEIVER, "GET_STATUS") if launched => {
                        send(
                            &mut stream,
                            PLATFORM_RECEIVER,
                            NS_RECEIVER,
                            &reply(&request, other.clone()),
                        );
                    }
                    (NS_RECEIVER, "GET_STATUS") => {
                        send(
                            &mut stream,
                            PLATFORM_RECEIVER,
                            NS_RECEIVER,
                            &reply(&request, app_status(false)),
                        );
                    }
                    (NS_RECEIVER, "LAUNCH") => {
                        launched = true;
                        send(
                            &mut stream,
                            PLATFORM_RECEIVER,
                            NS_RECEIVER,
                            &reply(&request, app_status(true)),
                        );
                        let mut notice = other.clone();
                        notice["requestId"] = 0.into();
                        send(&mut stream, PLATFORM_RECEIVER, NS_RECEIVER, &notice);
                    }
                    (NS_CONNECTION, "CLOSE") if message.destination == PLATFORM_RECEIVER => return log,
                    _ => {}
                }
            }
        }
    });
    let live = LiveCast::start(address, LiveOptions::default()).unwrap();
    for _ in 0..100 {
        if live.state() == LiveState::Ended {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let state = live.state();
    drop(live);
    let log = server.join().unwrap();
    assert_eq!(state, LiveState::Ended, "{log:#?}");
    assert!(
        !log.iter()
            .any(|line| line.ends_with(" STOP") || line.ends_with(" LOAD")),
        "{log:#?}"
    );
}

/// What the synthetic receiver does after a PLAYING LOAD reply.
#[derive(Clone, Copy, PartialEq)]
enum AfterLoad {
    KeepPlaying,
    /// Reports that no media is loaded.
    Unload,
    /// Sends a bare IDLE; a fresh status then reports a playback error.
    Error,
    /// Reports an error for our session while a fresh status shows another
    /// sender's replacement.
    ErrorThenReplaced,
    /// Queues BUFFERING ahead of the PLAYING LOAD reply and then stays quiet.
    BufferingFirst,
}

/// A receiver that queues a bare IDLE ahead of a PLAYING LOAD reply.
fn ordering_receiver(listener: TcpListener, config: Arc<ServerConfig>, after: AfterLoad) -> JoinHandle<Vec<String>> {
    std::thread::spawn(move || {
        let mut stream = accept(&listener, config);
        let (mut log, mut inbound, mut chunk) = (Vec::new(), Vec::new(), [0; 4096]);
        let (mut running, mut loaded) = (false, false);
        loop {
            let read = stream.read(&mut chunk).unwrap_or(0);
            if read == 0 {
                return log;
            }
            inbound.extend_from_slice(&chunk[..read]);
            for message in proto::drain_frames(&mut inbound).unwrap() {
                let Payload::Text(text) = &message.payload else {
                    continue;
                };
                let request: Value = serde_json::from_str(text).unwrap();
                let kind = request["type"].as_str().unwrap_or_default().to_owned();
                log.push(format!("{} {} {kind}", message.namespace, message.destination));
                let to = message.destination.as_str();
                let playing = json!([{"mediaSessionId": 9, "playerState": "PLAYING"}]);
                match (message.namespace.as_str(), kind.as_str()) {
                    (NS_RECEIVER, "GET_STATUS") => {
                        send(
                            &mut stream,
                            PLATFORM_RECEIVER,
                            NS_RECEIVER,
                            &reply(&request, app_status(running)),
                        );
                    }
                    (NS_RECEIVER, "LAUNCH") => {
                        running = true;
                        send(
                            &mut stream,
                            PLATFORM_RECEIVER,
                            NS_RECEIVER,
                            &reply(&request, app_status(true)),
                        );
                    }
                    (NS_RECEIVER, "STOP") => {
                        running = false;
                        send(
                            &mut stream,
                            PLATFORM_RECEIVER,
                            NS_RECEIVER,
                            &reply(&request, app_status(false)),
                        );
                    }
                    (NS_MEDIA, "LOAD") => {
                        let early = if after == AfterLoad::BufferingFirst {
                            "BUFFERING"
                        } else {
                            "IDLE"
                        };
                        let idle = json!({"type": "MEDIA_STATUS", "requestId": 0, "status": [{"mediaSessionId": 9, "playerState": early}]});
                        send(&mut stream, to, NS_MEDIA, &idle);
                        let status = json!({"type": "MEDIA_STATUS", "status": playing});
                        send(&mut stream, to, NS_MEDIA, &reply(&request, status));
                        loaded = true;
                        if after == AfterLoad::Error {
                            send(&mut stream, to, NS_MEDIA, &idle);
                        }
                        if after == AfterLoad::ErrorThenReplaced {
                            let error = json!({"type": "MEDIA_STATUS", "requestId": 0, "status": [{"mediaSessionId": 9, "playerState": "IDLE", "idleReason": "ERROR"}]});
                            send(&mut stream, to, NS_MEDIA, &error);
                        }
                        if after == AfterLoad::Unload {
                            let empty = json!({"type": "MEDIA_STATUS", "requestId": 0, "status": []});
                            send(&mut stream, to, NS_MEDIA, &empty);
                            loaded = false;
                        }
                    }
                    (NS_MEDIA, "GET_STATUS") => {
                        let current = match (loaded, after) {
                            (true, AfterLoad::Error) => {
                                json!([{"mediaSessionId": 9, "playerState": "IDLE", "idleReason": "ERROR"}])
                            }
                            (true, AfterLoad::ErrorThenReplaced) => {
                                json!([{"mediaSessionId": 10, "playerState": "PLAYING"}])
                            }
                            (true, _) => playing,
                            (false, _) => json!([]),
                        };
                        let status = json!({"type": "MEDIA_STATUS", "status": current});
                        send(&mut stream, to, NS_MEDIA, &reply(&request, status));
                    }
                    (NS_CONNECTION, "CLOSE") if to == PLATFORM_RECEIVER => return log,
                    _ => {}
                }
            }
        }
    })
}

fn cast_against(server: JoinHandle<Vec<String>>, address: SocketAddr) -> (LiveState, Vec<String>) {
    cast_for(server, address, Duration::from_millis(1500))
}

fn cast_for(server: JoinHandle<Vec<String>>, address: SocketAddr, run: Duration) -> (LiveState, Vec<String>) {
    // These bare access units carry no SPS, which the progressive stream needs.
    let options = LiveOptions {
        transport: Transport::Hls,
        ..LiveOptions::default()
    };
    let live = LiveCast::start(address, options).unwrap();
    for frame in 0..30u64 {
        let pts = Duration::from_millis(frame * 100);
        let keyframe = live.wants_keyframe(pts);
        let unit: &[u8] = if keyframe {
            &[0, 0, 0, 1, 0x65, 1]
        } else {
            &[0, 0, 0, 1, 0x41, 2]
        };
        live.push_annexb(unit, pts, keyframe);
    }
    std::thread::sleep(run);
    let state = live.state();
    drop(live);
    (state, server.join().unwrap())
}

#[test]
fn a_stale_idle_queued_before_a_playing_load_reply_does_not_end_the_cast() {
    let (address, listener, config) = listen();
    let (state, log) = cast_against(ordering_receiver(listener, config, AfterLoad::KeepPlaying), address);
    assert_eq!(state, LiveState::Playing, "{log:#?}");
}

#[test]
fn a_confirmed_unload_after_playing_ends_the_cast_without_stop() {
    let (address, listener, config) = listen();
    let (state, log) = cast_against(ordering_receiver(listener, config, AfterLoad::Unload), address);
    assert_eq!(state, LiveState::Ended, "{log:#?}");
    assert!(log.contains(&format!("{NS_MEDIA} transport-1 GET_STATUS")), "{log:#?}");
    assert!(!log.iter().any(|line| line.ends_with(" STOP")), "{log:#?}");
}

#[test]
fn live_options_must_keep_three_target_durations() {
    let address: SocketAddr = "127.0.0.1:9".parse().unwrap();
    let short = LiveOptions {
        transport: Transport::Hls,
        segment: Duration::from_secs(1),
        window: 2,
        preroll: 2,
        ..LiveOptions::default()
    };
    assert!(matches!(LiveCast::start(address, short), Err(Error::InvalidOptions(_))));
}

#[test]
fn a_confirmed_playback_error_fails_the_cast() {
    let (address, listener, config) = listen();
    let (state, log) = cast_against(ordering_receiver(listener, config, AfterLoad::Error), address);
    assert!(matches!(state, LiveState::Failed(_)), "{state:?} {log:#?}");
}

#[test]
fn a_queued_error_is_confirmed_and_a_replacement_is_left_running() {
    let (address, listener, config) = listen();
    let (state, log) = cast_against(
        ordering_receiver(listener, config, AfterLoad::ErrorThenReplaced),
        address,
    );
    assert_eq!(state, LiveState::Ended, "{log:#?}");
    assert!(!log.iter().any(|line| line.ends_with(" STOP")), "{log:#?}");
}

#[test]
fn a_buffering_report_queued_before_a_playing_reply_is_not_a_stall() {
    let (address, listener, config) = listen();
    let server = ordering_receiver(listener, config, AfterLoad::BufferingFirst);
    let (state, log) = cast_for(server, address, STALL_GRACE + Duration::from_secs(1));
    assert_eq!(state, LiveState::Playing, "{log:#?}");
}

#[test]
fn avcc_conversion_reports_the_crate_error_type() {
    let result: crate::Result<Vec<u8>> = crate::avcc_to_annexb(&[0, 0, 0, 9, 1], 4, &[]);
    assert!(matches!(result, Err(Error::H264(_))));
}

#[test]
fn live_cast_retries_a_receiver_that_is_not_listening_yet() {
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = probe.local_addr().unwrap();
    drop(probe);
    let late = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1200));
        let (_, _, config) = listen();
        receiver(TcpListener::bind(address).unwrap(), config).join().unwrap()
    });
    let options = LiveOptions {
        transport: Transport::Hls,
        ..LiveOptions::default()
    };
    let mut live = LiveCast::start(address, options).unwrap();
    for frame in 0..60u64 {
        let pts = Duration::from_millis(frame * 100);
        let keyframe = live.wants_keyframe(pts);
        live.push_annexb(&access_unit(keyframe), pts, keyframe);
        if live.state() == LiveState::Playing {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(live.state(), LiveState::Playing);
    live.stop();
    let log = late.join().unwrap();
    assert!(log.contains(&format!("{NS_MEDIA} transport-1 LOAD")), "{log:#?}");
}
