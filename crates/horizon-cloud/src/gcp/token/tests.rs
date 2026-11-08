use super::{jwt, key::PrivateKey, *};
use base64::Engine as _;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

/// A synthetic RSA key made only for these tests. It belongs to no Google account,
/// it is not a credential, and it is safe to publish. `ring` cannot make an RSA key
/// and Horizon adds no dependency that can, so the key is a constant here rather
/// than something the test generates at run time.
const TEST_KEY: &str = "\
-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDfFoRgxABuVxu1
ChoI1h24i9VqkZSpBZJBS+Et7ePKEFy2wt+3k37wajcnZlsoj7+saM7FYNXe1SeA
n+7D1slhEGYzxbox6IXxf0ruyBxrndRMk+reXM62eix8smmxZi2lL5cFmyHqgNkr
OICWT0hbyUj45jBxPZtPhzRZyzhr4Awqz781YLe274r6NuhjnRe6gGHUG/rE1nRq
JwZ9m/q2AfcAm27JBwfujnq60Ggki6PSMgzwxyKV5tfQdmn0s0sHgHzEIuRvK0k2
88Zr2xW5yw0F4Olghm7O1GuJjos7E+P2N/Ufc4QegUoMMaSEpX+I4EWlpDk+fwCB
jQLqUY3VAgMBAAECggEAUP8ZaBKxv14UGrqEslGfL9phTZR2v5lcvSViJ+1Xx40C
RH7jgADm1T4TMVRp44OxiVTyQdqDPCjy8zHeHC+XJQ0xtjljfWxZeVGIsMJqAy6m
HnmUDFDw4nLgRRP36Odayj9Jm0j0WvKWJqmYkjpN4Llnq6rTysGd6pWBzdpJbWyZ
I02Zd8zzpG1VAdEpBM5+JThPq7fMuEFWDRSFq2em5ZU2JBIi0H4fRehzuMOySVQO
oNazyfTPq7utAn4T284WZKuzU/FLUYX7SO08TICQlrHpDbzFVjx50kMUfRX5HtU5
gp39QcyxW5mB/C+T3i1Hlblg6v5SOhtF66Qu0+/EnwKBgQDxazwHsixGapVhfRk9
QZYwA1Tl4O8yAcE8tJEAxCIRGSXA8JyZpwcVkSEQNTEXyGbsvcQOdJrxZDyPJPm6
G6hkUNvB1Zu9QqafF+vlPS9LrC7liphoHpl5t2p1V0dbvLgQqzupDhdRbrGl7Jdf
zwJkPGyiCU13AxenXxAR3+oTxwKBgQDsj9qWYyT/utLe+iW5JCbWdiTqyAAPhEGf
CEF6JAnfZm4pmcvhUjXqU+tqkAsHxSagFCFXeuQpI/ytbJL4Tz09TYNRnMgGlQU+
sLh/tIMItETdkS3jCegYAw3Qu4XhUSLDLu6Xf/zpPR5UeVts/NkUplCcwjvyCLw1
NOfuw40ZgwKBgCRbsAjArqZUOvcZrglprULB3bFZNOzlEsMfto7U+wR0Mxlho1QI
sapv1vjn6eLZDZct5S8AJK2iGPS97tY2gB/Lha0ljHTuasWfBqnZkxvkbJOpUdas
dRS9qMzPPjH0tAvenfeR5bUf5KVGgm2tcYDDEOE+YWpBDvjn+jWW8VvVAoGBAMoA
8Tx4PxDs3MKxQuM8/rklu9KMpJibbFm7+AWvnoqY8jst7I5k4EtQC/g8Nl8VOC17
4xjLhOeopk0FKN9G8tlVRmy7f4c3W3oz9+h9IWOeZCv7m/6C6CERAowahXFpElEv
02elUDdvDGZAFxUTON/MQYwKxJuiX+zjV6ZocCW3AoGBANpiDXUYp48nN61vUT4S
dgoBebZi+bDBmr2MNZOJXBsfY8KYA2NriwMLl9eb2TBpiCkGKVsb6ap57YSPMDTg
2AxTvCeknVbhPxb/ukuCoM7o675WVB9unweMCxfeqV3hb4EF//pnrHg3OSun+HCn
PzEjX4cEZKzQg90RHrhzFnI6
-----END PRIVATE KEY-----
";

const TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
/// 2026-01-01T00:00:00Z, which every test uses instead of the system clock.
const START: u64 = 1_767_225_600;

fn decode(text: &str) -> String {
    String::from_utf8(base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(text).unwrap()).unwrap()
}

/// The DER that the PEM block carries, decoded the way the test can without the
/// module's key type.
fn der(pem: &str) -> Vec<u8> {
    let body: String = pem.lines().filter(|line| !line.starts_with("-----")).collect();
    base64::engine::general_purpose::STANDARD.decode(body).unwrap()
}

fn private_key(pem: &str) -> Result<PrivateKey> {
    PrivateKey::new(Zeroizing::new(pem.to_owned()))
}

fn key() -> PrivateKey {
    private_key(TEST_KEY).unwrap()
}

fn document(token_uri: &str) -> String {
    json!({
        "type": "service_account",
        "project_id": "horizon-test",
        "private_key_id": "0000000000000000000000000000000000000000",
        "private_key": TEST_KEY,
        "client_email": "worker@horizon-test.iam.gserviceaccount.com",
        "client_id": "100000000000000000000",
        "token_uri": token_uri,
    })
    .to_string()
}

fn account(token_uri: &str) -> ServiceAccount {
    ServiceAccount::parse(&document(token_uri)).unwrap()
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

fn clock(start: u64) -> (Clock, Arc<Mutex<SystemTime>>) {
    let now = Arc::new(Mutex::new(at(start)));
    let handle = Arc::clone(&now);
    let clock: Clock = Arc::new(move || *handle.lock().unwrap());
    (clock, now)
}

fn advance(now: &Mutex<SystemTime>, seconds: u64) {
    *now.lock().unwrap() += Duration::from_secs(seconds);
}

type Requests = Arc<Mutex<Vec<String>>>;

/// A scripted token endpoint on this machine. It answers each connection with the
/// next answer and records the body it read.
fn endpoint(answers: Vec<(u16, Value)>) -> (String, Requests, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let uri = format!("http://{}/token", listener.local_addr().unwrap());
    let requests = Requests::default();
    let observed = Arc::clone(&requests);
    let task = thread::spawn(move || {
        for (status, answer) in answers {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut input = Vec::new();
            let mut buffer = [0; 4096];
            let (start, length) = loop {
                let read = stream.read(&mut buffer).unwrap();
                input.extend_from_slice(&buffer[..read]);
                if let Some(end) = input.windows(4).position(|window| window == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&input[..end]).into_owned();
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    break (end + 4, length);
                }
            };
            while input.len() < start + length {
                let read = stream.read(&mut buffer).unwrap();
                input.extend_from_slice(&buffer[..read]);
            }
            observed
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&input[start..start + length]).into_owned());
            let text = answer.to_string();
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            )
            .unwrap();
        }
    });
    (uri, requests, task)
}

/// The claims of the assertion at the end of an exchange body.
fn claims(body: &str) -> Value {
    let assertion = body.strip_prefix(EXCHANGE).unwrap();
    let (_, claims) = assertion.split_once('.').unwrap();
    serde_json::from_str(&decode(claims.split('.').next().unwrap())).unwrap()
}

#[test]
fn a_valid_key_signs_and_the_signature_verifies() {
    let pair = ring::signature::RsaKeyPair::from_pkcs8(&der(TEST_KEY)).unwrap();
    let public: &[u8] = pair.public().as_ref();
    let signature = key().sign("header.claims").unwrap();
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature)
        .unwrap();
    ring::signature::UnparsedPublicKey::new(&ring::signature::RSA_PKCS1_2048_8192_SHA256, public)
        .verify(b"header.claims", &bytes)
        .unwrap();
}

#[test]
fn a_malformed_pem_is_a_typed_error() {
    let pkcs1 = TEST_KEY
        .replace("BEGIN PRIVATE KEY", "BEGIN RSA PRIVATE KEY")
        .replace("END PRIVATE KEY", "END RSA PRIVATE KEY");
    let truncated = TEST_KEY.lines().take(4).collect::<Vec<_>>().join("\n");
    for pem in [
        String::new(),
        "not a pem block".to_owned(),
        "-----BEGIN PRIVATE KEY-----\n-----END PRIVATE KEY-----\n".to_owned(),
        "-----BEGIN PRIVATE KEY-----\nnot base64 at all\n-----END PRIVATE KEY-----\n".to_owned(),
        pkcs1,
        truncated,
    ] {
        assert_eq!(private_key(&pem).unwrap_err(), Error::PrivateKey, "{pem:.40}");
    }
}

#[test]
fn a_document_that_is_not_a_service_account_is_refused() {
    let other = json!({
        "type": "authorized_user",
        "client_email": "worker@horizon-test.iam.gserviceaccount.com",
        "private_key": TEST_KEY,
        "project_id": "horizon-test",
        "token_uri": TOKEN_URI,
    })
    .to_string();
    assert_eq!(ServiceAccount::parse(&other).unwrap_err(), Error::NotServiceAccount);
    assert_eq!(ServiceAccount::parse("not json").unwrap_err(), Error::Document);
    assert_eq!(ServiceAccount::parse("{}").unwrap_err(), Error::NotServiceAccount);
}

#[test]
fn an_unusable_document_names_the_field() {
    let value: Value = serde_json::from_str(&document(TOKEN_URI)).unwrap();
    for (field, expected) in [
        ("client_email", Error::Field("client_email")),
        ("project_id", Error::Field("project_id")),
        ("token_uri", Error::Field("token_uri")),
        ("private_key", Error::Field("private_key")),
    ] {
        let mut empty = value.clone();
        empty[field] = json!("");
        assert_eq!(ServiceAccount::parse(&empty.to_string()).unwrap_err(), expected);

        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert_eq!(ServiceAccount::parse(&missing.to_string()).unwrap_err(), expected);
    }
}

#[test]
fn an_assertion_never_travels_in_clear_text() {
    assert!(ServiceAccount::parse(&document(TOKEN_URI)).is_ok());
    assert!(ServiceAccount::parse(&document("http://127.0.0.1:8080/token")).is_ok());
    for uri in [
        "http://example.com/token",
        "http://oauth2.googleapis.com/token",
        "https:///token",
        "https://",
        "ftp://oauth2.googleapis.com/token",
    ] {
        assert_eq!(
            ServiceAccount::parse(&document(uri)).unwrap_err(),
            Error::Field("token_uri"),
            "{uri}"
        );
    }
}

#[test]
fn the_assertion_goes_only_to_a_google_token_host() {
    // The key document is untrusted and its `token_uri` is the destination of a signed
    // assertion, so a host that merely looks like a name is not enough. `aud` is that same
    // uri, so a stolen assertion is not replayable at Google, but it must not be sent at all.
    for uri in [
        "https://oauth2.googleapis.com/token",
        "https://accounts.google.com/o/oauth2/token",
    ] {
        assert!(ServiceAccount::parse(&document(uri)).is_ok(), "{uri}");
    }
    for uri in [
        "https://attacker.example/token",
        "https://oauth2.googleapis.com.attacker.example/token",
        "https://evil.oauth2.googleapis.com/token",
        "https://oauth2.googleapis.com:8443/token",
        "https://127.0.0.1/token",
    ] {
        assert_eq!(
            ServiceAccount::parse(&document(uri)).unwrap_err(),
            Error::Field("token_uri"),
            "{uri}"
        );
    }
}

#[test]
fn the_assertion_is_exactly_as_documented() {
    let account = account(TOKEN_URI);
    let unsigned = jwt::unsigned(&account, 1_767_225_600).unwrap();
    let (header, claims) = unsigned.split_once('.').unwrap();
    assert_eq!(decode(header), r#"{"alg":"RS256","typ":"JWT"}"#);
    assert_eq!(
        decode(claims),
        concat!(
            r#"{"iss":"worker@horizon-test.iam.gserviceaccount.com","#,
            r#""scope":"https://www.googleapis.com/auth/cloud-platform","#,
            r#""aud":"https://oauth2.googleapis.com/token","iat":1767225600,"exp":1767229200}"#
        )
    );

    let assertion = jwt::assertion(&account, 1_767_225_600).unwrap();
    let (signed, signature) = assertion.rsplit_once('.').unwrap();
    assert_eq!(signed, unsigned);
    let pair = ring::signature::RsaKeyPair::from_pkcs8(&der(TEST_KEY)).unwrap();
    let public: &[u8] = pair.public().as_ref();
    ring::signature::UnparsedPublicKey::new(&ring::signature::RSA_PKCS1_2048_8192_SHA256, public)
        .verify(
            signed.as_bytes(),
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(signature)
                .unwrap(),
        )
        .unwrap();
}

#[test]
fn a_fresh_token_is_reused_and_renewed_before_it_expires() {
    let (uri, requests, task) = endpoint(vec![
        (
            200,
            json!({"access_token": "token-one", "expires_in": 3599, "token_type": "Bearer"}),
        ),
        (
            200,
            json!({"access_token": "token-two", "expires_in": 3599, "token_type": "Bearer"}),
        ),
    ]);
    let (now, handle) = clock(START);
    let cache = TokenCache::with_clock(account(&uri), now);

    let first = cache.token().unwrap();
    assert_eq!(first.expose(), "token-one");
    assert_eq!(first.expires_at(), at(START + 3599));
    assert_eq!(requests.lock().unwrap().len(), 1);
    let body = requests.lock().unwrap()[0].clone();
    assert!(body.starts_with(EXCHANGE), "{body}");
    assert_eq!(claims(&body)["iat"], json!(START));
    assert_eq!(claims(&body)["exp"], json!(START + 3600));
    assert_eq!(claims(&body)["aud"], json!(uri));

    for _ in 0..3 {
        assert_eq!(cache.token().unwrap().expose(), "token-one");
    }
    assert_eq!(requests.lock().unwrap().len(), 1);

    // 3599 seconds of lifetime, less the 300 second margin.
    advance(&handle, 3298);
    assert_eq!(cache.token().unwrap().expose(), "token-one");
    assert_eq!(requests.lock().unwrap().len(), 1);

    advance(&handle, 1);
    let second = cache.token().unwrap();
    assert_eq!(second.expose(), "token-two");
    assert_eq!(second.expires_at(), at(START + 3299 + 3599));
    assert_eq!(requests.lock().unwrap().len(), 2);
    let body = requests.lock().unwrap()[1].clone();
    assert_eq!(claims(&body)["iat"], json!(START + 3299));
    task.join().unwrap();
}

#[test]
fn a_short_lived_token_is_still_used_once() {
    let (uri, requests, task) = endpoint(vec![
        (200, json!({"access_token": "short-one", "expires_in": 60})),
        (200, json!({"access_token": "short-two", "expires_in": 60})),
    ]);
    let (now, handle) = clock(START);
    let cache = TokenCache::with_clock(account(&uri), now);

    assert_eq!(cache.token().unwrap().expose(), "short-one");
    advance(&handle, 29);
    assert_eq!(cache.token().unwrap().expose(), "short-one");
    assert_eq!(requests.lock().unwrap().len(), 1);

    advance(&handle, 1);
    assert_eq!(cache.token().unwrap().expose(), "short-two");
    assert_eq!(requests.lock().unwrap().len(), 2);
    task.join().unwrap();
}

#[test]
fn a_failed_renewal_keeps_the_cache_trying() {
    let (uri, requests, task) = endpoint(vec![
        (200, json!({"access_token": "first", "expires_in": 60})),
        (500, json!({"message": "backend error"})),
        (200, json!({"access_token": "third", "expires_in": 60})),
    ]);
    let (now, handle) = clock(START);
    let cache = TokenCache::with_clock(account(&uri), now);

    assert_eq!(cache.token().unwrap().expose(), "first");
    advance(&handle, 60);
    assert_eq!(cache.token().unwrap_err(), Error::Refused("HTTP 500".to_owned()));
    assert_eq!(cache.token().unwrap().expose(), "third");
    assert_eq!(requests.lock().unwrap().len(), 3);
    task.join().unwrap();
}

#[test]
fn a_refused_exchange_is_a_typed_error_without_the_answer_text() {
    let (uri, _requests, task) = endpoint(vec![(
        400,
        json!({
            "error": "invalid_grant",
            "error_description": "Invalid JWT: the assertion expired on 2026-01-01",
        }),
    )]);
    let (now, _handle) = clock(START);
    let cache = TokenCache::with_clock(account(&uri), now);

    let error = cache.token().unwrap_err();
    assert_eq!(error, Error::Refused("invalid_grant".to_owned()));
    assert!(!error.to_string().contains("Invalid JWT"));
    assert!(!format!("{error:?}").contains("Invalid JWT"));
    task.join().unwrap();
}

#[test]
fn an_answer_that_is_not_a_token_is_a_typed_error() {
    for answer in [
        json!({"token_type": "Bearer"}),
        json!({"access_token": "token", "expires_in": 0}),
        json!({"access_token": "token", "expires_in": 86401}),
        json!({"access_token": "token", "expires_in": 3599, "token_type": "mac"}),
        json!({"access_token": "", "expires_in": 3599}),
        json!("<html>maintenance</html>"),
    ] {
        let (uri, _requests, task) = endpoint(vec![(200, answer.clone())]);
        let (now, _handle) = clock(START);
        let cache = TokenCache::with_clock(account(&uri), now);
        assert_eq!(cache.token().unwrap_err(), Error::InvalidResponse, "{answer}");
        task.join().unwrap();
    }
}

#[test]
fn an_unreachable_endpoint_is_a_transport_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let uri = format!("http://{}/token", listener.local_addr().unwrap());
    drop(listener);
    let (now, _handle) = clock(START);
    let cache = TokenCache::with_clock(account(&uri), now);
    assert_eq!(cache.token().unwrap_err(), Error::Transport);
}

#[test]
fn the_private_key_never_appears_in_debug() {
    let service_account = account(TOKEN_URI);
    let printed = format!("{service_account:?}");
    assert!(printed.contains("worker@horizon-test.iam.gserviceaccount.com"));
    assert!(!printed.contains("PRIVATE KEY"));
    assert!(!printed.contains("MIIE"), "{printed}");

    let token = AccessToken {
        token: Secret::new("secret-token".to_owned()),
        expires_at: at(START),
    };
    assert!(!format!("{token:?}").contains("secret-token"));

    let cache = TokenCache::with_clock(service_account, Arc::new(SystemTime::now));
    assert!(!format!("{cache:?}").contains("PRIVATE KEY"));
}
