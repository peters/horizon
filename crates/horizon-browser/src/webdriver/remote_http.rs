//! Authenticated HTTPS transport for a remote classic `WebDriver` endpoint.
//!
//! The loopback client in `http.rs` keeps its loopback-only rule; this client
//! is the separate implementation for hosted grids. It binds one
//! authorization header to one origin, never follows redirects, bounds every
//! response, and maps failures onto [`HttpError`] with the same `Display`
//! shapes the session code already classifies.

use std::fmt;
use std::hint::black_box;
use std::io;
use std::net::IpAddr;
use std::time::Duration;

use serde_json::Value;

use super::http::{HttpError, interpret_body};
use super::transport::{ClassicTransport, validate_request_path};

/// Largest accepted response body, matching the loopback client.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
/// Bound on name resolution, connection, and request transmission; the
/// per-command read timeout is separate, as with the loopback client.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const USER_AGENT: &str = concat!("horizon/", env!("CARGO_PKG_VERSION"));

/// A ready-to-send `Authorization` header value. Built by the host from its
/// credential bindings; the only exit is the header itself, debug output is
/// redacted, and the buffer is overwritten on drop.
pub struct RemoteAuthorizationHeader {
    value: String,
}

impl RemoteAuthorizationHeader {
    /// # Errors
    /// The value must be a single printable-ASCII header value.
    pub fn new(value: String) -> Result<Self, HttpError> {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_graphic() || byte == b' ') {
            return Err(HttpError::InvalidResponse(
                "authorization header is not a printable value".into(),
            ));
        }
        Ok(Self { value })
    }

    fn as_str(&self) -> &str {
        &self.value
    }
}

impl fmt::Debug for RemoteAuthorizationHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RemoteAuthorizationHeader(<redacted>)")
    }
}

impl Drop for RemoteAuthorizationHeader {
    fn drop(&mut self) {
        let len = self.value.len();
        self.value.clear();
        self.value.extend(std::iter::repeat_n('\0', len));
        black_box(&self.value);
        self.value.clear();
    }
}

/// Classic `WebDriver` client for one remote endpoint.
pub struct RemoteHttpClient {
    agent: ureq::Agent,
    base: String,
    origin: String,
    authorization: Option<RemoteAuthorizationHeader>,
}

impl RemoteHttpClient {
    /// Bind a client to `endpoint` (scheme, host, optional port and base path).
    ///
    /// HTTPS is required unless the host is loopback; userinfo, query strings
    /// and fragments are rejected so a credential can never ride in the URL.
    ///
    /// # Errors
    /// Malformed or disallowed endpoints as [`HttpError::InvalidResponse`].
    pub fn new(endpoint: &str, authorization: Option<RemoteAuthorizationHeader>) -> Result<Self, HttpError> {
        let (origin, base, loopback) = parse_endpoint(endpoint)?;
        let config = ureq::Agent::config_builder()
            .https_only(!loopback)
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_resolve(Some(CONNECT_TIMEOUT))
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_send_request(Some(CONNECT_TIMEOUT))
            .timeout_send_body(Some(CONNECT_TIMEOUT))
            .user_agent(USER_AGENT)
            .build();
        Ok(Self {
            agent: ureq::Agent::new_with_config(config),
            base,
            origin,
            authorization,
        })
    }

    /// Scheme, host and port every request goes to.
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }

    #[cfg(test)]
    pub(super) fn config(&self) -> &ureq::config::Config {
        self.agent.config()
    }

    fn url(&self, path: &str) -> Result<String, HttpError> {
        validate_request_path(path)?;
        Ok(format!("{}{path}", self.base))
    }
}

impl fmt::Debug for RemoteHttpClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteHttpClient")
            .field("origin", &self.origin)
            .field("authorized", &self.authorization.is_some())
            .finish_non_exhaustive()
    }
}

impl ClassicTransport for RemoteHttpClient {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        read_timeout: Duration,
    ) -> Result<Value, HttpError> {
        let url = self.url(path)?;
        let timeout = read_timeout;
        let authorization = self.authorization.as_ref().map(RemoteAuthorizationHeader::as_str);
        let response = match method {
            "GET" => configure(self.agent.get(&url), timeout, authorization).call(),
            "DELETE" => configure(self.agent.delete(&url), timeout, authorization).call(),
            "POST" => {
                let empty = Value::Object(serde_json::Map::new());
                configure(self.agent.post(&url), timeout, authorization).send_json(body.unwrap_or(&empty))
            }
            other => return Err(HttpError::InvalidResponse(format!("unsupported method {other}"))),
        }
        .map_err(map_transport_error)?;
        let status = response.status().as_u16();
        let mut response = response;
        // A stalled body is still a timeout, and a truncated one is still I/O;
        // only the size bound is an invalid response.
        let bytes = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(|error| match error {
                ureq::Error::BodyExceedsLimit(_) => HttpError::InvalidResponse("response exceeded 64 MiB".into()),
                other => map_transport_error(other),
            })?;
        interpret_body(status, &bytes)
    }
}

fn configure<B>(
    builder: ureq::RequestBuilder<B>,
    timeout: Duration,
    authorization: Option<&str>,
) -> ureq::RequestBuilder<B> {
    let builder = builder
        .config()
        .timeout_recv_response(Some(timeout))
        .timeout_recv_body(Some(timeout))
        .build()
        .header("Accept", "application/json");
    match authorization {
        Some(value) => builder.header("Authorization", value),
        None => builder,
    }
}

/// Split an endpoint into `(origin, base without trailing slash, is loopback http)`.
///
/// The endpoint is parsed as a URI first so an invalid authority or port is
/// rejected here rather than on the first request.
fn parse_endpoint(endpoint: &str) -> Result<(String, String, bool), HttpError> {
    let invalid = |reason: &str| HttpError::InvalidResponse(format!("invalid remote endpoint: {reason}"));
    let endpoint = endpoint.trim();
    if endpoint.contains('#') {
        return Err(invalid("fragments are not allowed"));
    }
    let uri: ureq::http::Uri = endpoint.parse().map_err(|_| invalid("not a valid URI"))?;
    let scheme = uri.scheme_str().ok_or_else(|| invalid("missing scheme"))?;
    let authority = uri.authority().ok_or_else(|| invalid("missing host"))?;
    if authority.as_str().contains('@') {
        return Err(invalid("authority must be a host without userinfo"));
    }
    if uri.query().is_some() {
        return Err(invalid("query strings are not allowed"));
    }
    let host = authority.host().trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return Err(invalid("missing host"));
    }
    // `http::Uri` keeps a malformed port in the authority text without
    // parsing it; insist that any port suffix is a real port number.
    let port_text = authority.as_str().strip_prefix('[').map_or_else(
        || authority.as_str().split_once(':').map(|(_, port)| port),
        |rest| rest.split_once(']').and_then(|(_, tail)| tail.strip_prefix(':')),
    );
    if port_text.is_some_and(|port| port.parse::<u16>().is_err()) || authority.as_str().ends_with(':') {
        return Err(invalid("port must be a number between 0 and 65535"));
    }
    let loopback = host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    match scheme {
        "https" => {}
        "http" if loopback => {}
        _ => return Err(invalid("scheme must be https, or http for a loopback grid")),
    }
    let origin = format!("{scheme}://{}", authority.as_str()).to_ascii_lowercase();
    let base = format!("{origin}{}", uri.path().trim_end_matches('/'));
    Ok((origin, base, scheme == "http"))
}

fn map_transport_error(error: ureq::Error) -> HttpError {
    match error {
        ureq::Error::Timeout(_) => HttpError::Io(io::Error::new(io::ErrorKind::TimedOut, "request timed out")),
        ureq::Error::Io(error) => HttpError::Io(error),
        ureq::Error::TooManyRedirects | ureq::Error::RedirectFailed => {
            HttpError::InvalidResponse("endpoint redirected; redirects are not followed".into())
        }
        other => HttpError::Transport(transport_kind(&other).to_string()),
    }
}

fn transport_kind(error: &ureq::Error) -> &'static str {
    match error {
        ureq::Error::Tls(_) => "tls",
        ureq::Error::ConnectionFailed => "connection_failed",
        ureq::Error::HostNotFound => "host_not_found",
        ureq::Error::BadUri(_) => "bad_uri",
        ureq::Error::Protocol(_) => "protocol",
        ureq::Error::BodyExceedsLimit(_) => "body_exceeds_limit",
        _ => "other",
    }
}

#[cfg(test)]
mod tests;
