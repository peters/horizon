//! User tokens of a person's own GitHub App: device sign-in, the code exchange of a
//! web sign-in, renewal, and the manifest conversion that creates the app.
//!
//! A token acts as the person, limited to the app's permissions and the repositories
//! it is installed on. With **Expire user access tokens** on, an access token lasts
//! 8 hours and its refresh token about 6 months. A refresh replaces both, so each
//! holder keeps its own chain. A chain from the device sign-in renews with the client
//! ID alone; a chain from a web sign-in also needs the client secret.
//! Secrets never appear in `Debug` output or in errors.
use serde::Deserialize;
use std::{
    fmt::Write as _,
    io::Read,
    time::{Duration, SystemTime},
};
use zeroize::Zeroizing;

mod api;
pub use api::{User, valid_repository};
#[cfg(test)]
mod tests;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const RESPONSE_LIMIT: u64 = 64 * 1024;
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// GitHub's default poll interval for a device sign-in, and the step a `slow_down` adds.
const DEFAULT_INTERVAL: u64 = 5;
/// The longest poll interval accepted from GitHub.
const MAX_INTERVAL: u64 = 15 * 60;
/// The longest lifetime accepted from GitHub for a code or token, about ten years.
const MAX_LIFETIME: u64 = 10 * 365 * 24 * 60 * 60;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("GitHub could not be reached. Check the network and try again.")]
    Transport,
    #[error("GitHub sent an answer Horizon does not understand.")]
    InvalidResponse,
    #[error("The GitHub sign-in was declined.")]
    Denied,
    #[error("The GitHub sign-in expired before it was approved. Start it again.")]
    Expired,
    #[error("Device sign-in is off for this GitHub App. Turn on Enable Device Flow in the app's settings.")]
    DeviceFlowDisabled,
    #[error("Expiring tokens are off for this GitHub App. Turn on Expire user access tokens in the app's settings.")]
    NotExpiring,
    #[error("GitHub refused the app's client ID or client secret.")]
    ClientCredentials,
    #[error("GitHub no longer accepts this sign-in. Connect GitHub again.")]
    Revoked,
    #[error("GitHub refused the request ({0}).")]
    Refused(String),
    #[error("The GitHub App has more installations or repositories than Horizon reads.")]
    TooMany,
}
pub type Result<T> = std::result::Result<T, Error>;

/// A value that is wiped on drop and never printed.
pub struct Secret(Zeroizing<String>);

impl Secret {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Read straight into a wiped buffer: the string serde builds moves into the
/// `Secret` without a copy, so a later field that fails to parse still wipes it.
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

/// A started device sign-in. The person enters `user_code` at `verification_uri`
/// and approves; [`Client::poll_device`] then returns the chain.
#[derive(Debug)]
pub struct DeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    pub interval: Duration,
    pub expires_at: SystemTime,
    secret: Secret,
}

/// One answer while a device sign-in waits for the person.
#[derive(Debug)]
pub enum Poll {
    Pending,
    /// GitHub asks for a longer interval from now on.
    SlowDown(Duration),
    Granted(Chain),
}

/// An access token and the refresh token that renews it.
#[derive(Debug)]
pub struct Chain {
    pub access_token: Secret,
    pub access_expires_at: SystemTime,
    pub refresh_token: Secret,
    pub refresh_expires_at: SystemTime,
}

/// A GitHub App created from a manifest. Its private key is not kept: Horizon uses
/// only user tokens, which need no key.
#[derive(Debug)]
pub struct App {
    pub id: u64,
    pub slug: String,
    pub client_id: String,
    pub client_secret: Secret,
    pub html_url: String,
}

pub struct Client {
    agent: ureq::Agent,
    web: String,
    api: String,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    #[must_use]
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent("horizon")
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            web: "https://github.com".into(),
            api: "https://api.github.com".into(),
        }
    }

    /// A client for a fake GitHub on this machine. Only a loopback address is
    /// accepted, so a secret can never be sent to another host this way.
    /// # Errors
    /// Refuses addresses that are not loopback.
    #[doc(hidden)]
    pub fn loopback(address: std::net::SocketAddr) -> std::result::Result<Self, &'static str> {
        if !address.ip().is_loopback() {
            return Err("A test GitHub endpoint must be a loopback address");
        }
        let mut client = Self::new();
        client.web = format!("http://{address}");
        client.api = format!("http://{address}");
        Ok(client)
    }

    /// Starts a device sign-in for the app with `client_id`.
    /// # Errors
    /// Transport failures, a malformed answer, or the app's refusal.
    pub fn start_device(&self, client_id: &str) -> Result<DeviceCode> {
        self.start_device_for(client_id, None)
    }

    /// Starts a device sign-in for the OAuth app with `client_id` that asks for `scope`,
    /// such as `write:packages`. A GitHub App ignores scopes; its permissions apply.
    /// # Errors
    /// As [`Client::start_device`].
    pub fn start_device_with_scope(&self, client_id: &str, scope: &str) -> Result<DeviceCode> {
        self.start_device_for(client_id, Some(scope))
    }

    fn start_device_for(&self, client_id: &str, scope: Option<&str>) -> Result<DeviceCode> {
        #[derive(Deserialize)]
        struct Fields {
            device_code: Secret,
            user_code: String,
            verification_uri: String,
            expires_in: u64,
            #[serde(default)]
            interval: Option<u64>,
        }
        let mut fields = vec![("client_id", client_id)];
        if let Some(scope) = scope {
            fields.push(("scope", scope));
        }
        let answer = self.oauth("/login/device/code", &fields)?;
        let fields: Fields = answer.parse()?;
        let secret = fields.device_code;
        let interval = fields.interval.unwrap_or(DEFAULT_INTERVAL).max(1);
        if !valid_user_code(&fields.user_code) || !safe_https_url(&fields.verification_uri) || interval > MAX_INTERVAL {
            return Err(Error::InvalidResponse);
        }
        Ok(DeviceCode {
            user_code: fields.user_code,
            verification_uri: fields.verification_uri,
            interval: Duration::from_secs(interval),
            expires_at: later(fields.expires_in)?,
            secret,
        })
    }

    /// Asks once whether the person approved a device sign-in. Call it no more often
    /// than the code's interval. A `slow_down` answer raises `code.interval`, by
    /// GitHub's new value or else by 5 seconds, as RFC 8628 requires.
    /// # Errors
    /// [`Error::Denied`], [`Error::Expired`] or another refusal ends the sign-in.
    pub fn poll_device(&self, client_id: &str, code: &mut DeviceCode) -> Result<Poll> {
        let answer = self.post_form(
            "/login/oauth/access_token",
            &[
                ("client_id", client_id),
                ("device_code", code.secret.expose()),
                ("grant_type", DEVICE_GRANT),
            ],
        )?;
        match answer.oauth_error()? {
            Some((error, _)) if error == "authorization_pending" => Ok(Poll::Pending),
            Some((error, interval)) if error == "slow_down" => {
                let raised = code.interval.as_secs().saturating_add(DEFAULT_INTERVAL);
                let seconds = interval.map_or(raised, |given| given.max(raised));
                if seconds > MAX_INTERVAL {
                    return Err(Error::InvalidResponse);
                }
                code.interval = Duration::from_secs(seconds);
                Ok(Poll::SlowDown(code.interval))
            }
            Some((error, _)) => Err(oauth_error(&error)),
            None if answer.status == 200 => chain(&answer).map(Poll::Granted),
            None => Err(Error::Refused(format!("HTTP {}", answer.status))),
        }
    }

    /// Exchanges the code of a web sign-in that redirected to `redirect_uri`. GitHub
    /// requires the client secret here even with PKCE.
    /// # Errors
    /// Transport failures, a refused or reused code, or wrong client credentials.
    pub fn exchange_code(
        &self,
        client_id: &str,
        client_secret: &Secret,
        code: &Secret,
        redirect_uri: &str,
        verifier: &Secret,
    ) -> Result<Chain> {
        chain(&self.oauth(
            "/login/oauth/access_token",
            &[
                ("client_id", client_id),
                ("client_secret", client_secret.expose()),
                ("code", code.expose()),
                ("redirect_uri", redirect_uri),
                ("code_verifier", verifier.expose()),
            ],
        )?)
    }

    /// Renews a chain. `client_secret` is needed only for a chain from a web sign-in.
    /// The returned chain replaces the old one, whose tokens stop working.
    /// # Errors
    /// [`Error::Revoked`] when the refresh token expired, was used, or was revoked.
    pub fn refresh(&self, client_id: &str, client_secret: Option<&Secret>, refresh_token: &Secret) -> Result<Chain> {
        let mut fields = vec![
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.expose()),
        ];
        if let Some(secret) = client_secret {
            fields.push(("client_secret", secret.expose()));
        }
        chain(&self.oauth("/login/oauth/access_token", &fields)?)
    }

    /// Completes the manifest flow: the `code` GitHub sent to the manifest's redirect
    /// URL becomes the new app's identity and client secret.
    /// # Errors
    /// A malformed code, transport failures, or GitHub's refusal (an expired code).
    pub fn convert_manifest(&self, code: &Secret) -> Result<App> {
        #[derive(Deserialize)]
        struct Fields {
            id: u64,
            slug: String,
            client_id: String,
            client_secret: Secret,
            html_url: String,
        }
        let code = code.expose();
        if code.is_empty() || code.len() > 100 || !code.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(Error::InvalidResponse);
        }
        let mut url = Zeroizing::new(String::with_capacity(self.api.len() + code.len() + 32));
        let _ = write!(url, "{}/app-manifests/{code}/conversions", self.api);
        let response = self
            .agent
            .post(url.as_str())
            .header("Accept", "application/vnd.github+json")
            .send_empty();
        let answer = read(response)?;
        if answer.status != 201 {
            return Err(Error::Refused(format!("HTTP {}", answer.status)));
        }
        // Only these fields are read: the private key and webhook secret in the same
        // answer stay in the wiped response buffer.
        let fields: Fields = answer.parse()?;
        let client_secret = fields.client_secret;
        if !valid_slug(&fields.slug) || !valid_client_id(&fields.client_id) || !safe_https_url(&fields.html_url) {
            return Err(Error::InvalidResponse);
        }
        Ok(App {
            id: fields.id,
            slug: fields.slug,
            client_id: fields.client_id,
            client_secret,
            html_url: fields.html_url,
        })
    }

    /// Posts an OAuth form. GitHub answers OAuth errors with status 200 and an
    /// `error` field, which becomes a typed [`Error`].
    fn oauth(&self, path: &str, fields: &[(&str, &str)]) -> Result<Answer> {
        let answer = self.post_form(path, fields)?;
        if let Some((code, _)) = answer.oauth_error()? {
            return Err(oauth_error(&code));
        }
        if answer.status != 200 {
            return Err(Error::Refused(format!("HTTP {}", answer.status)));
        }
        Ok(answer)
    }

    fn post_form(&self, path: &str, fields: &[(&str, &str)]) -> Result<Answer> {
        let body = form(fields);
        let response = self
            .agent
            .post(format!("{}{path}", self.web))
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send(body.as_bytes());
        read(response)
    }
}

/// A response kept in a buffer that is wiped on drop. Each caller reads only the
/// fields it needs from it, so values it does not read are never copied out.
struct Answer {
    status: u16,
    body: Zeroizing<Vec<u8>>,
}

impl Answer {
    fn parse<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).map_err(|_| Error::InvalidResponse)
    }

    /// GitHub's OAuth error code, with the interval a `slow_down` may carry.
    fn oauth_error(&self) -> Result<Option<(String, Option<u64>)>> {
        #[derive(Deserialize)]
        struct Probe {
            #[serde(default)]
            error: Option<String>,
            #[serde(default)]
            interval: Option<u64>,
        }
        let probe: Probe = self.parse()?;
        Ok(probe.error.map(|error| (error, probe.interval)))
    }
}

/// Builds a form body in a buffer that is wiped on drop. The buffer gets its full
/// size first, so it never moves while it holds a secret.
fn form(fields: &[(&str, &str)]) -> Zeroizing<String> {
    // Each byte encodes to at most three bytes; each field adds `=` and `&`.
    let size = fields
        .iter()
        .map(|(key, value)| 3 * (key.len() + value.len()) + 2)
        .sum();
    let mut body = Zeroizing::new(String::with_capacity(size));
    for (index, (key, value)) in fields.iter().enumerate() {
        if index > 0 {
            body.push('&');
        }
        encode(&mut body, key);
        body.push('=');
        encode(&mut body, value);
    }
    body
}

fn encode(output: &mut String, value: &str) {
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            output.push(char::from(byte));
        } else {
            let _ = write!(output, "%{byte:02X}");
        }
    }
}

fn read(response: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Answer> {
    let mut response = response.map_err(|_| Error::Transport)?;
    let status = response.status().as_u16();
    // The whole limit is reserved first, so no part of a secret is left behind in a
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

fn oauth_error(code: &str) -> Error {
    match code {
        "access_denied" => Error::Denied,
        "expired_token" => Error::Expired,
        "device_flow_disabled" => Error::DeviceFlowDisabled,
        "incorrect_client_credentials" => Error::ClientCredentials,
        "bad_refresh_token" | "bad_verification_code" | "incorrect_device_code" => Error::Revoked,
        // Only an OAuth error code is kept, never free text that could echo input.
        code if !code.is_empty() && code.len() <= 64 && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') => {
            Error::Refused(code.to_owned())
        }
        _ => Error::InvalidResponse,
    }
}

/// A chain from a token answer. An answer without a refresh token comes from an app
/// whose tokens never expire, which Horizon does not accept.
fn chain(answer: &Answer) -> Result<Chain> {
    #[derive(Deserialize)]
    struct Fields {
        access_token: Secret,
        token_type: String,
        expires_in: Option<u64>,
        refresh_token: Option<Secret>,
        refresh_token_expires_in: Option<u64>,
    }
    let fields: Fields = answer.parse()?;
    let access_token = fields.access_token;
    let refresh_token = fields.refresh_token;
    // Callers send the token as a bearer token; any other type is a malformed answer.
    if !fields.token_type.eq_ignore_ascii_case("bearer") {
        return Err(Error::InvalidResponse);
    }
    let (Some(refresh_token), Some(access), Some(refresh)) =
        (refresh_token, fields.expires_in, fields.refresh_token_expires_in)
    else {
        return Err(Error::NotExpiring);
    };
    if [access_token.expose(), refresh_token.expose()]
        .iter()
        .any(|token| token.is_empty() || token.len() > 2048 || !token.bytes().all(|b| b.is_ascii_graphic()))
    {
        return Err(Error::InvalidResponse);
    }
    Ok(Chain {
        access_token,
        access_expires_at: later(access)?,
        refresh_token,
        refresh_expires_at: later(refresh)?,
    })
}

/// The time `seconds` from now. A lifetime beyond [`MAX_LIFETIME`] is a malformed
/// answer, never a panic.
fn later(seconds: u64) -> Result<SystemTime> {
    if seconds > MAX_LIFETIME {
        return Err(Error::InvalidResponse);
    }
    SystemTime::now()
        .checked_add(Duration::from_secs(seconds))
        .ok_or(Error::InvalidResponse)
}

fn valid_user_code(code: &str) -> bool {
    (4..=32).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-')
}

fn valid_slug(slug: &str) -> bool {
    (1..=100).contains(&slug.len()) && slug.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn valid_client_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// An `https://` URL with a host name and only visible ASCII, safe to show and open.
fn safe_https_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    url.len() <= 200
        && url.bytes().all(|b| b.is_ascii_graphic())
        && host.contains('.')
        && !host.starts_with('.')
        && !host.ends_with('.')
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// The manifest for a private app that can read and write repository contents and
/// pull requests as its user. GitHub sends the manifest flow's code to
/// `redirect_url`; user sign-ins return to `callback_url`. Device Flow and expiring
/// tokens are app settings the manifest cannot set; GitHub turns expiring tokens on
/// for new apps.
#[must_use]
pub fn manifest(name: &str, homepage: &str, redirect_url: &str, callback_url: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "url": homepage,
        "redirect_url": redirect_url,
        "callback_urls": [callback_url],
        "public": false,
        "request_oauth_on_install": false,
        "hook_attributes": {"url": homepage, "active": false},
        "default_permissions": {
            "contents": "write",
            "pull_requests": "write",
            "metadata": "read",
        },
        "default_events": [],
    })
}
