use super::super::FILE;
use super::*;
use base64::Engine as _;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::mpsc,
};

fn token(claims: &serde_json::Value) -> String {
    let encode = |value: &serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap())
    };
    format!(
        "{}.{}.synthetic",
        encode(&serde_json::json!({"alg": "none"})),
        encode(claims)
    )
}

fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(seconds).unwrap()
}

const NOW: i64 = 1_800_000_000;
const DAY: i64 = 24 * 60 * 60;

/// A login whose access token was issued at `issued` and ends at `expires`.
fn login(issued: i64, expires: i64) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": "synthetic-exchanged-key",
        "tokens": {
            "id_token": token(&serde_json::json!({"email": "person@example.test"})),
            "access_token": token(&serde_json::json!({"iat": issued, "exp": expires})),
            "refresh_token": "synthetic-refresh-1",
            "account_id": "synthetic-account",
        },
        "last_refresh": "2026-01-01T00:00:00Z",
    }))
    .unwrap()
}

fn home_with(bytes: &[u8]) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    super::super::private_directory(home.path()).unwrap();
    write_private(home.path(), bytes).unwrap();
    home
}

/// Answers one request with `status` and `body`, and hands back the request body.
fn server(status: u16, body: &'static str) -> (String, mpsc::Receiver<serde_json::Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/oauth/token", listener.local_addr().unwrap());
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse().unwrap();
            }
        }
        let mut request = vec![0; length];
        reader.read_exact(&mut request).unwrap();
        let _ = sender.send(serde_json::from_slice(&request).unwrap());
        let mut stream = stream;
        write!(
            stream,
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    (url, receiver)
}

/// A URL nothing listens on: a request there fails the test through its error.
fn nowhere() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    drop(listener);
    url
}

#[test]
fn a_login_is_due_with_two_days_or_half_its_life_left() {
    let due_at = |issued, expires, now| due(&Login::parse(&login(issued, expires)).unwrap(), at(now));
    // A ten-day token: renewed with two days left.
    assert!(!due_at(NOW, NOW + 10 * DAY, NOW + 7 * DAY));
    assert!(due_at(NOW, NOW + 10 * DAY, NOW + 8 * DAY));
    // A one-hour token: renewed with half an hour left.
    assert!(!due_at(NOW, NOW + 3600, NOW + 1700));
    assert!(due_at(NOW, NOW + 3600, NOW + 1800));
    // An expired token is due too.
    assert!(due_at(NOW, NOW + 3600, NOW + 7200));
}

#[test]
fn a_token_without_times_is_due_after_a_week() {
    let mut value: serde_json::Value = serde_json::from_slice(&login(0, 1)).unwrap();
    value["tokens"]["access_token"] = "opaque".into();
    value["last_refresh"] = "2026-01-01T00:00:00Z".into();
    let parsed = Login::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    let refreshed = parsed.refreshed.unwrap();
    assert!(!due(&parsed, refreshed + time::Duration::days(6)));
    assert!(due(&parsed, refreshed + time::Duration::days(7)));
}

#[test]
fn a_login_with_time_left_is_not_sent_anywhere() {
    let home = home_with(&login(NOW, NOW + 10 * DAY));
    let before = std::fs::read(home.path().join(FILE)).unwrap();
    let renewal = renew_with(home.path(), &nowhere(), at(NOW + DAY), &Cancellation::default()).unwrap();
    assert_eq!(renewal, Renewal::NotDue);
    assert_eq!(std::fs::read(home.path().join(FILE)).unwrap(), before);
}

#[test]
fn no_login_is_no_renewal() {
    let home = tempfile::tempdir().unwrap();
    let cancel = Cancellation::default();
    assert_eq!(
        renew_with(home.path(), &nowhere(), at(NOW), &cancel).unwrap(),
        Renewal::NoLogin
    );
    assert_eq!(
        renew_with(&home.path().join("missing"), &nowhere(), at(NOW), &cancel).unwrap(),
        Renewal::NoLogin
    );
}

#[test]
fn a_due_login_is_renewed_as_codex_renews_it() {
    let home = home_with(&login(NOW, NOW + 10 * DAY));
    let fresh = token(&serde_json::json!({"iat": NOW + 9 * DAY, "exp": NOW + 19 * DAY}));
    let body: &'static str = Box::leak(
        serde_json::json!({"access_token": fresh, "refresh_token": "synthetic-refresh-2"})
            .to_string()
            .into_boxed_str(),
    );
    let (url, request) = server(200, body);
    let now = at(NOW + 9 * DAY);
    assert_eq!(
        renew_with(home.path(), &url, now, &Cancellation::default()).unwrap(),
        Renewal::Renewed
    );
    assert_eq!(
        request.recv().unwrap(),
        serde_json::json!({
            "client_id": CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": "synthetic-refresh-1",
        })
    );
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(home.path().join(FILE)).unwrap()).unwrap();
    assert_eq!(saved["tokens"]["access_token"], fresh.as_str());
    assert_eq!(saved["tokens"]["refresh_token"], "synthetic-refresh-2");
    // What the answer left out, and every other field, stays.
    let original: serde_json::Value = serde_json::from_slice(&login(NOW, NOW + 10 * DAY)).unwrap();
    assert_eq!(saved["tokens"]["id_token"], original["tokens"]["id_token"]);
    assert_eq!(saved["tokens"]["account_id"], "synthetic-account");
    assert_eq!(saved["OPENAI_API_KEY"], "synthetic-exchanged-key");
    assert_eq!(
        OffsetDateTime::parse(saved["last_refresh"].as_str().unwrap(), &Rfc3339).unwrap(),
        now
    );
    // The renewal is a later renewal of the same account, so workers take it over.
    let renewed = read(home.path()).unwrap().unwrap();
    assert!(renewed.renews(&Login::parse(&login(NOW, NOW + 10 * DAY)).unwrap()));
}

#[test]
fn a_refused_refresh_token_asks_for_a_new_sign_in_and_keeps_the_login() {
    for (status, body) in [
        (
            400,
            r#"{"error":{"code":"refresh_token_reused","message":"synthetic"}}"#,
        ),
        (400, r#"{"error":"invalid_grant"}"#),
        (401, r#"{"error":{"code":"refresh_token_invalidated"}}"#),
    ] {
        let home = home_with(&login(NOW, NOW + 3600));
        let (url, _request) = server(status, body);
        let renewal = renew_with(home.path(), &url, at(NOW + 3500), &Cancellation::default()).unwrap();
        assert_eq!(renewal, Renewal::SignInAgain, "{status} {body}");
        // Kept for a worker's later renewal, but shown as needing a sign-in.
        assert!(read(home.path()).unwrap().is_some());
        assert_eq!(super::super::account(home.path()).unwrap(), None);
        // It is not sent to OpenAI again.
        assert_eq!(
            renew_with(home.path(), &nowhere(), at(NOW + 3550), &Cancellation::default()).unwrap(),
            Renewal::SignInAgain
        );
    }
}

#[test]
fn a_later_renewal_of_a_refused_login_counts_again() {
    let home = home_with(&login(NOW, NOW + 3600));
    let (url, _request) = server(400, r#"{"error":"invalid_grant"}"#);
    renew_with(home.path(), &url, at(NOW + 3500), &Cancellation::default()).unwrap();
    // A worker's renewal of the same account, taken over by an exchange.
    let mut value: serde_json::Value = serde_json::from_slice(&login(NOW + 3000, NOW + 3000 + 10 * DAY)).unwrap();
    value["last_refresh"] = "2027-01-15T09:00:00Z".into();
    write_private(home.path(), &serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(super::super::account(home.path()).unwrap().is_some());
    assert_eq!(
        renew_with(home.path(), &nowhere(), at(NOW + 3500), &Cancellation::default()).unwrap(),
        Renewal::NotDue
    );
}

#[test]
fn a_cloud_with_the_current_login_is_asked_again_only_after_a_while() {
    let mut delivered = Delivered::new();
    delivered.insert("recent".into(), ("login-1".into(), Instant::now()));
    delivered.insert(
        "old".into(),
        (
            "login-1".into(),
            Instant::now().checked_sub(RESEND + Duration::from_secs(1)).unwrap(),
        ),
    );
    assert!(current_on(&delivered, "recent", "login-1"));
    assert!(!current_on(&delivered, "recent", "login-2"));
    assert!(!current_on(&delivered, "old", "login-1"));
    assert!(!current_on(&delivered, "new", "login-1"));
}

#[test]
fn a_temporary_failure_keeps_the_login() {
    for (status, body) in [
        (500, r#"{"error":"synthetic"}"#),
        (400, r#"{"error":"temporarily_unavailable"}"#),
        // A proxy or captive portal, not OpenAI's verdict on the refresh token.
        (401, r#"{"error":"synthetic"}"#),
        (200, r#"{"refresh_token":"synthetic-without-access"}"#),
    ] {
        let home = home_with(&login(NOW, NOW + 3600));
        let before = std::fs::read(home.path().join(FILE)).unwrap();
        let (url, _request) = server(status, body);
        assert!(renew_with(home.path(), &url, at(NOW + 3500), &Cancellation::default()).is_err());
        assert_eq!(std::fs::read(home.path().join(FILE)).unwrap(), before);
    }
    let home = home_with(&login(NOW, NOW + 3600));
    assert!(renew_with(home.path(), &nowhere(), at(NOW + 3500), &Cancellation::default()).is_err());
    assert!(read(home.path()).unwrap().is_some());
}

#[test]
fn the_fingerprint_follows_the_login() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(fingerprint(root.path()).unwrap(), None);
    let home = home(root.path());
    super::super::private_directory(&home).unwrap();
    write_private(&home, &login(NOW, NOW + 10 * DAY)).unwrap();
    let first = fingerprint(root.path()).unwrap().unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&login(NOW, NOW + 10 * DAY)).unwrap();
    value["last_refresh"] = "2026-02-01T00:00:00Z".into();
    write_private(&home, &serde_json::to_vec(&value).unwrap()).unwrap();
    assert_ne!(fingerprint(root.path()).unwrap().unwrap(), first);
}

#[test]
fn a_cloud_is_skipped_without_a_login() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        keep_cloud_current(root.path(), "synthetic-cloud", &Cancellation::default()).unwrap(),
        Kept::Skipped
    );
}

#[test]
fn only_folders_with_a_cloud_record_are_clouds() {
    let root = tempfile::tempdir().unwrap();
    for name in ["cloud-b", "cloud-a", "credentials", "not a cloud"] {
        std::fs::create_dir_all(root.path().join(name)).unwrap();
    }
    for name in ["cloud-a", "cloud-b", "not a cloud"] {
        std::fs::write(root.path().join(name).join("deployment.json"), b"{}").unwrap();
    }
    std::fs::write(root.path().join("settings.json"), b"{}").unwrap();
    assert_eq!(clouds(root.path()).unwrap(), ["cloud-a", "cloud-b"]);
    assert!(clouds(&root.path().join("missing")).unwrap().is_empty());
}

#[test]
fn a_sync_without_a_login_asks_no_cloud_and_forgets_removed_ones() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("cloud-a")).unwrap();
    std::fs::write(root.path().join("cloud-a/deployment.json"), b"{}").unwrap();
    let mut delivered = Delivered::new();
    delivered.insert("cloud-gone".into(), ("synthetic".into(), Instant::now()));
    delivered.insert("cloud-a".into(), ("synthetic".into(), Instant::now()));
    let renewal = sync(root.path(), &mut delivered, &Cancellation::default()).unwrap();
    assert_eq!(renewal, Renewal::NoLogin);
    assert_eq!(delivered.keys().collect::<Vec<_>>(), ["cloud-a"]);
}

/// Runs the real install script against a local stand-in for the worker's home.
#[cfg(unix)]
#[test]
fn a_refused_login_is_not_given_but_a_worker_renewal_still_comes_back() {
    use std::process::Command;
    let home = home_with(&login(NOW, NOW + 3600));
    let (url, _request) = server(400, r#"{"error":"invalid_grant"}"#);
    renew_with(home.path(), &url, at(NOW + 3500), &Cancellation::default()).unwrap();
    let worker = tempfile::tempdir().unwrap();
    let installed = worker.path().join(".codex/auth.json");
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let install = || {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(super::super::install_script(worker.path().to_str().unwrap()));
        command
    };
    // A worker without a login gets nothing.
    super::super::exchange(home.path(), &mut install(), &runner).unwrap();
    assert!(!installed.exists());
    // A worker's own later renewal is not replaced, and it makes the login here usable.
    let mut value: serde_json::Value = serde_json::from_slice(&login(NOW + 3000, NOW + 3000 + 10 * DAY)).unwrap();
    value["last_refresh"] = "2027-01-15T09:00:00Z".into();
    let renewal = serde_json::to_vec(&value).unwrap();
    std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
    std::fs::write(&installed, &renewal).unwrap();
    super::super::exchange(home.path(), &mut install(), &runner).unwrap();
    assert_eq!(std::fs::read(&installed).unwrap(), renewal);
    assert_eq!(std::fs::read(home.path().join(FILE)).unwrap(), renewal);
    assert!(super::super::account(home.path()).unwrap().is_some());
}
