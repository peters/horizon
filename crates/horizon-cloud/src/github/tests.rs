use super::*;
use serde_json::{Value, json};
use std::{
    io::Write,
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
};

/// Each request, as its request line and its body.
type Requests = Arc<Mutex<Vec<(String, String)>>>;

/// A fake GitHub answering each connection with the next scripted response.
fn github(responses: Vec<(u16, Value)>) -> (Client, Requests, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = Client::loopback(listener.local_addr().unwrap()).unwrap();
    let requests = Requests::default();
    let observed = requests.clone();
    let task = thread::spawn(move || {
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut input = Vec::new();
            let mut buffer = [0; 4096];
            let (head, body_start, length) = loop {
                let read = stream.read(&mut buffer).unwrap();
                input.extend_from_slice(&buffer[..read]);
                if let Some(end) = input.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&input[..end]).into_owned();
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    break (head, end + 4, length);
                }
            };
            while input.len() < body_start + length {
                let read = stream.read(&mut buffer).unwrap();
                input.extend_from_slice(&buffer[..read]);
            }
            let line = head.lines().next().unwrap_or_default().to_owned();
            let sent = String::from_utf8_lossy(&input[body_start..body_start + length]).into_owned();
            observed.lock().unwrap().push((line, sent));
            let text = body.to_string();
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            )
            .unwrap();
        }
    });
    (client, requests, task)
}

fn tokens() -> Value {
    json!({
        "access_token": "ghu_synthetic_access",
        "expires_in": 28800,
        "refresh_token": "ghr_synthetic_refresh",
        "refresh_token_expires_in": 15_724_800,
        "token_type": "bearer",
        "scope": ""
    })
}

fn fields(body: &str) -> Vec<(String, String)> {
    body.split('&')
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap();
            (key.to_owned(), value.replace("%3A", ":"))
        })
        .collect()
}

fn device(client: &Client) -> DeviceCode {
    client.start_device("Iv23synthetic").unwrap()
}

#[test]
fn a_device_sign_in_waits_slows_down_and_returns_an_expiring_chain() {
    let (client, requests, task) = github(vec![
        (
            200,
            json!({"device_code": "dc-synthetic", "user_code": "ABCD-1234",
                   "verification_uri": "https://github.com/login/device", "expires_in": 899, "interval": 5}),
        ),
        (200, json!({"error": "authorization_pending"})),
        (200, json!({"error": "slow_down", "interval": 10})),
        (200, json!({"error": "slow_down"})),
        (200, tokens()),
    ]);
    let mut code = device(&client);
    assert_eq!(code.user_code, "ABCD-1234");
    assert_eq!(code.interval, Duration::from_secs(5));
    assert_eq!(
        format!("{code:?}").matches("dc-synthetic").count(),
        0,
        "the device code is never printed"
    );
    assert!(matches!(
        client.poll_device("Iv23synthetic", &mut code),
        Ok(Poll::Pending)
    ));
    assert!(matches!(
        client.poll_device("Iv23synthetic", &mut code),
        Ok(Poll::SlowDown(interval)) if interval == Duration::from_secs(10)
    ));
    assert!(matches!(
        client.poll_device("Iv23synthetic", &mut code),
        Ok(Poll::SlowDown(interval)) if interval == Duration::from_secs(15)
    ));
    let Ok(Poll::Granted(chain)) = client.poll_device("Iv23synthetic", &mut code) else {
        panic!("the approved sign-in returns a chain");
    };
    task.join().unwrap();
    assert_eq!(chain.access_token.expose(), "ghu_synthetic_access");
    assert_eq!(chain.refresh_token.expose(), "ghr_synthetic_refresh");
    let in_8h = SystemTime::now() + Duration::from_secs(28800);
    assert!(chain.access_expires_at <= in_8h && chain.access_expires_at + Duration::from_secs(60) > in_8h);
    assert!(!format!("{chain:?}").contains("ghu_") && !format!("{chain:?}").contains("ghr_"));
    let requests = requests.lock().unwrap();
    assert_eq!(requests[0].0, "POST /login/device/code HTTP/1.1");
    assert_eq!(fields(&requests[0].1), [("client_id".into(), "Iv23synthetic".into())]);
    let poll = fields(&requests[1].1);
    assert!(poll.contains(&("grant_type".into(), DEVICE_GRANT.into())));
    assert!(poll.contains(&("device_code".into(), "dc-synthetic".into())));
    assert!(!poll.iter().any(|(key, _)| key == "client_secret"));
}

#[test]
fn a_device_sign_in_reports_why_it_ended() {
    for (error, expected) in [
        ("access_denied", Error::Denied),
        ("expired_token", Error::Expired),
        ("device_flow_disabled", Error::DeviceFlowDisabled),
        ("incorrect_device_code", Error::Revoked),
        (
            "unsupported_grant_type",
            Error::Refused("unsupported_grant_type".into()),
        ),
        ("Some <free> text", Error::InvalidResponse),
    ] {
        let (client, _requests, task) = github(vec![
            (
                200,
                json!({"device_code": "dc", "user_code": "ABCD-1234",
                       "verification_uri": "https://github.com/login/device", "expires_in": 899}),
            ),
            (200, json!({"error": error})),
        ]);
        let mut code = device(&client);
        assert_eq!(
            code.interval,
            Duration::from_secs(DEFAULT_INTERVAL),
            "GitHub's default interval"
        );
        assert_eq!(
            client.poll_device("Iv23synthetic", &mut code).unwrap_err(),
            expected,
            "{error}"
        );
        task.join().unwrap();
    }
}

#[test]
fn a_device_code_answer_must_be_safe_to_show() {
    for (user_code, uri) in [
        ("ABCD 1234\u{1b}[2J", "https://github.com/login/device"),
        ("ABCD-1234", "http://github.com/login/device"),
        ("ABCD-1234", "https://"),
        ("ABCD-1234", "https://github.com/\u{1b}[2J"),
        ("ABCD-1234", "https://github.com:8443/login/device"),
        ("abcd-1234", "https://github.com/login/device"),
    ] {
        let (client, _requests, task) = github(vec![(
            200,
            json!({"device_code": "dc", "user_code": user_code, "verification_uri": uri, "expires_in": 899}),
        )]);
        assert_eq!(
            client.start_device("Iv23synthetic").unwrap_err(),
            Error::InvalidResponse
        );
        task.join().unwrap();
    }
}

#[test]
fn a_refresh_sends_the_secret_only_when_given_and_returns_the_new_chain() {
    let (client, requests, task) = github(vec![(200, tokens()), (200, tokens())]);
    let refresh = Secret::new("ghr_old".into());
    client.refresh("Iv23synthetic", None, &refresh).unwrap();
    let secret = Secret::new("synthetic-client-secret".into());
    client.refresh("Iv23synthetic", Some(&secret), &refresh).unwrap();
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    let without = fields(&requests[0].1);
    assert!(without.contains(&("grant_type".into(), "refresh_token".into())));
    assert!(without.contains(&("refresh_token".into(), "ghr_old".into())));
    assert!(!without.iter().any(|(key, _)| key == "client_secret"));
    assert!(fields(&requests[1].1).contains(&("client_secret".into(), "synthetic-client-secret".into())));
}

#[test]
fn a_used_or_expired_refresh_token_means_connecting_again() {
    for error in ["bad_refresh_token", "incorrect_client_credentials"] {
        let (client, _requests, task) = github(vec![(200, json!({"error": error, "error_description": "x"}))]);
        let result = client.refresh("Iv23synthetic", None, &Secret::new("ghr_old".into()));
        task.join().unwrap();
        let expected = if error == "bad_refresh_token" {
            Error::Revoked
        } else {
            Error::ClientCredentials
        };
        assert_eq!(result.unwrap_err(), expected);
    }
}

#[test]
fn tokens_that_never_expire_are_refused() {
    let (client, _requests, task) = github(vec![(
        200,
        json!({"access_token": "ghu_synthetic", "token_type": "bearer", "scope": ""}),
    )]);
    let result = client.refresh("Iv23synthetic", None, &Secret::new("ghr_old".into()));
    task.join().unwrap();
    assert_eq!(result.unwrap_err(), Error::NotExpiring);
}

#[test]
fn a_web_sign_in_code_is_exchanged_with_the_secret_and_verifier() {
    let (client, requests, task) = github(vec![(200, tokens())]);
    let chain = client
        .exchange_code(
            "Iv23synthetic",
            &Secret::new("synthetic-client-secret".into()),
            &Secret::new("code123".into()),
            "http://127.0.0.1:47614/callback",
            &Secret::new("verifier-synthetic".into()),
        )
        .unwrap();
    task.join().unwrap();
    assert_eq!(chain.access_token.expose(), "ghu_synthetic_access");
    let sent = fields(&requests.lock().unwrap()[0].1);
    assert!(sent.contains(&("client_secret".into(), "synthetic-client-secret".into())));
    assert!(sent.contains(&("code_verifier".into(), "verifier-synthetic".into())));
    assert!(sent.contains(&("redirect_uri".into(), "http:%2F%2F127.0.0.1:47614%2Fcallback".into())));
}

#[test]
fn a_manifest_code_becomes_the_app_without_its_private_key() {
    let (client, requests, task) = github(vec![(
        201,
        json!({"id": 42, "slug": "horizon-synthetic", "client_id": "Iv23synthetic",
               "client_secret": "synthetic-client-secret", "html_url": "https://github.com/apps/horizon-synthetic",
               "pem": "-----BEGIN RSA PRIVATE KEY-----synthetic", "webhook_secret": null}),
    )]);
    let app = client.convert_manifest(&Secret::new("abc123".into())).unwrap();
    task.join().unwrap();
    assert_eq!(
        (app.id, app.slug.as_str(), app.client_id.as_str()),
        (42, "horizon-synthetic", "Iv23synthetic")
    );
    assert_eq!(app.client_secret.expose(), "synthetic-client-secret");
    assert!(!format!("{app:?}").contains("synthetic-client-secret"));
    assert_eq!(
        requests.lock().unwrap()[0].0,
        "POST /app-manifests/abc123/conversions HTTP/1.1"
    );
    assert_eq!(
        client.convert_manifest(&Secret::new("../x".into())).unwrap_err(),
        Error::InvalidResponse,
        "no path injection"
    );
}

#[test]
fn an_expired_manifest_code_is_a_refusal() {
    let (client, _requests, task) = github(vec![(404, json!({"message": "Not Found"}))]);
    assert_eq!(
        client.convert_manifest(&Secret::new("abc123".into())).unwrap_err(),
        Error::Refused("HTTP 404".into())
    );
    task.join().unwrap();
}

#[test]
fn the_manifest_asks_for_contents_and_pull_requests_only() {
    let manifest = manifest(
        "Horizon (example)",
        "https://example.com",
        "http://127.0.0.1:1234/manifest",
        "http://127.0.0.1/callback",
    );
    assert_eq!(manifest["public"], false);
    assert_eq!(manifest["hook_attributes"]["active"], false);
    assert_eq!(
        manifest["default_permissions"],
        json!({"contents": "write", "pull_requests": "write", "metadata": "read"})
    );
    assert_eq!(manifest["redirect_url"], "http://127.0.0.1:1234/manifest");
    assert_eq!(manifest["callback_urls"], json!(["http://127.0.0.1/callback"]));
}

#[test]
fn only_a_loopback_address_can_replace_github() {
    assert!(Client::loopback("192.0.2.1:443".parse().unwrap()).is_err());
}

#[test]
fn oversized_lifetimes_and_intervals_are_malformed_answers_not_panics() {
    let mut huge_access = tokens();
    huge_access["expires_in"] = json!(u64::MAX);
    let mut huge_refresh = tokens();
    huge_refresh["refresh_token_expires_in"] = json!(u64::MAX);
    for answer in [huge_access, huge_refresh] {
        let (client, _requests, task) = github(vec![(200, answer)]);
        let result = client.refresh("Iv23synthetic", None, &Secret::new("ghr_old".into()));
        task.join().unwrap();
        assert_eq!(result.unwrap_err(), Error::InvalidResponse);
    }
    for (expires_in, interval) in [(u64::MAX, 5), (899, u64::MAX)] {
        let (client, _requests, task) = github(vec![(
            200,
            json!({"device_code": "dc", "user_code": "ABCD-1234",
                   "verification_uri": "https://github.com/login/device",
                   "expires_in": expires_in, "interval": interval}),
        )]);
        assert_eq!(
            client.start_device("Iv23synthetic").unwrap_err(),
            Error::InvalidResponse
        );
        task.join().unwrap();
    }
    let (client, _requests, task) = github(vec![
        (
            200,
            json!({"device_code": "dc", "user_code": "ABCD-1234",
                   "verification_uri": "https://github.com/login/device", "expires_in": 899}),
        ),
        (200, json!({"error": "slow_down", "interval": u64::MAX})),
    ]);
    let mut code = device(&client);
    assert_eq!(
        client.poll_device("Iv23synthetic", &mut code).unwrap_err(),
        Error::InvalidResponse
    );
    task.join().unwrap();
}

#[test]
fn a_manifest_answer_with_unsafe_values_is_refused() {
    let (client, _requests, task) = github(vec![(
        201,
        json!({"id": 42, "slug": "../evil", "client_id": "Iv23synthetic",
               "client_secret": "s", "html_url": "https://github.com/apps/x"}),
    )]);
    assert_eq!(
        client.convert_manifest(&Secret::new("abc123".into())).unwrap_err(),
        Error::InvalidResponse
    );
    task.join().unwrap();
}

#[test]
fn a_token_answer_must_be_a_bearer_token() {
    for token_type in [json!("mac"), Value::Null] {
        let mut answer = tokens();
        answer["token_type"] = token_type;
        let (client, _requests, task) = github(vec![(200, answer)]);
        let result = client.refresh("Iv23synthetic", None, &Secret::new("ghr_old".into()));
        task.join().unwrap();
        assert_eq!(result.unwrap_err(), Error::InvalidResponse);
    }
}

#[test]
fn the_user_is_named_with_github_private_commit_address() {
    let (client, requests, task) = github(vec![
        (200, json!({"id": 7, "login": "octo-cat", "name": "Octo Cat"})),
        (200, json!({"id": 8, "login": "plain", "name": null})),
        (401, json!({"message": "Bad credentials"})),
    ]);
    let token = Secret::new("ghu_synthetic".into());
    let user = client.user(&token).unwrap();
    assert_eq!(user.email, "7+octo-cat@users.noreply.github.com");
    assert_eq!(user.name, "Octo Cat");
    assert_eq!(
        client.user(&token).unwrap().name,
        "plain",
        "the login stands in for no name"
    );
    assert_eq!(client.user(&token).unwrap_err(), Error::Revoked);
    task.join().unwrap();
    assert_eq!(requests.lock().unwrap()[0].0, "GET /user HTTP/1.1");
}

#[test]
fn installed_repositories_reads_every_page_of_every_installation() {
    let page = |prefix: &str, from: usize, count: usize| -> Value {
        let repositories: Vec<Value> = (from..from + count)
            .map(|n| json!({"full_name": format!("Acme/{prefix}-{n}")}))
            .collect();
        json!({"total_count": 101, "repositories": repositories})
    };
    // Each installation has 101 repositories: the second is read in full although the
    // first already gave more names than its own total.
    let (client, requests, task) = github(vec![
        (200, json!({"installations": [{"id": 1}, {"id": 2}]})),
        (200, page("one", 0, 100)),
        (200, page("one", 100, 1)),
        (200, page("two", 0, 100)),
        (200, page("two", 100, 1)),
    ]);
    let names = client
        .installed_repositories(&Secret::new("ghu_synthetic".into()))
        .unwrap();
    task.join().unwrap();
    assert_eq!(names.len(), 202);
    assert!(names.contains(&"acme/two-100".to_owned()), "lowercased");
    let lines: Vec<String> = requests.lock().unwrap().iter().map(|(line, _)| line.clone()).collect();
    assert_eq!(
        lines[4],
        "GET /user/installations/2/repositories?per_page=100&page=2 HTTP/1.1"
    );
}

#[test]
fn repository_names_follow_github_rules() {
    for good in ["acme/web", "a-b/c.d_e", "acme/.github"] {
        assert!(valid_repository(good), "{good}");
    }
    for bad in ["acme", "acme/web/x", "acme/..", "-/x", "acme/we b", "acme/"] {
        assert!(!valid_repository(bad), "{bad}");
    }
}

#[test]
fn an_installation_beyond_the_page_limit_is_an_error_not_partial_data() {
    let full = |n: usize| -> Value {
        let repositories: Vec<Value> = (0..100)
            .map(|i| json!({"full_name": format!("acme/r-{n}-{i}")}))
            .collect();
        json!({"total_count": 5000, "repositories": repositories})
    };
    let mut responses = vec![(200, json!({"installations": [{"id": 1}]}))];
    responses.extend((0..20).map(|n| (200, full(n))));
    let (client, _requests, task) = github(responses);
    let result = client.installed_repositories(&Secret::new("ghu_synthetic".into()));
    task.join().unwrap();
    assert_eq!(result.unwrap_err(), Error::TooMany);
}

#[test]
fn a_device_sign_in_asks_for_a_scope_only_when_given_one() {
    let code = json!({"device_code": "synthetic-device", "user_code": "WDJB-MJHT",
                      "verification_uri": "https://github.com/login/device", "expires_in": 900, "interval": 5});
    let (client, requests, task) = github(vec![(200, code.clone()), (200, code)]);
    client
        .start_device_with_scope("Ov23synthetic", "write:packages")
        .unwrap();
    client.start_device("Iv23synthetic").unwrap();
    task.join().unwrap();
    let sent = requests.lock().unwrap();
    assert_eq!(
        fields(&sent[0].1),
        [
            ("client_id".to_owned(), "Ov23synthetic".to_owned()),
            ("scope".to_owned(), "write:packages".to_owned())
        ]
    );
    assert_eq!(
        fields(&sent[1].1),
        [("client_id".to_owned(), "Iv23synthetic".to_owned())]
    );
}

#[test]
fn a_repository_size_is_read_in_bytes_with_or_without_a_token() {
    let (client, requests, task) = github(vec![(200, json!({"size": 2048})), (200, json!({"size": 1}))]);
    assert_eq!(client.repository_size("acme/web", None).unwrap(), 2048 * 1024);
    let token = Secret::new("ghu_synthetic".into());
    assert_eq!(client.repository_size("acme/private", Some(&token)).unwrap(), 1024);
    task.join().unwrap();
    let lines: Vec<String> = requests.lock().unwrap().iter().map(|(line, _)| line.clone()).collect();
    assert_eq!(
        lines,
        ["GET /repos/acme/web HTTP/1.1", "GET /repos/acme/private HTTP/1.1"]
    );
    assert!(client.repository_size("acme/../x", None).is_err(), "never sent");
}
