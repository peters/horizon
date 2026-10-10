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

mod asks;
mod manifest;

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
        parse_status(
            "{\"version\":1,\"state\":\"ok\",\"persistent\":true,\"login\":\"octo-cat\",\"repositories\":\
             [{\"repository\":\"Acme/Web\",\"target\":\"primary\",\"access\":\"push\"}]}"
        ),
        Status::Current {
            login: "octo-cat".into(),
            repositories: vec!["acme/web".into()],
            checkouts: vec!["acme/web".into()],
            requests: false
        },
        "the worker reports each grant as an object"
    );
    assert_eq!(
        parse_status("note\n{\"state\":\"ok\",\"login\":\"octo-cat\",\"repositories\":[\"Acme/Web\",\"../x\"]}"),
        Status::Current {
            login: "octo-cat".into(),
            repositories: vec!["acme/web".into()],
            checkouts: vec!["acme/web".into()],
            requests: false
        }
    );
}

#[test]
fn a_service_that_is_not_running_is_not_current_and_requests_need_support() {
    use worker::{Status, parse_status};
    assert_eq!(
        parse_status("{\"state\":\"absent\",\"serving\":false,\"repositories\":[]}"),
        Status::Unavailable,
        "no sign-in for a chain that no running service could serve"
    );
    assert_eq!(
        parse_status("{\"state\":\"ok\",\"serving\":false,\"repositories\":[{\"repository\":\"acme/web\"}]}"),
        Status::Unavailable
    );
    assert_eq!(
        parse_status(
            "{\"state\":\"ok\",\"serving\":true,\"login\":\"octo-cat\",\"pending_requests\":0,\"repositories\":[{\"repository\":\"acme/web\",\"target\":\"primary\"}]}"
        ),
        Status::Current {
            login: "octo-cat".into(),
            repositories: vec!["acme/web".into()],
            checkouts: vec!["acme/web".into()],
            requests: true
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

#[cfg(unix)]
#[test]
fn a_secret_file_others_can_read_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = private(dir.path(), "synthetic");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(settings(Mode::Automatic, &path).client_secret().is_err());
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
    state.siblings = Some(
        serde_json::from_value(serde_json::json!({
            "primary_directory": "web",
            "members": [{"alias": "again", "repository": "Acme/Web", "directory": "web-again",
                         "revision": "b".repeat(40), "local_repository": "/synthetic", "profile": "dev"}]
        }))
        .unwrap(),
    );
    assert!(
        grants(&state, &runner).is_err(),
        "one repository on two worker checkouts is refused, not merged"
    );
    state.siblings = None;
    git(&["remote", "set-url", "origin", "https://gitlab.example/acme/web.git"]);
    assert!(
        grants(&state, &runner).unwrap().is_empty(),
        "a checkout with an origin elsewhere has no grant"
    );
    git(&["remote", "remove", "origin"]);
    assert!(
        grants(&state, &runner).unwrap().is_empty(),
        "a checkout without an origin has no grant"
    );
    git(&["remote", "add", "origin", "git@github.com:Acme/Web.git"]);
    cancel.cancel();
    assert!(
        grants(&state, &runner).is_err(),
        "a cancelled lookup is an error, never an empty grant set"
    );
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    state.repository = tempfile::tempdir().unwrap().path().into();
    assert!(
        grants(&state, &runner).is_err(),
        "a checkout whose remotes cannot be read is an error"
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

fn request(line: &str) -> (Option<signin::Callback>, String) {
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
    assert!(matches!(code, Some(signin::Callback::Code(code)) if code.expose() == "abc123"));
    assert!(answer.contains("You can close this page."));
    let (denied, answer) =
        request("GET /callback?error=access_denied&error_description=x&state=expected-state HTTP/1.1");
    assert!(
        matches!(denied, Some(signin::Callback::Denied)),
        "a declined sign-in ends it"
    );
    assert!(answer.contains("declined"));
    for line in [
        "GET /callback?code=abc123&state=other HTTP/1.1",
        "GET /callback?error=access_denied&state=other HTTP/1.1",
        "GET /callback?code=abc%3B1&state=expected-state HTTP/1.1",
        "GET /favicon.ico HTTP/1.1",
        "POST /callback?code=abc123&state=expected-state HTTP/1.1",
    ] {
        let (code, _) = request(line);
        assert!(code.is_none(), "{line}");
    }
}

/// Reads one whole request, its head and its body, so closing the connection never
/// resets it (Windows sends the body in a separate segment).
/// Reads one request and returns it, head and body.
pub(in crate::cloud_runtime::github) fn read_request(stream: &mut TcpStream) -> String {
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut input = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let read = stream.read(&mut buffer).unwrap();
        if read == 0 {
            return String::from_utf8_lossy(&input).into_owned();
        }
        input.extend_from_slice(&buffer[..read]);
        if let Some(end) = input.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&input[..end]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if input.len() >= end + 4 + length {
                return String::from_utf8_lossy(&input).into_owned();
            }
        }
    }
}

/// A fake GitHub that answers each connection with the next scripted JSON body.
pub(in crate::cloud_runtime::github) fn github(
    responses: Vec<serde_json::Value>,
) -> (horizon_cloud::github::Client, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = horizon_cloud::github::Client::loopback(listener.local_addr().unwrap()).unwrap();
    let task = std::thread::spawn(move || {
        for body in responses {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
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
    assert!(matches!(result, Err(signin::Ended::Skipped)));
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

#[test]
fn more_grants_than_a_worker_takes_are_refused_before_any_sign_in() {
    let checkout = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(checkout.path())
            .args(["init", "--quiet"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(checkout.path())
            .args(["remote", "add", "origin", "https://github.com/acme/web.git"])
            .status()
            .unwrap()
            .success()
    );
    let config = horizon_cloud::CloudConfig::parse(
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker:latest\n    cpu: 4\n    memory_gb: 8\n",
    )
    .unwrap();
    let members: Vec<serde_json::Value> = (0..16)
        .map(|n| {
            serde_json::json!({"alias": format!("s{n}"), "repository": format!("acme/lib-{n}"),
                               "directory": format!("lib-{n}"), "revision": "b".repeat(40),
                               "local_repository": "/synthetic", "profile": "dev"})
        })
        .collect();
    let state: Deployment = serde_json::from_value(serde_json::json!({
        "version": 1, "cloud_id": "fixture", "repository": checkout.path(), "revision": "a".repeat(40),
        "profile": config.profiles["dev"], "stage": "Provision", "operation": {"state": "prepared"},
        "spec": null, "worker": null, "sessions": [],
        "siblings": {"primary_directory": "web", "members": members}
    }))
    .unwrap();
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    assert!(
        grants(&state, &runner).is_err(),
        "the primary and 16 siblings are 17 grants"
    );
}

#[test]
fn access_for_a_removed_checkout_is_narrowed_and_cloud_grants_are_kept() {
    use worker::{Status, parse_status};
    let Status::Current {
        checkouts,
        repositories,
        ..
    } = parse_status(
        "{\"state\":\"ok\",\"serving\":true,\"repositories\":[\
         {\"repository\":\"acme/web\",\"target\":\"primary\"},\
         {\"repository\":\"acme/old-lib\",\"target\":\"sibling:lib\"},\
         {\"repository\":\"acme/extra\",\"target\":null}]}",
    )
    else {
        panic!("a current chain");
    };
    assert_eq!(repositories.len(), 3);
    let grants = [Grant {
        repository: "acme/web".into(),
        target: Target::Primary,
    }];
    let said = std::cell::RefCell::new(Vec::new());
    let say = |text: &str| said.borrow_mut().push(text.to_owned());
    assert!(narrower(&checkouts, &grants, &say), "the removed sibling is narrowed");
    assert_eq!(said.borrow().len(), 1, "a grant for the whole cloud is not stale");
    assert!(!narrower(&["acme/web".to_owned()], &grants, &say));
}

#[test]
fn a_worker_that_took_a_new_chain_never_reports_its_old_access() {
    let held = || Some(("octo-cat".to_owned(), vec!["acme/web".to_owned()], true));
    assert_eq!(
        settle(Signed::Refused("Skipped.".into()), held()),
        Settled::Kept("Skipped.".into(), held().unwrap())
    );
    assert_eq!(
        settle(Signed::Unserved("The service is down.".into()), held()),
        Settled::Ended {
            reason: "The service is down.".into(),
            renewable: false
        },
        "after the install the old chain is gone, and a sign-in does not start the service"
    );
    assert_eq!(
        settle(Signed::Refused("Skipped.".into()), None),
        Settled::Ended {
            reason: "Skipped.".into(),
            renewable: true
        }
    );
    assert_eq!(
        settle(Signed::In { complete: false }, held()),
        Settled::Signed { complete: false }
    );
}

#[test]
fn only_access_to_every_checkout_replaces_the_settings_binding() {
    let grants = [
        Grant {
            repository: "acme/web".into(),
            target: Target::Primary,
        },
        Grant {
            repository: "acme/lib".into(),
            target: Target::try_from("sibling:lib".to_owned()).unwrap(),
        },
    ];
    assert!(
        !complete(&["acme/web".to_owned()], &grants),
        "acme/lib keeps its binding"
    );
    assert!(complete(
        &["Acme/Web".to_owned(), "acme/lib".to_owned(), "acme/extra".to_owned()],
        &grants
    ));
    assert!(
        !complete(&["acme/web".to_owned()], &[]),
        "no grants is not complete access"
    );
}

#[test]
fn a_repository_of_another_account_needs_a_public_app() {
    let app = settings(Mode::Ask, std::path::Path::new("/secret"));
    let own = not_installed(&app, "Octo-Cat", "octo-cat/web");
    assert_eq!(
        own,
        "the app is not installed on octo-cat/web. Add it at https://github.com/apps/horizon-example/installations/new"
    );
    let org = not_installed(&app, "Octo-Cat", "acme/web");
    assert!(
        org.starts_with("the app is not installed on acme/web. The app belongs to Octo-Cat"),
        "{org}"
    );
    assert!(
        org.contains("select Make public at https://github.com/settings/apps/horizon-example/advanced"),
        "{org}"
    );
    assert!(
        org.ends_with("add it to acme at https://github.com/apps/horizon-example/installations/new"),
        "{org}"
    );
}
