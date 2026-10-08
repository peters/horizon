use super::*;
use crate::cloud_runtime::Cancellation;
use horizon_cloud::github::Chain;
use std::{
    io::{Read as _, Write as _},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

fn settings(mode: Mode, secret: &std::path::Path) -> Settings {
    Settings {
        app_id: 42,
        slug: "horizon-example".into(),
        client_id: "Iv23synthetic".into(),
        client_secret_file: secret.into(),
        mode,
    }
}

fn private(dir: &std::path::Path, value: &str) -> PathBuf {
    let path = dir.join("github-app-secret");
    std::fs::write(&path, value).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    path
}

fn chain() -> Chain {
    Chain {
        access_token: Secret::new("ghu_synthetic_access".into()),
        access_expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000),
        refresh_token: Secret::new("ghr_synthetic_refresh".into()),
        refresh_expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(2_000),
    }
}

fn user() -> horizon_cloud::github::User {
    horizon_cloud::github::User {
        login: "octo-cat".into(),
        name: "Octo Cat".into(),
        email: "7+octo-cat@users.noreply.github.com".into(),
    }
}

#[test]
fn settings_need_an_absolute_secret_path_and_a_clean_identity() {
    let dir = tempfile::tempdir().unwrap();
    assert!(settings(Mode::Ask, &dir.path().join("secret")).validate().is_ok());
    assert!(settings(Mode::Ask, std::path::Path::new("secret")).validate().is_err());
    let mut bad = settings(Mode::Ask, &dir.path().join("secret"));
    bad.slug = "../evil".into();
    assert!(bad.validate().is_err());
    let parsed: Settings = serde_json::from_value(serde_json::json!({
        "app_id": 42, "slug": "horizon-example", "client_id": "Iv23synthetic",
        "client_secret_file": "/synthetic/secret"
    }))
    .unwrap();
    assert_eq!(parsed.mode, Mode::Ask, "Ask is the default mode");
    assert_eq!(
        parsed.installation_url(),
        "https://github.com/apps/horizon-example/installations/new"
    );
}

#[test]
fn worker_status_is_read_from_its_last_json_line() {
    use worker::{Status, parse_status};
    assert_eq!(parse_status("{\"state\":\"unsupported\"}"), Status::Unsupported);
    assert_eq!(
        parse_status("{\"state\":\"revoked\",\"repositories\":[\"a/b\"]}"),
        Status::Absent
    );
    assert_eq!(parse_status("{\"state\":\"ok\",\"repositories\":[]}"), Status::Absent);
    assert_eq!(parse_status("garbage"), Status::Absent);
    assert_eq!(
        parse_status("note\n{\"state\":\"ok\",\"login\":\"octo-cat\",\"repositories\":[\"Acme/Web\",\"../x\"]}"),
        Status::Current {
            login: "octo-cat".into(),
            repositories: vec!["acme/web".into()]
        }
    );
}

#[test]
fn the_payload_is_private_and_carries_the_secret_only_in_automatic_mode() {
    let dir = tempfile::tempdir().unwrap();
    let secret_file = private(dir.path(), "synthetic-client-secret\n");
    let grants = [
        Grant {
            repository: "acme/web".into(),
            target: Target::Primary,
        },
        Grant {
            repository: "acme/api".into(),
            target: Target::try_from("sibling:api".to_owned()).unwrap(),
        },
    ];
    for mode in [Mode::Ask, Mode::Automatic] {
        let settings = settings(mode, &secret_file);
        let secret = (mode == Mode::Automatic).then(|| settings.client_secret().unwrap());
        let file = worker::payload(&settings, secret.as_ref(), &user(), &grants, &chain()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(file.path()).unwrap().permissions().mode() & 0o077, 0);
        }
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(file.path()).unwrap()).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["author_email"], "7+octo-cat@users.noreply.github.com");
        assert_eq!(value["grants"][1]["target"], "sibling:api");
        assert_eq!(value["grants"][0]["access"], "push");
        assert_eq!(value["chain"]["access_expires_at"], 1_000);
        assert_eq!(value["chain"]["refresh_token"], "ghr_synthetic_refresh");
        match mode {
            Mode::Ask => assert!(value.get("client_secret").is_none(), "Ask mode keeps the secret here"),
            Mode::Automatic => assert_eq!(value["client_secret"], "synthetic-client-secret"),
        }
    }
}

#[test]
fn a_secret_file_others_can_read_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = private(dir.path(), "synthetic");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(settings(Mode::Automatic, &path).client_secret().is_err());
    }
}

fn runner_parts() -> (Cancellation, Arc<Mutex<Vec<Event>>>) {
    (Cancellation::default(), Arc::default())
}

#[test]
fn grants_take_the_primary_from_its_github_origin_and_siblings_by_name() {
    let checkout = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(checkout.path())
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["init", "--quiet"]);
    git(&["remote", "add", "origin", "git@github.com:Acme/Web.git"]);
    let config = horizon_cloud::CloudConfig::parse(
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker:latest\n    cpu: 4\n    memory_gb: 8\n",
    )
    .unwrap();
    let mut state: Deployment = serde_json::from_value(serde_json::json!({
        "version": 1, "cloud_id": "fixture", "repository": checkout.path(), "revision": "a".repeat(40),
        "profile": config.profiles["dev"], "stage": "Provision", "operation": {"state": "prepared"},
        "spec": null, "worker": null, "sessions": []
    }))
    .unwrap();
    let (cancel, events) = runner_parts();
    let emit = |event| events.lock().unwrap().push(event);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    assert_eq!(
        grants(&state, &runner).unwrap(),
        [Grant {
            repository: "acme/web".into(),
            target: Target::Primary
        }]
    );
    state.repository = tempfile::tempdir().unwrap().path().into();
    assert!(
        grants(&state, &runner).unwrap().is_empty(),
        "a checkout without a GitHub origin has no grant"
    );
}

#[test]
fn the_pkce_challenge_is_the_base64url_sha256_of_the_verifier() {
    let verifier = Secret::new("abc".into());
    assert_eq!(
        signin::challenge(&verifier),
        "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0"
    );
    let url = signin::authorize_url("Iv23synthetic", "http://127.0.0.1:4711/callback", "s1", "c1");
    assert_eq!(
        url,
        "https://github.com/login/oauth/authorize?client_id=Iv23synthetic&redirect_uri=http%3A%2F%2F127.0.0.1%3A4711%2Fcallback&state=s1&code_challenge=c1&code_challenge_method=S256"
    );
}

fn request(line: &str) -> (Option<Secret>, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let line = line.to_owned();
    let client = std::thread::spawn(move || {
        let mut stream = TcpStream::connect(address).unwrap();
        write!(stream, "{line}\r\nHost: x\r\n\r\n").unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer
    });
    let (stream, _) = listener.accept().unwrap();
    let code = signin::callback(stream, "expected-state");
    (code, client.join().unwrap())
}

#[test]
fn only_a_callback_with_the_expected_state_yields_its_code() {
    let (code, answer) = request("GET /callback?code=abc123&state=expected-state HTTP/1.1");
    assert_eq!(code.unwrap().expose(), "abc123");
    assert!(answer.contains("You can close this page."));
    for line in [
        "GET /callback?code=abc123&state=other HTTP/1.1",
        "GET /callback?code=abc%3B1&state=expected-state HTTP/1.1",
        "GET /favicon.ico HTTP/1.1",
        "POST /callback?code=abc123&state=expected-state HTTP/1.1",
    ] {
        let (code, _) = request(line);
        assert!(code.is_none(), "{line}");
    }
}

/// A fake GitHub that answers each connection with the next scripted JSON body.
fn github(responses: Vec<serde_json::Value>) -> (horizon_cloud::github::Client, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = horizon_cloud::github::Client::loopback(listener.local_addr().unwrap()).unwrap();
    let task = std::thread::spawn(move || {
        for body in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0; 8192];
            let _ = stream.read(&mut buffer).unwrap();
            let text = body.to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            )
            .unwrap();
        }
    });
    (client, task)
}

#[test]
fn a_device_sign_in_shows_its_code_and_a_skip_ends_it() {
    let dir = tempfile::tempdir().unwrap();
    let settings = settings(Mode::Ask, &dir.path().join("unused"));
    let (client, task) = github(vec![serde_json::json!({
        "device_code": "dc", "user_code": "WDJB-MJHT",
        "verification_uri": "https://github.com/login/device", "expires_in": 899, "interval": 1
    })]);
    let (cancel, events) = runner_parts();
    let seen = events.clone();
    let emit = move |event| seen.lock().unwrap().push(event);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    let skipper = std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(300));
        skip("cloud-skip");
    });
    let result = signin::chain(&settings, "cloud-skip", &client, &runner);
    skipper.join().unwrap();
    task.join().unwrap();
    assert!(matches!(result, Err(signin::Ended::Reason(reason)) if reason.starts_with("Skipped")));
    let events = events.lock().unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        Event::GitHub(Prompt::Device { user_code, .. }) if user_code == "WDJB-MJHT"
    )));
}

#[test]
fn an_approved_device_sign_in_returns_its_chain() {
    let dir = tempfile::tempdir().unwrap();
    let settings = settings(Mode::Ask, &dir.path().join("unused"));
    let (client, task) = github(vec![
        serde_json::json!({
            "device_code": "dc", "user_code": "WDJB-MJHT",
            "verification_uri": "https://github.com/login/device", "expires_in": 899, "interval": 1
        }),
        serde_json::json!({"error": "authorization_pending"}),
        serde_json::json!({
            "access_token": "ghu_synthetic", "token_type": "bearer", "expires_in": 28800,
            "refresh_token": "ghr_synthetic", "refresh_token_expires_in": 15_724_800
        }),
    ]);
    let (cancel, _events) = runner_parts();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let chain = signin::chain(&settings, "cloud-approve", &client, &runner)
        .unwrap_or_else(|_| panic!("the approved sign-in returns a chain"));
    task.join().unwrap();
    assert_eq!(chain.refresh_token.expose(), "ghr_synthetic");
}
