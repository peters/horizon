use super::{Challenge, MISSING, OTHER_REGISTRY, REFUSED, Reference, api_host, verify_pull_at};
use crate::{Cancellation, CloudError, Credential, host::RegistryLogin};
use std::{
    io::{BufRead as _, BufReader, Write as _},
    net::TcpListener,
    sync::{Arc, Mutex},
};

fn login(server: &str) -> RegistryLogin {
    RegistryLogin {
        server: server.into(),
        username: "puller".into(),
        password: Credential::new("pull-secret".into()).unwrap(),
    }
}

/// A registry on loopback that answers each request in turn with `responses`
/// (status, extra header, body) and records each request line with its
/// Authorization header. `@REALM@` in a header becomes its own token URL.
fn registry(
    responses: Vec<(u16, &'static str, &'static str)>,
) -> (String, Arc<Mutex<Vec<String>>>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (recorded, host) = (seen.clone(), address.clone());
    let task = std::thread::spawn(move || {
        for (status, header, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut authorization = String::new();
            loop {
                let mut header_line = String::new();
                reader.read_line(&mut header_line).unwrap();
                if header_line.trim().is_empty() {
                    break;
                }
                if let Some(value) = header_line.to_ascii_lowercase().strip_prefix("authorization:") {
                    authorization = value.trim().split(' ').next().unwrap_or_default().to_owned();
                }
            }
            recorded
                .lock()
                .unwrap()
                .push(format!("{} {authorization}", line.trim()));
            let header = header.replace("@REALM@", &format!("http://{host}/token"));
            let extra = if header.is_empty() {
                String::new()
            } else {
                format!("{header}\r\n")
            };
            write!(
                stream,
                "HTTP/1.1 {status} X\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    (address, seen, task)
}

fn check(address: &str, responses_done: std::thread::JoinHandle<()>) -> Result<(), CloudError> {
    let image = format!("{address}/team/worker@sha256:{}", "a".repeat(64));
    let result = verify_pull_at(&login(address), &image, &Cancellation::default(), "http");
    responses_done.join().unwrap();
    result
}

const BEARER: &str =
    "WWW-Authenticate: Bearer realm=\"@REALM@\",service=\"registry.test\",scope=\"repository:team/worker:pull\"";

#[test]
fn a_login_the_token_service_accepts_reads_the_manifest_with_its_token() {
    let (address, seen, task) = registry(vec![
        (401, BEARER, ""),
        (200, "Content-Type: application/json", r#"{"access_token":"granted"}"#),
        (200, "", ""),
    ]);
    check(&address, task).unwrap();
    let seen = seen.lock().unwrap();
    assert!(seen[0].starts_with("HEAD /v2/team/worker/manifests/sha256:") && seen[0].ends_with(' '));
    assert!(
        seen[1].starts_with("GET /token?scope=repository%3Ateam%2Fworker%3Apull&service=registry.test "),
        "{}",
        seen[1]
    );
    assert!(seen[1].ends_with(" basic"), "the login goes only to the token service");
    assert!(seen[2].ends_with(" bearer"));
}

#[test]
fn an_expired_or_revoked_login_is_refused_before_a_server_is_created() {
    for refusal in [401, 403] {
        let (address, _, task) = registry(vec![(401, BEARER, ""), (refusal, "", "")]);
        assert!(matches!(check(&address, task), Err(CloudError::Invalid(message)) if message == REFUSED));
    }
    // A token that the registry then refuses for the image is refused the same way.
    let (address, _, task) = registry(vec![
        (401, BEARER, ""),
        (200, "", r#"{"token":"granted"}"#),
        (403, "", ""),
    ]);
    assert!(matches!(check(&address, task), Err(CloudError::Invalid(message)) if message == REFUSED));
    // Registries that ask for basic authentication directly.
    let (address, seen, task) = registry(vec![
        (401, "WWW-Authenticate: Basic realm=\"registry\"", ""),
        (401, "", ""),
    ]);
    assert!(matches!(check(&address, task), Err(CloudError::Invalid(message)) if message == REFUSED));
    assert!(seen.lock().unwrap()[1].ends_with(" basic"));
}

#[test]
fn a_missing_image_is_refused_and_anything_indefinite_is_left_to_the_host() {
    let (address, _, task) = registry(vec![
        (401, BEARER, ""),
        (200, "", r#"{"token":"granted"}"#),
        (404, "", ""),
    ]);
    assert!(matches!(check(&address, task), Err(CloudError::Invalid(message)) if message == MISSING));
    // A public image, a failing token service, an unreadable token and an unknown
    // challenge leave the pull to the host.
    for responses in [
        vec![(200, "", "")],
        vec![(401, BEARER, ""), (503, "", "")],
        vec![(401, BEARER, ""), (200, "", "not json")],
        vec![(401, "WWW-Authenticate: Negotiate", "")],
        vec![(500, "", "")],
    ] {
        let (address, _, task) = registry(responses);
        check(&address, task).unwrap();
    }
    // Nothing listening: the registry cannot be reached.
    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let image = format!("{closed}/team/worker@sha256:{}", "a".repeat(64));
    verify_pull_at(&login(&closed), &image, &Cancellation::default(), "http").unwrap();
}

#[test]
fn a_token_service_on_another_scheme_never_receives_the_login() {
    let (address, seen, task) = registry(vec![(
        401,
        "WWW-Authenticate: Bearer realm=\"https://elsewhere.test/token\",service=\"x\"",
        "",
    )]);
    check(&address, task).unwrap();
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn references_resolve_as_docker_resolves_them() {
    let parse = |image: &str| Reference::parse(image).map(|r| (r.host, r.repository, r.target));
    let owned =
        |host: &str, repository: &str, reference: &str| Some((host.into(), repository.into(), reference.into()));
    assert_eq!(
        parse("Example.azurecr.io/team/worker@sha256:abc"),
        owned("example.azurecr.io", "team/worker", "sha256:abc")
    );
    assert_eq!(
        parse("registry.test:5000/worker:v1"),
        owned("registry.test:5000", "worker", "v1")
    );
    assert_eq!(
        parse("registry.test:5000/worker"),
        owned("registry.test:5000", "worker", "latest")
    );
    assert_eq!(
        parse("ubuntu"),
        owned("registry-1.docker.io", "library/ubuntu", "latest")
    );
    assert_eq!(
        parse("docker.io/team/worker:v2"),
        owned("registry-1.docker.io", "team/worker", "v2")
    );
    assert_eq!(
        parse("localhost/worker@sha256:abc"),
        owned("localhost", "worker", "sha256:abc")
    );
    assert_eq!(parse("registry.test/work er"), None);
}

#[test]
fn challenges_are_read_with_quoted_and_bare_values() {
    assert_eq!(Challenge::parse("Basic realm=\"x\""), Some(Challenge::Basic));
    assert_eq!(
        Challenge::parse("Bearer realm=\"https://a.test/token\", service=\"a.test\",scope=\"repository:x:pull\""),
        Some(Challenge::Bearer {
            realm: "https://a.test/token".into(),
            service: Some("a.test".into())
        })
    );
    assert_eq!(
        Challenge::parse("bearer realm=https://a.test/token"),
        Some(Challenge::Bearer {
            realm: "https://a.test/token".into(),
            service: None
        })
    );
    assert_eq!(
        Challenge::parse("Bearer service=\"a\""),
        None,
        "a bearer challenge needs its realm"
    );
    assert_eq!(Challenge::parse("Negotiate"), None);
}

#[test]
fn an_anonymous_not_found_is_left_to_the_host() {
    // Registries may answer 404 to an anonymous request to hide a private image.
    let (address, seen, task) = registry(vec![(404, "", "")]);
    check(&address, task).unwrap();
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "the login is not tried without a challenge"
    );
}

#[test]
fn a_login_for_another_registry_is_never_sent() {
    let (address, seen, task) = registry(vec![]);
    let image = format!("{address}/team/worker@sha256:{}", "a".repeat(64));
    let refused = verify_pull_at(&login("other.test"), &image, &Cancellation::default(), "http");
    task.join().unwrap();
    assert!(matches!(refused, Err(CloudError::Invalid(message)) if message == OTHER_REGISTRY));
    assert!(seen.lock().unwrap().is_empty(), "nothing is asked of any registry");
    // Docker Hub's names are one registry; case does not matter.
    for (login, image) in [
        ("docker.io", "ubuntu"),
        ("index.docker.io", "docker.io/team/worker"),
        ("Registry.Test", "registry.test/worker"),
    ] {
        assert_eq!(
            api_host(login),
            Reference::parse(image).unwrap().host,
            "{login} {image}"
        );
    }
}

#[test]
fn a_cancelled_check_is_never_left_to_the_host() {
    // An indefinite answer is left to the host only while nobody cancelled.
    let (address, _, task) = registry(vec![(401, BEARER, ""), (503, "", "")]);
    let cancel = Cancellation::default();
    cancel.cancel();
    let image = format!("{address}/team/worker@sha256:{}", "a".repeat(64));
    let checked = super::check_pull(&login(&address), &image, &Cancellation::default(), "http");
    task.join().unwrap();
    // Left to the host when nobody cancelled ...
    checked.unwrap();
    // ... and cancelled when someone did, whatever the registry answered.
    let (address, _, task) = registry(vec![]);
    let image = format!("{address}/team/worker@sha256:{}", "a".repeat(64));
    assert!(matches!(
        verify_pull_at(&login(&address), &image, &cancel, "http"),
        Err(CloudError::Cancelled)
    ));
    task.join().unwrap();
}

#[test]
fn a_token_in_either_field_is_used() {
    let (address, seen, task) = registry(vec![
        (401, BEARER, ""),
        (200, "", r#"{"token":"first","access_token":"first"}"#),
        (200, "", ""),
    ]);
    check(&address, task).unwrap();
    assert!(seen.lock().unwrap()[2].ends_with(" bearer"));
}
