use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::json;

use super::{RemoteAuthorizationHeader, RemoteHttpClient};
use crate::webdriver::http::HttpError;
use crate::webdriver::transport::ClassicTransport;

/// One scripted reply: status, body bytes, and optional extra header lines.
struct Reply {
    status: u16,
    body: Vec<u8>,
    headers: Vec<String>,
    delay: Duration,
    /// Pause between the header block and the body, to stall a body read.
    body_delay: Duration,
    declared_length: Option<usize>,
}

impl Reply {
    fn json(status: u16, body: &serde_json::Value) -> Self {
        Self {
            status,
            body: body.to_string().into_bytes(),
            headers: vec!["Content-Type: application/json".into()],
            delay: Duration::ZERO,
            body_delay: Duration::ZERO,
            declared_length: None,
        }
    }
}

struct Recorded {
    method: String,
    path: String,
    authorization: Option<String>,
    body: String,
}

/// Loopback HTTP/1.1 responder that answers one connection per scripted reply
/// and records what it saw.
struct Server {
    port: u16,
    seen: Arc<Mutex<Vec<Recorded>>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let handle = thread::spawn(move || {
            for reply in replies {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head_end, content_length) = loop {
                    let Ok(read) = stream.read(&mut chunk) else { return };
                    if read == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buffer[..end]).to_string();
                        let length = head
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while buffer.len() < head_end + content_length {
                    let Ok(read) = stream.read(&mut chunk) else { break };
                    if read == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                }
                let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
                let mut lines = head.lines();
                let request_line = lines.next().unwrap_or_default();
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let path = parts.next().unwrap_or_default().to_string();
                let authorization = lines.find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("authorization")
                        .then(|| value.trim().to_string())
                });
                let body = String::from_utf8_lossy(&buffer[head_end..]).to_string();
                recorder.lock().expect("lock").push(Recorded {
                    method,
                    path,
                    authorization,
                    body,
                });
                thread::sleep(reply.delay);
                let length = reply.declared_length.unwrap_or(reply.body.len());
                let mut response = format!(
                    "HTTP/1.1 {} Reply\r\nContent-Length: {length}\r\nConnection: close\r\n",
                    reply.status
                );
                for header in &reply.headers {
                    response.push_str(header);
                    response.push_str("\r\n");
                }
                response.push_str("\r\n");
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
                thread::sleep(reply.body_delay);
                let _ = stream.write_all(&reply.body);
                let _ = stream.flush();
            }
        });
        Self {
            port,
            seen,
            handle: Some(handle),
        }
    }

    fn endpoint(&self, base_path: &str) -> String {
        format!("http://127.0.0.1:{}{base_path}", self.port)
    }

    fn recorded(&self) -> Vec<(String, String, Option<String>, String)> {
        self.seen
            .lock()
            .expect("lock")
            .iter()
            .map(|seen| {
                (
                    seen.method.clone(),
                    seen.path.clone(),
                    seen.authorization.clone(),
                    seen.body.clone(),
                )
            })
            .collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Unblock accept() if a test ended early, then join.
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn header(value: &str) -> RemoteAuthorizationHeader {
    RemoteAuthorizationHeader::new(value.to_string()).expect("header")
}

#[test]
fn authorization_and_base_path_reach_the_hub_only() {
    let server = Server::start(vec![Reply::json(
        200,
        &json!({"value": {"sessionId": "abc", "capabilities": {}}}),
    )]);
    let client = RemoteHttpClient::new(&server.endpoint("/wd/hub/"), Some(header("Basic c2VjcmV0"))).expect("client");
    let value = client
        .post("/session", &json!({"capabilities": {"alwaysMatch": {}}}))
        .expect("session created");
    assert_eq!(value["value"]["sessionId"], "abc");
    let seen = server.recorded();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "POST");
    assert_eq!(
        seen[0].1, "/wd/hub/session",
        "base path is prepended and the trailing slash dropped"
    );
    assert_eq!(seen[0].2.as_deref(), Some("Basic c2VjcmV0"));
    assert!(seen[0].3.contains("alwaysMatch"));
    assert_eq!(client.origin(), format!("http://127.0.0.1:{}", server.port));
}

#[test]
fn requests_without_authorization_carry_no_header() {
    let server = Server::start(vec![Reply::json(200, &json!({"value": null}))]);
    let client = RemoteHttpClient::new(&server.endpoint(""), None).expect("client");
    client.delete("/session/abc").expect("deleted");
    let seen = server.recorded();
    assert_eq!(seen[0].0, "DELETE");
    assert_eq!(seen[0].1, "/session/abc");
    assert_eq!(seen[0].2, None);
}

#[test]
fn endpoint_rules_match_the_configuration_contract() {
    for endpoint in [
        "http://grid.example.net/wd/hub",
        "https://user:key@grid.example.net/wd/hub",
        "https://@grid.example.net/wd/hub",
        "https://grid.example.net/wd/hub?key=1",
        "https://grid.example.net/wd/hub#x",
        "grid.example.net",
        "ftp://127.0.0.1/",
    ] {
        assert!(RemoteHttpClient::new(endpoint, None).is_err(), "{endpoint}");
    }
    for endpoint in [
        "https://grid.example.net:notaport/wd/hub",
        "https://grid.example.net:99999",
        "https://",
    ] {
        assert!(RemoteHttpClient::new(endpoint, None).is_err(), "{endpoint}");
    }
    let remote = RemoteHttpClient::new("https://Grid.Example.net:8443/wd/hub/", None).expect("https endpoint");
    assert_eq!(remote.origin(), "https://grid.example.net:8443");
    assert!(remote.config().https_only(), "non-loopback endpoints force TLS");
    assert_eq!(remote.config().max_redirects(), 0);
    let local = RemoteHttpClient::new("http://[::1]:4723", None).expect("loopback http");
    assert!(!local.config().https_only());
    let named = RemoteHttpClient::new("http://LOCALHOST:4723/wd/hub", None).expect("case-insensitive localhost");
    assert!(!named.config().https_only());
    assert_eq!(named.origin(), "http://localhost:4723");
}

#[test]
fn request_paths_cannot_escape_the_base() {
    let server = Server::start(Vec::new());
    let client = RemoteHttpClient::new(&server.endpoint("/wd/hub"), None).expect("client");
    for path in [
        "session",
        "/session/../status",
        "/session?x=1",
        "/session#f",
        "//other.host/session",
        "/a b",
        "/session/%2e%2e/%2e%2e/status",
        "/session%2Fabc",
        "/session\tx",
        "/session\u{7f}",
        "/s\u{0}n",
        "/sessi\u{f3}n",
    ] {
        assert!(matches!(client.get(path), Err(HttpError::InvalidResponse(_))), "{path}");
    }
    assert!(server.recorded().is_empty(), "no request was sent");
}

#[test]
fn redirects_are_not_followed_and_never_carry_the_credential_elsewhere() {
    let mut reply = Reply::json(302, &json!({}));
    reply.headers.push("Location: http://127.0.0.1:1/elsewhere".into());
    let server = Server::start(vec![reply]);
    let client = RemoteHttpClient::new(&server.endpoint(""), Some(header("Bearer t"))).expect("client");
    let error = client.get("/status").expect_err("redirect is a failure");
    assert!(
        matches!(error, HttpError::WebDriver { .. } | HttpError::InvalidResponse(_)),
        "{error}"
    );
    assert_eq!(server.recorded().len(), 1, "exactly one request, to the hub");
}

#[test]
fn webdriver_errors_keep_their_codes_including_expired_sessions_and_rate_limits() {
    let server = Server::start(vec![
        Reply::json(
            404,
            &json!({"value": {"error": "invalid session id", "message": "session gone"}}),
        ),
        Reply {
            status: 429,
            body: b"slow down".to_vec(),
            headers: vec!["Content-Type: text/plain".into()],
            delay: Duration::ZERO,
            body_delay: Duration::ZERO,
            declared_length: None,
        },
    ]);
    let client = RemoteHttpClient::new(&server.endpoint(""), None).expect("client");
    match client.get("/session/gone/url").expect_err("expired") {
        HttpError::WebDriver { error, message } => {
            assert_eq!(error, "invalid session id");
            assert_eq!(message, "session gone");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(matches!(
        client.get("/status").expect_err("rate limited"),
        HttpError::Json(_)
    ));
}

#[test]
fn malformed_and_oversized_bodies_are_rejected_without_panics() {
    let server = Server::start(vec![
        Reply {
            status: 200,
            body: b"<html>not json</html>".to_vec(),
            headers: vec!["Content-Type: text/html".into()],
            delay: Duration::ZERO,
            body_delay: Duration::ZERO,
            declared_length: None,
        },
        Reply {
            status: 200,
            body: vec![b' '; 1024],
            headers: vec!["Content-Type: application/json".into()],
            delay: Duration::ZERO,
            body_delay: Duration::ZERO,
            declared_length: Some(65 * 1024 * 1024),
        },
    ]);
    let client = RemoteHttpClient::new(&server.endpoint(""), None).expect("client");
    assert!(matches!(
        client.get("/status").expect_err("malformed"),
        HttpError::Json(_)
    ));
    let error = client
        .get_with_read_timeout("/status", Duration::from_secs(2))
        .expect_err("truncated or oversized");
    assert!(
        matches!(error, HttpError::InvalidResponse(_) | HttpError::Io(_)),
        "{error}"
    );
}

#[test]
fn timeouts_surface_as_io_timeouts_the_navigation_classifier_recognizes() {
    let mut reply = Reply::json(200, &json!({"value": null}));
    reply.delay = Duration::from_millis(600);
    let server = Server::start(vec![reply]);
    let client = RemoteHttpClient::new(&server.endpoint(""), None).expect("client");
    let error = client
        .post_with_read_timeout(
            "/session/abc/url",
            &json!({"url": "https://x.test"}),
            Duration::from_millis(100),
        )
        .expect_err("timed out");
    let text = error.to_string();
    assert!(text.starts_with("WebDriver HTTP I/O:"), "{text}");
    assert!(text.contains("timed out"), "{text}");
}

#[test]
fn a_stalled_body_after_headers_is_still_a_timeout() {
    let mut reply = Reply::json(200, &json!({"value": null}));
    reply.body_delay = Duration::from_millis(600);
    let server = Server::start(vec![reply]);
    let client = RemoteHttpClient::new(&server.endpoint(""), None).expect("client");
    let error = client
        .post_with_read_timeout(
            "/session/abc/url",
            &json!({"url": "https://x.test"}),
            Duration::from_millis(100),
        )
        .expect_err("stalled body");
    let text = error.to_string();
    assert!(
        matches!(error, HttpError::Io(ref io) if io.kind() == std::io::ErrorKind::TimedOut),
        "{text}"
    );
    assert!(
        text.starts_with("WebDriver HTTP I/O:") && text.contains("timed out"),
        "{text}"
    );
}

#[test]
fn authorization_header_is_redacted_and_validated() {
    let value = header("Basic c2VjcmV0");
    assert_eq!(format!("{value:?}"), "RemoteAuthorizationHeader(<redacted>)");
    assert!(RemoteAuthorizationHeader::new(String::new()).is_err());
    assert!(RemoteAuthorizationHeader::new("Basic x\r\nX-Injected: 1".into()).is_err());
    let client = RemoteHttpClient::new("https://grid.example.net", Some(value)).expect("client");
    assert!(!format!("{client:?}").contains("c2VjcmV0"));
}
