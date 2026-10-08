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

#[cfg(test)]
mod tests;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const RESPONSE_LIMIT: u64 = 64 * 1024;
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// GitHub's default poll interval for a device sign-in.
const DEFAULT_INTERVAL: u64 = 5;

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
        #[derive(Deserialize)]
        struct Answer {
            device_code: String,
            user_code: String,
            verification_uri: String,
            expires_in: u64,
            #[serde(default)]
            interval: Option<u64>,
        }
        let value = self.oauth("/login/device/code", &[("client_id", client_id)])?;
        let answer: Answer = serde_json::from_value(value).map_err(|_| Error::InvalidResponse)?;
        let device_code = Secret::new(answer.device_code);
        if !valid_user_code(&answer.user_code) || !answer.verification_uri.starts_with("https://") {
            return Err(Error::InvalidResponse);
        }
        Ok(DeviceCode {
            user_code: answer.user_code,
            verification_uri: answer.verification_uri,
            interval: Duration::from_secs(answer.interval.unwrap_or(DEFAULT_INTERVAL).max(1)),
            expires_at: SystemTime::now() + Duration::from_secs(answer.expires_in),
            secret: device_code,
        })
    }

    /// Asks once whether the person approved a device sign-in. Call it no more often
    /// than the code's interval.
    /// # Errors
    /// [`Error::Denied`], [`Error::Expired`] or another refusal ends the sign-in.
    pub fn poll_device(&self, client_id: &str, code: &DeviceCode) -> Result<Poll> {
        let (status, value) = self.post_form(
            "/login/oauth/access_token",
            &[
                ("client_id", client_id),
                ("device_code", code.secret.expose()),
                ("grant_type", DEVICE_GRANT),
            ],
        )?;
        match value.get("error").and_then(serde_json::Value::as_str) {
            Some("authorization_pending") => Ok(Poll::Pending),
            Some("slow_down") => {
                // GitHub sends the new interval; without one, add its usual 5 seconds.
                let seconds = value.get("interval").and_then(serde_json::Value::as_u64);
                let interval = seconds.map_or(
                    code.interval + Duration::from_secs(DEFAULT_INTERVAL),
                    Duration::from_secs,
                );
                Ok(Poll::SlowDown(interval))
            }
            Some(error) => Err(oauth_error(error)),
            None if status == 200 => chain(value).map(Poll::Granted),
            None => Err(Error::Refused(format!("HTTP {status}"))),
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
        code: &str,
        redirect_uri: &str,
        verifier: &Secret,
    ) -> Result<Chain> {
        chain(self.oauth(
            "/login/oauth/access_token",
            &[
                ("client_id", client_id),
                ("client_secret", client_secret.expose()),
                ("code", code),
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
        chain(self.oauth("/login/oauth/access_token", &fields)?)
    }

    /// Completes the manifest flow: the `code` GitHub sent to the manifest's redirect
    /// URL becomes the new app's identity and client secret.
    /// # Errors
    /// A malformed code, transport failures, or GitHub's refusal (an expired code).
    pub fn convert_manifest(&self, code: &str) -> Result<App> {
        #[derive(Deserialize)]
        struct Answer {
            id: u64,
            slug: String,
            client_id: String,
            client_secret: String,
            html_url: String,
        }
        if code.is_empty() || code.len() > 100 || !code.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(Error::InvalidResponse);
        }
        let url = format!("{}/app-manifests/{code}/conversions", self.api);
        let response = self
            .agent
            .post(&url)
            .header("Accept", "application/vnd.github+json")
            .send_empty();
        let (status, value) = read(response)?;
        if status != 201 {
            return Err(Error::Refused(format!("HTTP {status}")));
        }
        let answer: Answer = serde_json::from_value(value).map_err(|_| Error::InvalidResponse)?;
        let client_secret = Secret::new(answer.client_secret);
        Ok(App {
            id: answer.id,
            slug: answer.slug,
            client_id: answer.client_id,
            client_secret,
            html_url: answer.html_url,
        })
    }

    /// Posts an OAuth form. GitHub answers OAuth errors with status 200 and an
    /// `error` field, which becomes a typed [`Error`].
    fn oauth(&self, path: &str, fields: &[(&str, &str)]) -> Result<serde_json::Value> {
        let (status, value) = self.post_form(path, fields)?;
        if let Some(code) = value.get("error").and_then(serde_json::Value::as_str) {
            return Err(oauth_error(code));
        }
        if status != 200 {
            return Err(Error::Refused(format!("HTTP {status}")));
        }
        Ok(value)
    }
}

impl Client {
    fn post_form(&self, path: &str, fields: &[(&str, &str)]) -> Result<(u16, serde_json::Value)> {
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

/// Builds a form body in a buffer that is wiped on drop.
fn form(fields: &[(&str, &str)]) -> Zeroizing<String> {
    let mut body = Zeroizing::new(String::new());
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

fn read(
    response: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<(u16, serde_json::Value)> {
    let mut response = response.map_err(|_| Error::Transport)?;
    let status = response.status().as_u16();
    let mut body = Zeroizing::new(Vec::new());
    response
        .body_mut()
        .as_reader()
        .take(RESPONSE_LIMIT + 1)
        .read_to_end(&mut body)
        .map_err(|_| Error::Transport)?;
    if body.len() as u64 > RESPONSE_LIMIT {
        return Err(Error::InvalidResponse);
    }
    let value = serde_json::from_slice(&body).map_err(|_| Error::InvalidResponse)?;
    Ok((status, value))
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
fn chain(value: serde_json::Value) -> Result<Chain> {
    #[derive(Deserialize)]
    struct Answer {
        access_token: String,
        expires_in: Option<u64>,
        refresh_token: Option<String>,
        refresh_token_expires_in: Option<u64>,
    }
    let answer: Answer = serde_json::from_value(value).map_err(|_| Error::InvalidResponse)?;
    let access_token = Secret::new(answer.access_token);
    let refresh_token = answer.refresh_token.map(Secret::new);
    let (Some(refresh_token), Some(access), Some(refresh)) =
        (refresh_token, answer.expires_in, answer.refresh_token_expires_in)
    else {
        return Err(Error::NotExpiring);
    };
    if [access_token.expose(), refresh_token.expose()]
        .iter()
        .any(|token| token.is_empty() || token.len() > 2048 || !token.bytes().all(|b| b.is_ascii_graphic()))
    {
        return Err(Error::InvalidResponse);
    }
    let now = SystemTime::now();
    Ok(Chain {
        access_token,
        access_expires_at: now + Duration::from_secs(access),
        refresh_token,
        refresh_expires_at: now + Duration::from_secs(refresh),
    })
}

fn valid_user_code(code: &str) -> bool {
    (4..=32).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The manifest for a private app that can read and write repository contents and
/// pull requests as its user. Device Flow and expiring tokens are app settings the
/// manifest cannot set; GitHub turns expiring tokens on for new apps.
#[must_use]
pub fn manifest(name: &str, homepage: &str, redirect_url: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "url": homepage,
        "redirect_url": redirect_url,
        "callback_urls": [redirect_url],
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
