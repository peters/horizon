//! The classic `WebDriver` HTTP contract the session code speaks, independent
//! of whether the driver runs on this machine or behind an authenticated
//! remote endpoint.

use std::time::Duration;

use serde_json::Value;

use super::http::{HttpClient, HttpError};

/// Default read timeout for one classic command.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// One HTTP round trip per classic `WebDriver` command. Paths are absolute
/// (`/session/...`) and are resolved against the transport's own base, so a
/// caller can never redirect a request to another origin.
pub trait ClassicTransport: Send + Sync {
    /// # Errors
    /// Transport, decoding, or `WebDriver`-level failures as [`HttpError`].
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        read_timeout: Duration,
    ) -> Result<Value, HttpError>;

    /// # Errors
    /// See [`ClassicTransport::request`].
    fn get(&self, path: &str) -> Result<Value, HttpError> {
        self.request("GET", path, None, DEFAULT_READ_TIMEOUT)
    }

    /// # Errors
    /// See [`ClassicTransport::request`].
    fn get_with_read_timeout(&self, path: &str, read_timeout: Duration) -> Result<Value, HttpError> {
        self.request("GET", path, None, read_timeout)
    }

    /// # Errors
    /// See [`ClassicTransport::request`].
    fn post(&self, path: &str, body: &Value) -> Result<Value, HttpError> {
        self.request("POST", path, Some(body), DEFAULT_READ_TIMEOUT)
    }

    /// # Errors
    /// See [`ClassicTransport::request`].
    fn post_with_read_timeout(&self, path: &str, body: &Value, read_timeout: Duration) -> Result<Value, HttpError> {
        self.request("POST", path, Some(body), read_timeout)
    }

    /// # Errors
    /// See [`ClassicTransport::request`].
    fn delete(&self, path: &str) -> Result<Value, HttpError> {
        self.request("DELETE", path, None, DEFAULT_READ_TIMEOUT)
    }
}

impl ClassicTransport for HttpClient {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        read_timeout: Duration,
    ) -> Result<Value, HttpError> {
        HttpClient::request(self, method, path, body, read_timeout)
    }
}

/// Reject anything that is not a plain absolute request path. Session ids are
/// validated upstream; this keeps a malformed suffix from ever reaching a wire.
/// Percent-encoding is accepted only in the canonical form
/// [`encode_path_segment`] produces, so no encoded byte can change the route
/// shape once a proxy decodes it.
pub(super) fn validate_request_path(path: &str) -> Result<(), HttpError> {
    if !path.starts_with('/')
        || !path.chars().all(|c| c.is_ascii_graphic())
        || path.contains(['?', '#', '\\'])
        || path.contains("//")
        || path.split('/').any(|segment| segment == "..")
        || !canonically_encoded(path)
    {
        return Err(HttpError::InvalidResponse(format!("invalid request path {path:?}")));
    }
    Ok(())
}

/// Percent-encode an opaque value (an element reference, for instance) as
/// exactly one route segment: unreserved bytes stay as they are and every
/// other byte becomes uppercase `%HH`. A `/` is encoded too, but
/// [`validate_request_path`] refuses the result, since a proxy that decodes
/// it would change the route; callers reject such values up front.
#[must_use]
pub(crate) fn encode_path_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if is_unreserved(byte) {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Every `%` starts an uppercase `%HH` escape of a byte that canonical
/// encoding would actually escape: never an unreserved byte (a proxy may
/// decode `%2E%2E` into `..`) and never `/` (which would split the segment).
fn canonically_encoded(path: &str) -> bool {
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(hex) = bytes.get(index + 1..index + 3) else {
                return false;
            };
            if !hex
                .iter()
                .all(|digit| digit.is_ascii_digit() || (b'A'..=b'F').contains(digit))
            {
                return false;
            }
            let Ok(text) = std::str::from_utf8(hex) else {
                return false;
            };
            let Ok(decoded) = u8::from_str_radix(text, 16) else {
                return false;
            };
            if is_unreserved(decoded) || decoded == b'/' {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    true
}

const fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

#[cfg(test)]
mod tests {
    use super::{encode_path_segment, validate_request_path};

    #[test]
    fn opaque_segments_encode_canonically_and_validate() {
        assert_eq!(encode_path_segment("node-6066-11e4.a_b~c"), "node-6066-11e4.a_b~c");
        assert_eq!(encode_path_segment("a/b c%d?e#f"), "a%2Fb%20c%25d%3Fe%23f");
        assert_eq!(encode_path_segment("\u{e9}l\u{e9}ment"), "%C3%A9l%C3%A9ment");
        assert_eq!(encode_path_segment("../x"), "..%2Fx");
        for id in ["a b c%d?e#f", "\u{e9}l\u{e9}ment", "...", "{\"json\":1}"] {
            let path = format!("/session/s1/element/{}/clear", encode_path_segment(id));
            assert!(validate_request_path(&path).is_ok(), "{path}");
            assert_eq!(path.split('/').count(), 6, "{path} keeps one segment per component");
        }
    }

    #[test]
    fn only_canonical_escapes_are_accepted() {
        for path in [
            "/session/%2e%2e/status",
            "/session/%2E%2E/status",
            "/session/s1/element/a%2Fb/clear",
            "/session/s1/element/a%2fb/clear",
            "/session/s1/element/a%41/clear",
            "/session/s1/element/a%/clear",
            "/session/s1/element/a%2/clear",
            "/session/s1/element/a%zz/clear",
            "/session/s1/element/a%2g/clear",
        ] {
            assert!(validate_request_path(path).is_err(), "{path}");
        }
        assert!(validate_request_path("/session/s1/element/a%20b%C3%A9/clear").is_ok());
    }
}
