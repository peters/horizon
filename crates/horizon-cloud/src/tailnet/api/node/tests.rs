use super::*;
use crate::tailnet::api::{remove, save, tests::Keys};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
};

fn fixture(oauth: bool) -> (tempfile::TempDir, Store, Keys) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().into());
    std::fs::write(
        root.path().join("tailnets.json"),
        br#"{"tailnets":[{"id":"work","name":"Work"}]}"#,
    )
    .unwrap();
    let keys = Keys::default();
    let value = if oauth {
        ApiCredential::oauth("synthetic-id", "synthetic-secret").unwrap()
    } else {
        ApiCredential::personal_token("tskey-api-synthetic").unwrap()
    };
    save(&store, "work", &value, &keys).unwrap();
    (root, store, keys)
}

fn response(status: u16, body: &[u8], extra: &str) -> Vec<u8> {
    let mut result = format!(
        "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    )
    .into_bytes();
    result.extend_from_slice(body);
    result
}

fn server(responses: Vec<(Duration, Vec<u8>)>) -> (HttpTransport, mpsc::Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let origin = format!("http://{}/api/v2", listener.local_addr().unwrap());
    let (send, receive) = mpsc::channel();
    let task = thread::spawn(move || {
        for (delay, response) in responses {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "fixture request did not arrive");
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
                }
            };
            // Windows inherits the listener's nonblocking mode on accepted sockets.
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() <= 16 * 1024);
            }
            let header = String::from_utf8(request.clone()).unwrap();
            let length = header
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(|n| n.parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            assert!(length <= 16 * 1024);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            request.extend(body);
            send.send(String::from_utf8(request).unwrap()).unwrap();
            thread::sleep(delay);
            let _ = stream.write_all(&response);
        }
    });
    (HttpTransport::for_test(origin), receive, task)
}

#[test]
fn personal_delete_uses_only_exact_encoded_id_and_keeps_enrollment_binding_untouched() {
    let (_root, store, keys) = fixture(false);
    let (http, requests, task) = server(vec![(Duration::ZERO, response(204, b"", ""))]);
    let before = keys.count();
    assert_eq!(
        delete(&store, "work", "n/a%?#", Duration::from_secs(2), &keys, &http),
        NodeDeletion::ConfirmedDeleted
    );
    let request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(request.starts_with("DELETE /api/v2/device/n%2Fa%25%3F%23 HTTP/1.1"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer tskey-api-synthetic")
    );
    assert!(!request.contains("/devices"));
    assert_eq!(keys.count(), before);
    task.join().unwrap();
}

#[test]
fn oauth_exchanges_scoped_tagged_token_then_deletes_exact_id() {
    let (_root, store, keys) = fixture(true);
    let token = br#"{"access_token":"synthetic-bearer","token_type":"Bearer","expires_in":600}"#;
    let (http, requests, task) = server(vec![
        (Duration::ZERO, response(200, token, "")),
        (Duration::ZERO, response(200, b"", "")),
    ]);
    assert_eq!(
        delete(&store, "work", "n-owned", Duration::from_secs(2), &keys, &http),
        NodeDeletion::ConfirmedDeleted
    );
    let request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(request.starts_with("POST /api/v2/oauth/token HTTP/1.1"));
    let body = request.split_once("\r\n\r\n").unwrap().1;
    assert!(body.split('&').any(|field| field == "scope=devices%3Acore"));
    assert!(body.split('&').any(|field| field == "tags=tag%3Ahorizon-worker"));
    let request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(request.starts_with("DELETE /api/v2/device/n-owned HTTP/1.1"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer synthetic-bearer")
    );
    assert!(!request.contains("synthetic-secret"));
    task.join().unwrap();
}

#[test]
fn denied_not_found_redirect_and_oversized_body_never_confirm_or_leak_raw_errors() {
    for (status, body, extra, expected) in [
        (401, b"private-api-error".to_vec(), "", RetentionReason::Authorization),
        (403, vec![], "", RetentionReason::Authorization),
        (404, vec![], "", RetentionReason::NotFoundUnverified),
        (408, vec![], "", RetentionReason::Transport),
        (429, vec![], "", RetentionReason::Transport),
        (503, vec![], "", RetentionReason::Transport),
        (
            302,
            vec![],
            "Location: http://127.0.0.1:1/escape\r\n",
            RetentionReason::UnexpectedResponse,
        ),
        (
            200,
            vec![b'x'; http::BODY_LIMIT + 1],
            "",
            RetentionReason::UnexpectedResponse,
        ),
    ] {
        let (_root, store, keys) = fixture(false);
        let (http, requests, task) = server(vec![(Duration::ZERO, response(status, &body, extra))]);
        let result = delete(&store, "work", "n-owned", Duration::from_secs(2), &keys, &http);
        assert_eq!(result, NodeDeletion::Retained(expected));
        assert!(!format!("{result:?}").contains("private-api-error"));
        assert!(
            requests
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .starts_with("DELETE ")
        );
        task.join().unwrap();
    }
}

#[test]
fn timeout_retains_identity_and_bounds_the_http_lifetime() {
    let (_root, store, keys) = fixture(false);
    let (http, _requests, task) = server(vec![(Duration::from_millis(200), response(204, b"", ""))]);
    let before = Instant::now();
    assert_eq!(
        delete(&store, "work", "n-owned", Duration::from_millis(50), &keys, &http),
        NodeDeletion::Retained(RetentionReason::Transport)
    );
    assert!(before.elapsed() < Duration::from_secs(1));
    task.join().unwrap();
}

struct NoTransport;
impl Transport for NoTransport {
    fn send(&self, _: Request<'_>, _: Instant) -> Result<http::Response, RetentionReason> {
        panic!("request must not run")
    }
}

#[test]
fn removed_binding_missing_tailnet_and_invalid_id_refuse_before_http() {
    let (_root, store, keys) = fixture(false);
    remove(&store, "work", &keys).unwrap();
    for id in ["work", "unavailable"] {
        assert_eq!(
            delete(&store, id, "n-owned", Duration::from_secs(2), &keys, &NoTransport),
            NodeDeletion::Retained(RetentionReason::MissingCredential)
        );
    }
    assert_eq!(
        delete(&store, "work", "..", Duration::from_secs(2), &keys, &NoTransport),
        NodeDeletion::Retained(RetentionReason::UnexpectedResponse)
    );
}

#[test]
fn status_and_delete_share_managed_slot_rules_without_legacy_resurrection() {
    use crate::tailnet::api::{ApiCredentialStatus, records, status};
    let (_root, store, keys) = fixture(false);
    let stored = records(&store).unwrap();
    let super::super::Binding::Active { slot, .. } = &stored.bindings["work"] else {
        panic!()
    };
    keys.delete(slot).unwrap();
    keys.put(
        "work",
        br#"{"kind":"personal_token","token":"tskey-api-staged-synthetic"}"#,
    )
    .unwrap();
    assert_eq!(status(&store, "work", &keys).unwrap(), ApiCredentialStatus::Missing);
    assert_eq!(
        delete(&store, "work", "n-owned", Duration::from_secs(2), &keys, &NoTransport),
        NodeDeletion::Retained(RetentionReason::MissingCredential)
    );
    remove(&store, "work", &keys).unwrap();
    assert_eq!(status(&store, "work", &keys).unwrap(), ApiCredentialStatus::Missing);
    assert_eq!(
        delete(&store, "work", "n-owned", Duration::from_secs(2), &keys, &NoTransport),
        NodeDeletion::Retained(RetentionReason::MissingCredential)
    );
}

struct OAuthOnly(HttpTransport);
impl Transport for OAuthOnly {
    fn send(&self, request: Request<'_>, deadline: Instant) -> Result<http::Response, RetentionReason> {
        assert!(
            matches!(&request, Request::OAuth { .. }),
            "failed OAuth must not dispatch DELETE"
        );
        self.0.send(request, deadline)
    }
}

fn retained_oauth(reply: Vec<u8>, expected: RetentionReason) {
    let (_root, store, keys) = fixture(true);
    let (http, requests, task) = server(vec![(Duration::ZERO, reply)]);
    let before = keys.count();
    assert_eq!(
        delete(
            &store,
            "work",
            "n-owned",
            Duration::from_secs(2),
            &keys,
            &OAuthOnly(http)
        ),
        NodeDeletion::Retained(expected)
    );
    assert!(
        requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .starts_with("POST /api/v2/oauth/token HTTP/1.1")
    );
    task.join().unwrap();
    assert!(requests.try_recv().is_err());
    assert_eq!(keys.count(), before);
}

#[test]
fn oauth_denial_capacity_and_unexpected_statuses_remain_distinct_without_delete() {
    for (status, reason) in [
        (401, RetentionReason::Authorization),
        (403, RetentionReason::Authorization),
        (408, RetentionReason::Transport),
        (429, RetentionReason::Transport),
        (503, RetentionReason::Transport),
        (404, RetentionReason::UnexpectedResponse),
        (500, RetentionReason::UnexpectedResponse),
    ] {
        retained_oauth(response(status, b"private-api-error", ""), reason);
    }
    retained_oauth(
        response(302, b"", "Location: http://127.0.0.1:1/escape\r\n"),
        RetentionReason::UnexpectedResponse,
    );
}

#[test]
fn malformed_zero_expiry_and_non_bearer_oauth_replies_cannot_dispatch_delete() {
    for body in [
        b"not-json".to_vec(),
        br#"{"token_type":"Bearer","expires_in":600}"#.to_vec(),
        br#"{"access_token":"synthetic-bearer","token_type":"Bearer","expires_in":0}"#.to_vec(),
        br#"{"access_token":"synthetic-bearer","token_type":"Basic","expires_in":600}"#.to_vec(),
        br#"{"access_token":"","token_type":"Bearer","expires_in":600}"#.to_vec(),
        serde_json::to_vec(&serde_json::json!({
            "access_token":"x".repeat(4097), "token_type":"Bearer", "expires_in":600
        }))
        .unwrap(),
    ] {
        retained_oauth(response(200, &body, ""), RetentionReason::UnexpectedResponse);
    }
}

#[test]
fn oversized_oauth_response_headers_refuse_before_delete() {
    let token = br#"{"access_token":"synthetic-bearer","token_type":"Bearer","expires_in":600}"#;
    retained_oauth(
        response(200, token, &format!("X-Large: {}\r\n", "x".repeat(http::BODY_LIMIT))),
        RetentionReason::Transport,
    );
}

struct LateOAuth;
impl Transport for LateOAuth {
    fn send(&self, request: Request<'_>, deadline: Instant) -> Result<http::Response, RetentionReason> {
        assert!(
            matches!(&request, Request::OAuth { .. }),
            "expired shared budget must not dispatch DELETE"
        );
        thread::sleep(deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(5));
        let token = br#"{"access_token":"synthetic-bearer","token_type":"Bearer","expires_in":600}"#;
        http::bounded(200, &token[..])
    }
}

#[test]
fn valid_oauth_reply_after_shared_budget_expiry_cannot_dispatch_delete() {
    let (_root, store, keys) = fixture(true);
    assert_eq!(
        delete(&store, "work", "n-owned", Duration::from_millis(20), &keys, &LateOAuth),
        NodeDeletion::Retained(RetentionReason::Transport)
    );
}
