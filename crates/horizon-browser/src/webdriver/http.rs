use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::Value;
use thiserror::Error;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Error)]
pub(super) enum HttpError {
    #[error("WebDriver endpoint must be loopback: {0}")]
    NonLoopback(IpAddr),
    #[error("WebDriver HTTP I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid WebDriver HTTP response: {0}")]
    InvalidResponse(String),
    #[error("WebDriver JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("WebDriver {error}: {message}")]
    WebDriver { error: String, message: String },
}

impl HttpError {
    pub(super) fn is_unsupported_websocket_capability(&self) -> bool {
        let Self::WebDriver { error, message } = self else {
            return false;
        };
        matches!(error.as_str(), "invalid argument" | "session not created")
            && message.to_ascii_lowercase().contains("websocketurl")
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct HttpClient {
    address: SocketAddr,
}

impl HttpClient {
    pub(super) fn new(address: SocketAddr) -> Result<Self, HttpError> {
        if !address.ip().is_loopback() {
            return Err(HttpError::NonLoopback(address.ip()));
        }
        Ok(Self { address })
    }

    pub(super) fn get(&self, path: &str) -> Result<Value, HttpError> {
        self.request("GET", path, None)
    }

    pub(super) fn post(&self, path: &str, body: &Value) -> Result<Value, HttpError> {
        self.request("POST", path, Some(body))
    }

    pub(super) fn delete(&self, path: &str) -> Result<Value, HttpError> {
        self.request("DELETE", path, None)
    }

    fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, HttpError> {
        if !path.starts_with('/') || path.contains(['\r', '\n']) {
            return Err(HttpError::InvalidResponse(format!("invalid request path {path:?}")));
        }
        let body = body.map_or_else(String::new, Value::to_string);
        let mut stream = TcpStream::connect_timeout(&self.address, CONNECT_TIMEOUT)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        stream.set_nodelay(true)?;
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.address,
            body.len()
        )?;
        stream.flush()?;

        let mut response = Vec::new();
        stream
            .take(u64::try_from(MAX_RESPONSE_BYTES + 1).unwrap_or(u64::MAX))
            .read_to_end(&mut response)?;
        if response.len() > MAX_RESPONSE_BYTES {
            return Err(HttpError::InvalidResponse("response exceeded 64 MiB".to_string()));
        }
        parse_response(&response)
    }
}

fn parse_response(response: &[u8]) -> Result<Value, HttpError> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| HttpError::InvalidResponse("missing header terminator".to_string()))?;
    let headers =
        std::str::from_utf8(&response[..header_end]).map_err(|error| HttpError::InvalidResponse(error.to_string()))?;
    let mut lines = headers.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| HttpError::InvalidResponse("invalid status line".to_string()))?;
    let chunked = lines.any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding") && value.to_ascii_lowercase().contains("chunked")
        })
    });
    let raw_body = &response[header_end + 4..];
    let body = if chunked {
        decode_chunked(raw_body)?
    } else {
        raw_body.to_vec()
    };
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&body)?
    };
    let payload = value.get("value").cloned().unwrap_or_else(|| value.clone());
    if !(200..300).contains(&status) || payload.get("error").is_some() {
        let error = payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("http error")
            .to_string();
        let message = payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| value.as_str().unwrap_or_default())
            .to_string();
        return Err(HttpError::WebDriver { error, message });
    }
    Ok(value)
}

fn decode_chunked(mut body: &[u8]) -> Result<Vec<u8>, HttpError> {
    let mut decoded = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| HttpError::InvalidResponse("malformed chunk size".to_string()))?;
        let size_text =
            std::str::from_utf8(&body[..line_end]).map_err(|error| HttpError::InvalidResponse(error.to_string()))?;
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or_default().trim(), 16)
            .map_err(|error| HttpError::InvalidResponse(error.to_string()))?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Ok(decoded);
        }
        let framed_size = size
            .checked_add(2)
            .ok_or_else(|| HttpError::InvalidResponse("chunk size overflow".to_string()))?;
        if body.len() < framed_size || &body[size..framed_size] != b"\r\n" {
            return Err(HttpError::InvalidResponse("truncated chunk".to_string()));
        }
        decoded.extend_from_slice(&body[..size]);
        if decoded.len() > MAX_RESPONSE_BYTES {
            return Err(HttpError::InvalidResponse(
                "decoded response exceeded 64 MiB".to_string(),
            ));
        }
        body = &body[framed_size..];
    }
}

#[cfg(test)]
mod tests {
    use super::{HttpError, decode_chunked, parse_response};

    #[test]
    fn parses_classic_webdriver_value() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 24\r\n\r\n{\"value\":{\"ready\":true}}";
        assert_eq!(parse_response(response).expect("response")["value"]["ready"], true);
    }

    #[test]
    fn decodes_chunked_json() {
        assert_eq!(decode_chunked(b"4\r\ntest\r\n0\r\n\r\n").expect("chunks"), b"test");
        assert!(decode_chunked(b"ffffffffffffffff\r\n").is_err());
    }

    #[test]
    fn surfaces_webdriver_errors() {
        let response = b"HTTP/1.1 500 Error\r\nContent-Length: 57\r\n\r\n{\"value\":{\"error\":\"session not created\",\"message\":\"busy\"}}";
        assert!(parse_response(response).is_err());
    }

    #[test]
    fn identifies_only_websocket_capability_negotiation_errors() {
        let unsupported = HttpError::WebDriver {
            error: "invalid argument".to_string(),
            message: "Unknown capability webSocketUrl".to_string(),
        };
        let busy = HttpError::WebDriver {
            error: "session not created".to_string(),
            message: "Safari is already controlled".to_string(),
        };
        assert!(unsupported.is_unsupported_websocket_capability());
        assert!(!busy.is_unsupported_websocket_capability());
    }
}
