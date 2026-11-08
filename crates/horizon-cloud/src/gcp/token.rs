//! Access tokens of a Google service account, minted inside Horizon.
//!
//! A service-account JSON key carries the account's identity and an RSA private
//! key. Horizon signs a JWT with that key (`RS256`), trades the JWT at Google's
//! token endpoint for an `OAuth2` access token, and keeps the token until shortly
//! before it expires. A worker asks [`TokenCache::token`] before each request, so
//! an expired token never reaches Google.
//!
//! The private key stays in a wiped buffer. It appears in no `Debug` output and in
//! no error, and the assertion sent to Google never appears in an error either.

use serde::Deserialize;
use std::{
    io::Read,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

mod jwt;
mod key;
#[cfg(test)]
mod tests;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The largest token answer read. The answer holds four short fields.
const RESPONSE_LIMIT: u64 = 16 * 1024;

/// How long before its expiry a token is renewed. The margin covers a request in
/// flight, so a token that was valid when a request started is still valid when
/// the request arrives.
pub const RENEW_MARGIN: Duration = Duration::from_secs(300);

/// The longest token lifetime accepted from Google, one day. A longer answer is
/// refused rather than trusted to set a renewal instant far in the future.
const MAX_LIFETIME: u64 = 24 * 60 * 60;

/// The largest access token accepted, well above the 1 KB Google sends.
const MAX_TOKEN_LEN: usize = 4096;

/// The start of every exchange body. It is fixed text followed by the assertion,
/// which is base64url and dots: a form value carries both unchanged, and Google's
/// endpoint reads the grant type as written.
const EXCHANGE: &str = "grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer&assertion=";

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("The service-account key is not a JSON document.")]
    Document,
    #[error("The key file is not a service-account key.")]
    NotServiceAccount,
    #[error("The service-account key has no usable {0}.")]
    Field(&'static str),
    #[error("The service-account private key is not an RSA key in a PKCS#8 PEM block.")]
    PrivateKey,
    #[error("Google could not be reached. Check the network and try again.")]
    Transport,
    #[error("Google sent an answer Horizon does not understand.")]
    InvalidResponse,
    /// Only Google's OAuth error code is kept, never free text that could echo the
    /// assertion Horizon sent.
    #[error("Google refused the token request ({0}).")]
    Refused(String),
    #[error("The system clock is outside the range a token can be made for.")]
    Clock,
}
pub type Result<T> = std::result::Result<T, Error>;

/// A value that is wiped on drop and never printed.
#[derive(Clone, Default)]
struct Secret(Zeroizing<String>);

impl Secret {
    fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    fn expose(&self) -> &str {
        &self.0
    }

    fn into_inner(self) -> Zeroizing<String> {
        self.0
    }
}

/// Read straight into a wiped buffer, as `github::Secret` does: the string serde
/// builds moves into the `Secret` without a copy, so a later field that fails to
/// parse still wipes it.
impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::new)
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<redacted>")
    }
}

/// The identity, project and private key of a Google service account, as the JSON
/// key that the Google Cloud console downloads describes them.
pub struct ServiceAccount {
    client_email: String,
    project_id: String,
    token_uri: String,
    private_key: key::PrivateKey,
}

/// The fields of a service-account key that Horizon reads. Every field is optional
/// so that a missing one is reported by name, and a present but unusable one by the
/// same name.
#[derive(Deserialize)]
struct Document {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    client_email: String,
    #[serde(default)]
    project_id: String,
    #[serde(default)]
    token_uri: String,
    #[serde(default)]
    private_key: Secret,
}

impl ServiceAccount {
    /// Reads the JSON key that the Google Cloud console downloads for a service
    /// account.
    ///
    /// # Errors
    /// [`Error::Document`] when the text is not a JSON document,
    /// [`Error::NotServiceAccount`] when it is another kind of credential, and
    /// [`Error::Field`] or [`Error::PrivateKey`] when a value is unusable.
    pub fn parse(json: &str) -> Result<Self> {
        let document: Document = serde_json::from_str(json).map_err(|_| Error::Document)?;
        if document.kind != "service_account" {
            return Err(Error::NotServiceAccount);
        }
        if !valid_email(&document.client_email) {
            return Err(Error::Field("client_email"));
        }
        if !valid_project_id(&document.project_id) {
            return Err(Error::Field("project_id"));
        }
        if !safe_token_uri(&document.token_uri) {
            return Err(Error::Field("token_uri"));
        }
        if document.private_key.expose().trim().is_empty() {
            return Err(Error::Field("private_key"));
        }
        let private_key = key::PrivateKey::new(document.private_key.into_inner())?;
        Ok(Self {
            client_email: document.client_email,
            project_id: document.project_id,
            token_uri: document.token_uri,
            private_key,
        })
    }

    /// The account's address, which every assertion names as its issuer.
    #[must_use]
    pub fn client_email(&self) -> &str {
        &self.client_email
    }

    /// The Google Cloud project the account belongs to.
    #[must_use]
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// The endpoint that trades an assertion for a token.
    #[must_use]
    pub fn token_uri(&self) -> &str {
        &self.token_uri
    }

    fn private_key(&self) -> &key::PrivateKey {
        &self.private_key
    }
}

/// The private key never appears here, not even its first characters.
impl std::fmt::Debug for ServiceAccount {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServiceAccount")
            .field("client_email", &self.client_email)
            .field("project_id", &self.project_id)
            .field("token_uri", &self.token_uri)
            .field("private_key", &"<redacted>")
            .finish()
    }
}

/// A token that is valid now, and the instant it stops being valid.
#[derive(Clone)]
pub struct AccessToken {
    token: Secret,
    expires_at: SystemTime,
}

impl AccessToken {
    /// The value of the `Authorization: Bearer` header.
    #[must_use]
    pub fn expose(&self) -> &str {
        self.token.expose()
    }

    /// When Google stops accepting the token.
    #[must_use]
    pub fn expires_at(&self) -> SystemTime {
        self.expires_at
    }
}

impl std::fmt::Debug for AccessToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AccessToken")
            .field("token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// The clock a cache reads. A test supplies its own, so a renewal is proven without
/// waiting for the margin to pass.
pub type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// A minted token and the instant a new one is needed.
#[derive(Clone)]
struct Cached {
    token: AccessToken,
    renews_at: SystemTime,
}

/// The access token of one service account, minted on demand and renewed before it
/// expires. One cache serves one worker; every thread of the worker may share it.
pub struct TokenCache {
    account: ServiceAccount,
    now: Clock,
    agent: ureq::Agent,
    cached: Mutex<Option<Cached>>,
}

impl TokenCache {
    /// A cache for `account` that reads the system clock.
    #[must_use]
    pub fn new(account: ServiceAccount) -> Self {
        Self::with_clock(account, Arc::new(SystemTime::now))
    }

    /// A cache that reads `now` instead of the system clock.
    #[must_use]
    pub fn with_clock(account: ServiceAccount, now: Clock) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent("horizon")
            .build();
        Self {
            account,
            now,
            agent: ureq::Agent::new_with_config(config),
            cached: Mutex::new(None),
        }
    }

    /// The account whose token this cache mints.
    #[must_use]
    pub fn account(&self) -> &ServiceAccount {
        &self.account
    }

    /// The token to send with the next request: the cached one while it has more
    /// than the renewal margin left, otherwise a freshly minted one. Two threads
    /// that renew at the same instant each exchange once; both answers are valid
    /// tokens and the last one is kept.
    ///
    /// # Errors
    /// Transport failures, a refused assertion, or an answer Horizon does not
    /// understand. A failure leaves the cached token untouched, so a later call
    /// tries the exchange again.
    pub fn token(&self) -> Result<AccessToken> {
        let now = (self.now)();
        if let Some(cached) = self.cached.lock().unwrap_or_else(PoisonError::into_inner).as_ref()
            && cached.renews_at > now
        {
            return Ok(cached.token.clone());
        }
        let minted = self.mint(now)?;
        *self.cached.lock().unwrap_or_else(PoisonError::into_inner) = Some(minted.clone());
        Ok(minted.token)
    }

    /// One exchange with Google: sign an assertion for `issued` and trade it for a
    /// token. The cache lock is not held here, so a slow answer blocks no other
    /// caller that already has a usable token.
    fn mint(&self, issued: SystemTime) -> Result<Cached> {
        let assertion = jwt::assertion(&self.account, unix(issued)?)?;
        let body = exchange_body(&assertion);
        let response = self
            .agent
            .post(self.account.token_uri())
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send(body.as_str());
        let answer = read(response)?;
        let fields: TokenAnswer = match answer.parse() {
            Ok(fields) => fields,
            Err(error) if (200..=299).contains(&answer.status) => return Err(error),
            // A proxy or a gateway refuses without a JSON body, so there is no code.
            Err(_) => return Err(refusal(answer.status)),
        };
        if let Some(code) = fields.error {
            return Err(refused(&code));
        }
        if !(200..=299).contains(&answer.status) {
            return Err(refusal(answer.status));
        }
        minted(fields, issued)
    }
}

impl std::fmt::Debug for TokenCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let has_token = self.cached.lock().unwrap_or_else(PoisonError::into_inner).is_some();
        formatter
            .debug_struct("TokenCache")
            .field("account", &self.account)
            .field("has_token", &has_token)
            .finish_non_exhaustive()
    }
}

/// The four fields Google's token endpoint answers with, plus its error code.
#[derive(Deserialize)]
struct TokenAnswer {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    access_token: Option<Secret>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    token_type: Option<String>,
}

/// The body of an exchange: fixed text, then the assertion.
fn exchange_body(assertion: &str) -> Zeroizing<String> {
    let mut body = Zeroizing::new(String::with_capacity(EXCHANGE.len() + assertion.len()));
    body.push_str(EXCHANGE);
    body.push_str(assertion);
    body
}

/// The token and its deadline, from a token answer.
fn minted(fields: TokenAnswer, issued: SystemTime) -> Result<Cached> {
    let Some(token) = fields.access_token else {
        return Err(Error::InvalidResponse);
    };
    let Some(seconds) = fields.expires_in else {
        return Err(Error::InvalidResponse);
    };
    if !usable_token(token.expose()) {
        return Err(Error::InvalidResponse);
    }
    if fields
        .token_type
        .as_deref()
        .is_some_and(|kind| !kind.eq_ignore_ascii_case("bearer"))
    {
        return Err(Error::InvalidResponse);
    }
    if seconds == 0 || seconds > MAX_LIFETIME {
        return Err(Error::InvalidResponse);
    }
    let lifetime = Duration::from_secs(seconds);
    Ok(Cached {
        token: AccessToken {
            token,
            expires_at: issued.checked_add(lifetime).ok_or(Error::Clock)?,
        },
        renews_at: issued
            .checked_add(lifetime.saturating_sub(margin(lifetime)))
            .ok_or(Error::Clock)?,
    })
}

/// How long before the expiry a token is renewed: [`RENEW_MARGIN`], or half the
/// lifetime of a token that is shorter than the margin allows, so that such a token
/// is still used once instead of being renewed at every call.
fn margin(lifetime: Duration) -> Duration {
    RENEW_MARGIN.min(lifetime / 2)
}

/// `at` as seconds since the epoch, which is what an assertion carries.
fn unix(at: SystemTime) -> Result<i64> {
    let seconds = at.duration_since(UNIX_EPOCH).map_err(|_| Error::Clock)?.as_secs();
    i64::try_from(seconds).map_err(|_| Error::Clock)
}

fn usable_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= MAX_TOKEN_LEN && token.bytes().all(|b| b.is_ascii_graphic())
}

/// A refusal that carries no error code keeps only the HTTP status.
fn refusal(status: u16) -> Error {
    Error::Refused(format!("HTTP {status}"))
}

/// Google's OAuth error code, kept only when it is a code and not free text.
fn refused(code: &str) -> Error {
    if (1..=64).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
        Error::Refused(code.to_owned())
    } else {
        Error::InvalidResponse
    }
}

fn valid_email(email: &str) -> bool {
    email.len() <= 320 && email.contains('@') && email.bytes().all(|b| b.is_ascii_graphic())
}

/// A Google Cloud project ID: lowercase letters, digits, hyphens and dots.
fn valid_project_id(project: &str) -> bool {
    (1..=100).contains(&project.len())
        && project.bytes().all(|b| {
            b.is_ascii_lowercase()
                || b.is_ascii_uppercase()
                || b.is_ascii_digit()
                || b == b'-'
                || b == b'.'
                || b == b':'
        })
}

/// An `https://` endpoint, or an `http://` endpoint on this machine, which only a
/// test uses: an assertion must never travel in clear text over a network. The
/// endpoint comes from the key document, so that document decides where its own
/// assertion goes; only clear text is refused here.
fn safe_token_uri(uri: &str) -> bool {
    if uri.len() > 200 || !uri.bytes().all(|b| b.is_ascii_graphic()) {
        return false;
    }
    if let Some(rest) = uri.strip_prefix("https://") {
        let host = authority(rest);
        return host.contains('.') && !host.starts_with('.') && !host.ends_with('.');
    }
    uri.strip_prefix("http://")
        .is_some_and(|rest| loopback(authority(rest)))
}

/// The host and port part of a URL that has no scheme.
fn authority(rest: &str) -> &str {
    rest.split(['/', '?', '#']).next().unwrap_or_default()
}

fn loopback(authority: &str) -> bool {
    match authority.split_once(':') {
        Some((host, port)) => !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) && local_host(host),
        None => local_host(authority),
    }
}

fn local_host(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|address| address.is_loopback())
}

/// One answer, with its body in a buffer that is wiped on drop.
struct Answer {
    status: u16,
    body: Zeroizing<Vec<u8>>,
}

impl Answer {
    fn parse<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).map_err(|_| Error::InvalidResponse)
    }
}

fn read(response: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Answer> {
    let mut response = response.map_err(|_| Error::Transport)?;
    let status = response.status().as_u16();
    // The whole limit is reserved first, so no part of an answer is left behind in a
    // buffer that a growing read freed.
    let mut body = Zeroizing::new(Vec::with_capacity(
        usize::try_from(RESPONSE_LIMIT + 1).unwrap_or(usize::MAX),
    ));
    response
        .body_mut()
        .as_reader()
        .take(RESPONSE_LIMIT + 1)
        .read_to_end(&mut body)
        .map_err(|_| Error::Transport)?;
    if body.len() as u64 > RESPONSE_LIMIT {
        return Err(Error::InvalidResponse);
    }
    Ok(Answer { status, body })
}
