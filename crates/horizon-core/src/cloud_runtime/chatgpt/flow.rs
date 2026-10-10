//! The loopback OAuth flow: authorize in the system browser, receive the callback on
//! 127.0.0.1, exchange the code, validate the ID token, and store the registration.
use super::{
    CONFIG_URL, Error, Result, id_token,
    store::{self, Record},
};
use crate::cloud_runtime::Cancellation;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::rand::SecureRandom as _;
use std::{
    io::{Read as _, Write as _},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, channel},
    time::{Duration, Instant},
};

mod refresh;

const AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
const RESOURCE: &str = "https://api.openai.com/v1";
/// The identity and plan-usage scopes requested for every sign-in.
const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// What the agent registration is named on the user's first sign-in.
const AGENT_NAME: &str = "Horizon";
/// The first-time registration entrypoint; never saved as the connection's client ID.
const DYNAMIC_CLIENT: &str = "dynamic_agent_client";
/// How long Horizon waits for the person to sign in.
const TIMEOUT: Duration = Duration::from_secs(600);
/// The redirect path `OpenAI` sends the code to; only the port may vary between sign-ins.
const CALLBACK_PATH: &str = "/auth/callback";

/// A pending authorization attempt: the browser round trip and its one-time values.
struct Attempt {
    state: String,
    nonce: String,
    code_verifier: String,
    client_id: String,
    /// The exact loopback URI the browser round trip used; the code exchange reuses it.
    redirect_uri: String,
    /// Whether this is a first-time registration; the callback then issues the client ID.
    registering: bool,
    id_token_hint: Option<zeroize::Zeroizing<String>>,
    login_hint: Option<String>,
    host_id: String,
}

impl Attempt {
    fn authorize_url(&self) -> String {
        let code_challenge = code_challenge(&self.code_verifier);
        let mut params = vec![
            (
                "client_id",
                if self.registering {
                    DYNAMIC_CLIENT
                } else {
                    &self.client_id
                },
            ),
            ("response_type", "code"),
            ("redirect_uri", &self.redirect_uri),
            ("scope", SCOPE),
            ("resource", RESOURCE),
            ("state", &self.state),
            ("nonce", &self.nonce),
            ("code_challenge_method", "S256"),
            ("code_challenge", code_challenge.as_str()),
            ("ext_agent_host_id", &self.host_id),
        ];
        if self.registering {
            params.push(("agent_name_hint", AGENT_NAME));
        }
        if let Some(hint) = &self.id_token_hint {
            params.push(("id_token_hint", hint.as_str()));
        }
        if let Some(hint) = &self.login_hint {
            params.push(("login_hint", hint.as_str()));
        }
        format!("{AUTHORIZE_URL}?{}", encode_query(&params))
    }

    /// A saved registration reauthorizes through its issued client ID; otherwise this
    /// is a first-time dynamic registration.
    fn prepare(root: &Path, port: u16) -> Result<Self> {
        let host_id = store::host_id(root)?;
        let (client_id, registering, id_token_hint, login_hint) = match store::default_registration(root)? {
            Some(record) if record.access_token.is_some() && record.refresh_token.is_some() => (
                record.client_id,
                false,
                (!record.id_token.is_empty()).then_some(record.id_token),
                record.email,
            ),
            _ => (String::new(), true, None, None),
        };
        Ok(Self {
            state: random_token()?,
            nonce: random_token()?,
            code_verifier: random_token()?,
            client_id,
            registering,
            redirect_uri: format!("http://127.0.0.1:{port}{CALLBACK_PATH}"),
            id_token_hint,
            login_hint,
            host_id,
        })
    }
}

/// Starts the flow: binds the loopback callback, opens the browser, and serves the code.
/// # Errors
/// The loopback listener could not be opened, or the browser could not be opened.
pub(super) fn start(
    root: &Path,
    open: fn(&str) -> std::io::Result<()>,
    cancel: Cancellation,
) -> Result<Receiver<Result<super::Connection>>> {
    cancel.check().map_err(|_| Error::Declined)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let attempt = Attempt::prepare(root, port)?;
    let url = attempt.authorize_url();
    let root: PathBuf = root.to_owned();
    let (tx, rx) = channel();
    let opening_cancel = cancel.clone();
    opening_cancel.check().map_err(|_| Error::Declined)?;
    std::thread::spawn(move || {
        let _ = tx.send(serve(&listener, &attempt, &root, &cancel));
    });
    opening_cancel.check().map_err(|_| Error::Declined)?;
    if let Err(error) = open(&url) {
        opening_cancel.cancel();
        return Err(error.into());
    }
    Ok(rx)
}

fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| Error::Invalid("the system random source is unavailable"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn code_challenge(code_verifier: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, code_verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest.as_ref())
}

fn encode_query(params: &[(&str, &str)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{key}={}", urlencode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => {
                use std::fmt::Write as _;
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// Waits for the browser's redirect, then exchanges the code and stores the result.
fn serve(listener: &TcpListener, attempt: &Attempt, root: &Path, cancel: &Cancellation) -> Result<super::Connection> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        cancel.check().map_err(|_| Error::Declined)?;
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(callback) = answer(stream, attempt) {
                    cancel.check().map_err(|_| Error::Declined)?;
                    return finish(root, attempt, callback, cancel);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(Error::Provider(
                        "ChatGPT did not finish sign-in in time. Try again.".into(),
                    ));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Answers one request. Returns the callback values when the browser delivered the
/// redirect to the configured path.
fn answer(mut stream: TcpStream, attempt: &Attempt) -> Option<Callback> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buffer = zeroize::Zeroizing::new(vec![0; 8192]);
    let mut read = 0;
    while read < buffer.len() && !buffer[..read].windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut buffer[read..]) {
            Ok(0) | Err(_) => break,
            Ok(count) => read += count,
        }
    }
    let request = std::str::from_utf8(&buffer[..read]).unwrap_or_default();
    let target = request
        .strip_prefix("GET ")
        .and_then(|rest| rest.split(' ').next())
        .unwrap_or_default();
    let (body, callback) = if target == "/" {
        (
            "Sign in with ChatGPT is waiting for the browser redirect.".to_owned(),
            None,
        )
    } else if let Some(query) = target
        .strip_prefix(CALLBACK_PATH)
        .and_then(|rest| rest.strip_prefix('?'))
    {
        let params = parse_query(query);
        if param(&params, "state") != Some(attempt.state.as_str()) {
            ("Horizon did not expect this request.".to_owned(), None)
        } else if let Some(error) = param(&params, "error") {
            (
                format!("ChatGPT sign-in ended with: {error}"),
                Some(Callback::Denied(param(&params, "error_description").map(str::to_owned))),
            )
        } else if let Some(code) = param(&params, "code") {
            let client_id = if attempt.registering {
                param(&params, "client_id").map(str::to_owned)
            } else {
                Some(param(&params, "client_id").unwrap_or(&attempt.client_id).to_owned())
            };
            (
                "Authorization received. Horizon is finishing sign-in. Return to Horizon to see the result.".to_owned(),
                Some(Callback::Code {
                    code: zeroize::Zeroizing::new(code.to_owned()),
                    client_id,
                }),
            )
        } else {
            ("Horizon did not expect this request.".to_owned(), None)
        }
    } else {
        ("Horizon did not expect this request.".to_owned(), None)
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
    callback
}

/// What the callback delivered after state validation.
enum Callback {
    Code {
        code: zeroize::Zeroizing<String>,
        client_id: Option<String>,
    },
    Denied(Option<String>),
}

fn param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((percent_decode(key), percent_decode(value)))
        })
        .collect()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) else {
                    break;
                };
                let Ok(byte) = u8::from_str_radix(hex, 16) else {
                    break;
                };
                out.push(byte);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn finish(root: &Path, attempt: &Attempt, callback: Callback, cancel: &Cancellation) -> Result<super::Connection> {
    let lock = store::session_lock(root)?;
    let (code, client_id) = match callback {
        Callback::Denied(description) => {
            let detail = description.unwrap_or_else(|| "sign-in was declined".into());
            return Err(Error::Provider(detail));
        }
        Callback::Code { code, client_id } => {
            let client_id = client_id
                .ok_or_else(|| Error::Provider("ChatGPT finished sign-in without a client ID. Try again.".into()))?;
            if attempt.registering && client_id == DYNAMIC_CLIENT {
                return Err(Error::Provider("ChatGPT did not register the agent. Try again.".into()));
            }
            if !attempt.registering && client_id != attempt.client_id {
                return Err(Error::Provider(
                    "ChatGPT returned a different client ID than the saved registration.".into(),
                ));
            }
            if !store::valid_client_id(&client_id) {
                return Err(Error::Provider(
                    "ChatGPT returned an unusable client ID. Try again.".into(),
                ));
            }
            (code, client_id)
        }
    };

    let token = exchange(&code, &client_id, &attempt.code_verifier, &attempt.redirect_uri)?;
    // A connection that cannot renew is a dead end once the access token lapses, so
    // the sign-in fails instead of storing a pair without a refresh token.
    let refresh_token = token
        .refresh_token
        .ok_or_else(|| Error::Provider("ChatGPT finished sign-in without a refresh token. Try again.".into()))?;
    let id_token = token.id_token.ok_or(Error::IdToken)?;
    let (subject, email) = id_token::validate(&id_token, &client_id, &attempt.nonce)?;
    let previous = store::registration(&lock, &client_id)?;
    // The issued client ID remains bound to its original account and workspace.
    // A new account uses a new dynamic registration, including after sign-out.
    if let Some(record) = &previous
        && record.subject != subject
    {
        return Err(Error::Provider(
            "A different ChatGPT account signed in than the saved one. Sign out first.".into(),
        ));
    }
    // A re-sign-in of the same account keeps the confirmed plan-usage notice.
    let usage_confirmed = previous
        .as_ref()
        .is_some_and(|record| record.subject == subject && record.usage_confirmed);

    let stored = Record {
        email,
        issuer: "https://auth.openai.com".into(),
        subject,
        client_id: client_id.clone(),
        ext_agent_host_id: attempt.host_id.clone(),
        id_token,
        access_token: Some(token.access_token),
        refresh_token: Some(refresh_token),
        token_type: Some(token.token_type),
        expires_in: token.expires_in,
        earliest_refresh_at: token.earliest_refresh_at,
        scopes: token.scope.as_deref().map_or_else(requested_scopes, |scope| {
            scope.split_whitespace().map(str::to_owned).collect()
        }),
        usage_confirmed,
        saved_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |age| i64::try_from(age.as_secs()).unwrap_or(0)),
    };
    cancel.check().map_err(|_| Error::Declined)?;
    store::activate(root, &stored)?;
    Ok(super::Connection::from(stored))
}

/// The scopes this flow requested; the token endpoint may omit `scope` when the grant
/// matches the request exactly, and then the requested set is the granted set.
fn requested_scopes() -> Vec<String> {
    SCOPE.split_whitespace().map(str::to_owned).collect()
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    #[serde(deserialize_with = "nonempty_string")]
    access_token: zeroize::Zeroizing<String>,
    #[serde(default, deserialize_with = "nonempty_token")]
    refresh_token: Option<zeroize::Zeroizing<String>>,
    #[serde(default, deserialize_with = "store::protected_token")]
    id_token: Option<zeroize::Zeroizing<String>>,
    #[serde(default)]
    token_type: String,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    earliest_refresh_at: Option<i64>,
}

fn nonempty_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<zeroize::Zeroizing<String>, D::Error> {
    let token = store::protected_string(deserializer)?;
    if token.trim().is_empty() {
        return Err(serde::de::Error::custom("an access token must not be empty"));
    }
    Ok(token)
}

fn nonempty_token<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<zeroize::Zeroizing<String>>, D::Error> {
    let token = store::protected_token(deserializer)?;
    if token.as_ref().is_some_and(|token| token.trim().is_empty()) {
        return Err(serde::de::Error::custom("a refresh token must not be empty"));
    }
    Ok(token)
}

/// Exchanges the authorization code for tokens at the token endpoint. No client
/// secret: this is a public client.
fn exchange(code: &str, client_id: &str, code_verifier: &str, redirect_uri: &str) -> Result<TokenResponse> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("code_verifier", code_verifier),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("resource", RESOURCE),
    ];
    let response = ureq::post(TOKEN_URL)
        .config()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .send_form(form)
        .map_err(|error| Error::Provider(error.to_string()))?;
    let status = response.status();
    if !(200..300).contains(&status.as_u16()) {
        return Err(Error::Provider(format!("the sign-in service answered {status}")));
    }
    super::response::read(response.into_body(), "token exchange")
}

/// Revokes the renewable session, then clears the stored tokens. Returns whether the
/// remote revocation was confirmed.
/// # Errors
/// The registration could not be read or written.
pub(super) fn sign_out(root: &Path, client_id: &str) -> Result<Option<bool>> {
    let lock = store::session_lock(root)?;
    let record = store::registration(&lock, client_id)?.ok_or(Error::Missing)?;
    let confirmed = record.refresh_token.as_ref().and_then(|token| revoke(token, client_id));
    store::clear_tokens(&lock, client_id)?;
    Ok(confirmed)
}

/// Ends the renewable session at the configured revocation endpoint.
fn revoke(refresh_token: &str, client_id: &str) -> Option<bool> {
    let discovery_response = ureq::get(CONFIG_URL)
        .config()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .call()
        .ok()?;
    if !(200..300).contains(&discovery_response.status().as_u16()) {
        return None;
    }
    let discovery: Discovery = super::response::read(discovery_response.into_body(), "revocation discovery").ok()?;
    let response = ureq::post(&discovery.revocation_endpoint)
        .config()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .send_form([
            ("token", refresh_token),
            ("token_type_hint", "refresh_token"),
            ("client_id", client_id),
        ])
        .ok()?;
    Some((200..300).contains(&response.status().as_u16()))
}

/// Obtains a replacement token set for one registration. The replacement refresh
/// token is stored together with the new access token, as the grant rotates.
/// # Errors
/// The registration has no refresh token, or the token endpoint refused the refresh.
pub(super) fn refresh(root: &Path, client_id: &str) -> Result<()> {
    refresh::refresh(root, client_id)
}

#[derive(serde::Deserialize)]
struct Discovery {
    revocation_endpoint: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_token_values_are_malformed_before_a_registration_can_be_written() {
        for (access, refresh) in [("", "refresh"), ("access", ""), ("  ", "refresh"), ("access", "  ")] {
            let bytes = serde_json::to_vec(&serde_json::json!({
                "access_token": access, "refresh_token": refresh, "id_token": "synthetic-id"
            }))
            .unwrap();
            assert!(matches!(
                super::super::response::parse::<TokenResponse>(&bytes, "synthetic exchange"),
                Err(Error::Malformed)
            ));
        }
    }

    #[test]
    fn a_pre_cancelled_attempt_does_not_open_the_browser_or_create_credentials() {
        fn unexpected_open(_: &str) -> std::io::Result<()> {
            panic!("a cancelled attempt must not open the browser");
        }
        let root = tempfile::tempdir().unwrap();
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            start(root.path(), unexpected_open, cancel),
            Err(Error::Declined)
        ));
        assert!(!root.path().join("chatgpt").exists());
    }

    #[test]
    fn code_challenge_is_the_base64url_sha256_of_the_verifier() {
        // Independently computed: sha256 of the verifier, base64url without padding.
        assert_eq!(
            code_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXw"),
            "LjpMKJRsJ-95DtvIeU9GCxhTWmmqrdNTCHRavuE4Z7U"
        );
    }

    #[test]
    fn urlencode_and_percent_decode_round_trip() {
        let encoded = urlencode("a b+c/d@e~f_1");
        assert_eq!(encoded, "a%20b%2Bc%2Fd%40e~f_1");
        assert_eq!(percent_decode(&encoded), "a b+c/d@e~f_1");
        assert_eq!(percent_decode("a+b"), "a b");
    }

    fn attempt(root: &std::path::Path) -> Attempt {
        Attempt::prepare(root, 1455).unwrap()
    }

    #[test]
    fn first_registration_asks_for_a_dynamic_client() {
        let root = tempfile::tempdir().unwrap();
        let attempt = attempt(root.path());
        assert!(attempt.registering);
        let url = attempt.authorize_url();
        let query = url.split_once('?').unwrap().1;
        for expected in [
            "client_id=dynamic_agent_client",
            "agent_name_hint=Horizon",
            "response_type=code",
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A1455%2Fauth%2Fcallback",
            &format!("ext_agent_host_id={}", urlencode(&attempt.host_id)),
            "resource=https%3A%2F%2Fapi.openai.com%2Fv1",
            "code_challenge_method=S256",
        ] {
            assert!(query.contains(expected), "missing {expected} in {query}");
        }
        assert!(!query.contains("id_token_hint"));
    }

    #[test]
    fn a_saved_registration_reauthorizes_without_a_name_hint() {
        let root = tempfile::tempdir().unwrap();
        store::save(
            root.path(),
            &store::Record {
                email: Some("peters@example.com".into()),
                issuer: "https://auth.openai.com".into(),
                subject: "user-1".into(),
                client_id: "oaiapp_saved".into(),
                ext_agent_host_id: "urn:uuid:00000000-0000-4000-8000-000000000000".into(),
                id_token: zeroize::Zeroizing::new("retained-id-token".into()),
                access_token: Some(zeroize::Zeroizing::new("access".into())),
                refresh_token: Some(zeroize::Zeroizing::new("refresh".into())),
                token_type: Some("Bearer".into()),
                expires_in: Some(3600),
                earliest_refresh_at: None,
                scopes: vec!["openid".into()],
                usage_confirmed: true,
                saved_at_unix: 1,
            },
        )
        .unwrap();
        let attempt = attempt(root.path());
        assert!(!attempt.registering);
        let url = attempt.authorize_url();
        let query = url.split_once('?').unwrap().1;
        assert!(query.contains("client_id=oaiapp_saved"));
        assert!(query.contains(&format!("id_token_hint={}", urlencode("retained-id-token"))));
        assert!(query.contains(&format!("login_hint={}", urlencode("peters@example.com"))));
        assert!(!query.contains("agent_name_hint"));
    }

    #[test]
    fn the_callback_requires_the_attempt_state() {
        let root = tempfile::tempdir().unwrap();
        let attempt = attempt(root.path());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let state = uuid::Uuid::new_v4().simple().to_string();
        let request = format!(
            "GET {CALLBACK_PATH}?code=the-code&state={state}&client_id=oaiapp_issued HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
        );
        client.write_all(request.as_bytes()).unwrap();
        assert!(answer(stream, &attempt).is_none(), "a wrong state is rejected");
    }

    #[test]
    fn the_callback_delivers_code_and_issued_client() {
        let root = tempfile::tempdir().unwrap();
        let attempt = attempt(root.path());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let request = format!(
            "GET {CALLBACK_PATH}?code=the-code&state={}&client_id=oaiapp_issued HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            attempt.state
        );
        client.write_all(request.as_bytes()).unwrap();
        match answer(stream, &attempt) {
            Some(Callback::Code { code, client_id }) => {
                assert_eq!(code.as_str(), "the-code");
                assert_eq!(client_id.as_deref(), Some("oaiapp_issued"));
            }
            _ => panic!("expected a code callback"),
        }
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.contains("Authorization received"));
        assert!(response.contains("Return to Horizon to see the result"));
        assert!(!response.contains("You are signed in"));
    }
    #[test]
    fn a_signed_out_registration_uses_a_new_client_for_account_switching() {
        let root = tempfile::tempdir().unwrap();
        let issued = "oaiapp_saved";
        store::save(
            root.path(),
            &store::Record {
                email: Some("user@example.com".into()),
                issuer: "https://auth.openai.com".into(),
                subject: "original-user".into(),
                client_id: issued.into(),
                ext_agent_host_id: store::host_id(root.path()).unwrap(),
                id_token: zeroize::Zeroizing::new(String::new()),
                access_token: None,
                refresh_token: None,
                token_type: None,
                expires_in: None,
                earliest_refresh_at: None,
                scopes: vec![],
                usage_confirmed: true,
                saved_at_unix: 1,
            },
        )
        .unwrap();
        let attempt = attempt(root.path());
        assert!(attempt.registering);
        assert!(attempt.authorize_url().contains("client_id=dynamic_agent_client"));
        assert!(!attempt.authorize_url().contains("id_token_hint"));
        assert_eq!(
            store::default_registration(root.path()).unwrap().unwrap().subject,
            "original-user"
        );
    }

    #[test]
    fn a_refresh_response_does_not_require_an_id_token() {
        let token: TokenResponse = serde_json::from_str(
            r#"{"access_token":"synthetic-access","refresh_token":"synthetic-refresh","expires_in":3600}"#,
        )
        .unwrap();
        assert!(token.id_token.is_none());
        assert_eq!(token.refresh_token.unwrap().as_str(), "synthetic-refresh");
    }

    #[test]
    fn a_returning_callback_cannot_replace_the_selected_client_id() {
        let root = tempfile::tempdir().unwrap();
        let mut attempt = attempt(root.path());
        attempt.registering = false;
        attempt.client_id = "oaiapp_selected".into();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let request = format!(
            "GET {CALLBACK_PATH}?code=synthetic-code&state={}&client_id=oaiapp_different HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            attempt.state
        );
        client.write_all(request.as_bytes()).unwrap();
        let callback = answer(stream, &attempt).unwrap();
        assert!(matches!(
            finish(root.path(), &attempt, callback, &Cancellation::default()),
            Err(Error::Provider(_))
        ));
        assert!(store::connections(root.path()).unwrap().is_empty());
    }
}
