use std::time::Duration;

use serde_json::json;

use super::{RemoteAuthorizationHeader, RemoteHttpClient};
use crate::webdriver::http::HttpError;
use crate::webdriver::test_server::{Reply, Server};
use crate::webdriver::transport::ClassicTransport;

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
    assert_eq!(seen[0].method, "POST");
    assert_eq!(
        seen[0].path, "/wd/hub/session",
        "base path is prepended and the trailing slash dropped"
    );
    assert_eq!(seen[0].authorization.as_deref(), Some("Basic c2VjcmV0"));
    assert!(seen[0].body.contains("alwaysMatch"));
    assert_eq!(client.origin(), format!("http://127.0.0.1:{}", server.port));
}

#[test]
fn requests_without_authorization_carry_no_header() {
    let server = Server::start(vec![Reply::json(200, &json!({"value": null}))]);
    let client = RemoteHttpClient::new(&server.endpoint(""), None).expect("client");
    client.delete("/session/abc").expect("deleted");
    let seen = server.recorded();
    assert_eq!(seen[0].method, "DELETE");
    assert_eq!(seen[0].path, "/session/abc");
    assert_eq!(seen[0].authorization, None);
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
    assert!(
        local.config().proxy().is_none(),
        "loopback grids never use an environment proxy"
    );
    assert!(named.config().proxy().is_none());
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
    let server = Server::start(vec![
        Reply::json(200, &json!({"value": null})).body_delayed(Duration::from_millis(600)),
    ]);
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
