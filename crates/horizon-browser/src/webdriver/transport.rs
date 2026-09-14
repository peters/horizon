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
pub(super) fn validate_request_path(path: &str) -> Result<(), HttpError> {
    if !path.starts_with('/')
        || path.contains(['\r', '\n', '?', '#', ' ', '\\'])
        || path.contains("//")
        || path.split('/').any(|segment| segment == "..")
    {
        return Err(HttpError::InvalidResponse(format!("invalid request path {path:?}")));
    }
    Ok(())
}
