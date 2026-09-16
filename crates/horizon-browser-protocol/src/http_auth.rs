//! HTTP Basic and Digest credential values for live browser sessions.

use std::fmt;

use serde::{Deserialize, Serialize};

pub const MAX_HTTP_AUTH_USERNAME_BYTES: usize = 256;
pub const MAX_HTTP_AUTH_PASSWORD_BYTES: usize = 1_024;
pub const MAX_HTTP_AUTH_ORIGIN_BYTES: usize = 512;

/// Set or clear session HTTP authentication credentials.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserHttpAuthOperation {
    Set,
    Clear,
}

/// Password bytes that serialize for the private action queue but never
/// display in `Debug` output.
#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString(<redacted>)")
    }
}

/// Validate a username supplied with [`BrowserHttpAuthOperation::Set`].
///
/// # Errors
/// Returns a stable explanation when the value is empty, too long, or contains
/// control characters.
pub fn validate_http_auth_username(value: &str) -> Result<(), &'static str> {
    if value.is_empty() {
        return Err("HTTP auth username must not be empty");
    }
    if value.len() > MAX_HTTP_AUTH_USERNAME_BYTES {
        return Err("HTTP auth username is too long");
    }
    if value.chars().any(char::is_control) {
        return Err("HTTP auth username contains control characters");
    }
    Ok(())
}

/// Validate a password supplied with [`BrowserHttpAuthOperation::Set`].
///
/// Empty passwords are accepted. Control characters and oversized values are
/// not.
///
/// # Errors
/// Returns a stable explanation when the value is too long or contains control
/// characters.
pub fn validate_http_auth_password(value: &str) -> Result<(), &'static str> {
    if value.len() > MAX_HTTP_AUTH_PASSWORD_BYTES {
        return Err("HTTP auth password is too long");
    }
    if value.chars().any(char::is_control) {
        return Err("HTTP auth password contains control characters");
    }
    Ok(())
}

/// Canonical `scheme://host[:port]` origin for credential matching.
///
/// # Errors
/// Returns a stable explanation when the value is not an `http` or `https`
/// origin, contains userinfo, or carries a path, query, or fragment other than
/// an optional trailing slash.
pub fn parse_http_auth_origin(value: &str) -> Result<String, &'static str> {
    canonical_origin(value, true)
}

/// Origin of a request URL, ignoring path, query, and fragment.
#[must_use]
pub fn request_origin(url: &str) -> Option<String> {
    canonical_origin(url, false).ok()
}

fn canonical_origin(value: &str, bare_origin: bool) -> Result<String, &'static str> {
    let value = value.trim();
    if value.is_empty() {
        return Err("HTTP auth origin is missing or too long");
    }
    if bare_origin && value.len() > MAX_HTTP_AUTH_ORIGIN_BYTES {
        return Err("HTTP auth origin is missing or too long");
    }
    if value.chars().any(char::is_control) {
        return Err("HTTP auth origin contains control characters");
    }
    let Some((scheme, rest)) = value.split_once("://") else {
        return Err("HTTP auth origin must be an http or https origin");
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err("HTTP auth origin must use http or https");
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let remainder = &rest[authority_end..];
    if bare_origin && !(remainder.is_empty() || remainder == "/") {
        return Err("HTTP auth origin must not include a path, query, or fragment");
    }
    if authority.contains('@') {
        return Err("HTTP auth origin must not contain userinfo");
    }
    let (host, port) = split_host_port(authority)?;
    if host.is_empty() {
        return Err("HTTP auth origin host is missing");
    }
    let host = canonicalize_host(host)?;
    let default_port = if scheme == "http" { 80 } else { 443 };
    let origin = match port {
        Some(port) if port != default_port => format!("{scheme}://{host}:{port}"),
        _ => format!("{scheme}://{host}"),
    };
    if origin.len() > MAX_HTTP_AUTH_ORIGIN_BYTES {
        return Err("HTTP auth origin is missing or too long");
    }
    Ok(origin)
}

fn split_host_port(authority: &str) -> Result<(&str, Option<u16>), &'static str> {
    if let Some(host) = authority.strip_prefix('[') {
        let Some(end) = host.find(']') else {
            return Err("HTTP auth origin host is missing");
        };
        let address = &authority[..=end + 1];
        let rest = &authority[end + 2..];
        if rest.is_empty() {
            return Ok((address, None));
        }
        let Some(port) = rest.strip_prefix(':') else {
            return Err("HTTP auth origin host is missing");
        };
        return parse_port(port).map(|port| (address, Some(port)));
    }
    match authority.rsplit_once(':') {
        Some((host, _)) if host.contains(':') => Err("HTTP auth origin host is missing"),
        Some((host, port)) => {
            if host.is_empty() || port.is_empty() {
                return Err("HTTP auth origin port is invalid");
            }
            parse_port(port).map(|port| (host, Some(port)))
        }
        None => Ok((authority, None)),
    }
}

fn canonicalize_host(host: &str) -> Result<String, &'static str> {
    if let Some(inner) = host.strip_prefix('[').and_then(|host| host.strip_suffix(']')) {
        let address: std::net::Ipv6Addr = inner.parse().map_err(|_| "HTTP auth origin host is missing")?;
        return Ok(format!("[{address}]"));
    }
    Ok(host.to_ascii_lowercase())
}

fn parse_port(port: &str) -> Result<u16, &'static str> {
    port.parse().map_err(|_| "HTTP auth origin port is invalid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_canonicalize_and_reject_userinfo_or_paths() {
        assert_eq!(
            parse_http_auth_origin("HTTP://Example.COM:80/").expect("bare"),
            "http://example.com"
        );
        assert_eq!(
            parse_http_auth_origin("https://127.0.0.1:8443").expect("port"),
            "https://127.0.0.1:8443"
        );
        assert_eq!(
            request_origin("http://Example.COM:80/basic-auth?x=1#y").expect("request"),
            "http://example.com"
        );
        assert_eq!(
            request_origin("http://[::1]:8080/digest-auth").expect("ipv6"),
            "http://[::1]:8080"
        );
        assert_eq!(
            parse_http_auth_origin("http://[0:0:0:0:0:0:0:1]:8080").expect("canonical ipv6"),
            "http://[::1]:8080"
        );
        assert!(parse_http_auth_origin("http://[not-an-ip]").is_err());
        assert!(parse_http_auth_origin("http://user:pass@example.com").is_err());
        assert!(parse_http_auth_origin("http://example.com/basic-auth").is_err());
        assert!(parse_http_auth_origin("ftp://example.com").is_err());
        assert!(parse_http_auth_origin("http://example.com:abc").is_err());
        assert!(parse_http_auth_origin("http://example.com:").is_err());
        assert!(parse_http_auth_origin("http://example.com:99999").is_err());
        assert!(request_origin("about:blank").is_none());
        let long_path = format!("http://example.com/{}", "a".repeat(800));
        assert_eq!(request_origin(&long_path).as_deref(), Some("http://example.com"));
        let long_origin = format!("http://{}.example", "a".repeat(MAX_HTTP_AUTH_ORIGIN_BYTES));
        assert!(parse_http_auth_origin(&long_origin).is_err());
    }

    #[test]
    fn secret_debug_does_not_echo_the_password() {
        let secret = SecretString::new("smoke-pass-zephyr");
        assert_eq!(format!("{secret:?}"), "SecretString(<redacted>)");
        assert_eq!(secret.as_str(), "smoke-pass-zephyr");
        assert_eq!(serde_json::to_string(&secret).expect("json"), "\"smoke-pass-zephyr\"");
    }
}
