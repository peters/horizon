//! Control flow against a synthetic receiver over loopback TLS.
use crate::{
    CastClient, DEFAULT_MEDIA_RECEIVER, Error, LiveCast, LiveOptions, LiveState, MediaLoad, MediaStatus, StreamType,
    client::{NS_CONNECTION, NS_HEARTBEAT, PLATFORM_RECEIVER},
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
    sync::Arc,
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

fn fetch(url: &str) -> String {
    let rest = url.strip_prefix("http://").unwrap();
    let (host, path) = rest.split_at(rest.find('/').unwrap());
    let mut socket = TcpStream::connect(host).unwrap();
    write!(socket, "GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
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
                            assert!(fetch(url).contains("#EXTM3U"));
                            log.push("fetched playlist".to_owned());
                        }
                        let loaded = json!({"type": "MEDIA_STATUS", "status": [{"mediaSessionId": 9, "playerState": "BUFFERING"}]});
                        send(&mut stream, to, NS_MEDIA, &reply(&request, loaded));
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
        .next()
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

#[test]
fn live_cast_loads_the_served_playlist_and_tracks_playback() {
    let (address, listener, config) = listen();
    let server = receiver(listener, config);
    let mut live = LiveCast::start(address, LiveOptions::default()).unwrap();
    assert!(live.url().starts_with("http://127.0.0.1:"));
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
    for _ in 0..100 {
        if live.state() == LiveState::Playing {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(live.state(), LiveState::Playing);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        live.state(),
        LiveState::Playing,
        "a brief BUFFERING report must not flap the state"
    );
    live.stop();
    assert_eq!(live.state(), LiveState::Ended);
    let log = server.join().unwrap();
    assert!(log.contains(&"fetched playlist".to_owned()), "{log:#?}");
    assert!(log.contains(&format!("{NS_RECEIVER} receiver-0 STOP")), "{log:#?}");
}
