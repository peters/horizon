use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use thiserror::Error;
use tungstenite::Message;
use tungstenite::client::IntoClientRequest;
use tungstenite::protocol::WebSocket;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_millis(16);

#[derive(Debug, Error)]
pub(crate) enum JsonWsError {
    #[error("websocket I/O: {0}")]
    Io(#[from] io::Error),
    #[error("websocket: {0}")]
    WebSocket(#[from] tungstenite::Error),
    #[error("websocket handshake: {0}")]
    Handshake(String),
    #[error("invalid websocket URL: {0}")]
    InvalidUrl(String),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("protocol command {method} failed: {message}")]
    Protocol { method: String, message: String },
    #[error("timed out waiting for {method}")]
    Timeout { method: String },
    #[error("websocket closed while waiting for {method}")]
    Closed { method: String },
}

pub(crate) struct JsonCallOutcome {
    pub(crate) result: Result<Value, JsonWsError>,
    pub(crate) events: Vec<Value>,
}

pub(crate) struct JsonWsLink {
    ws: WebSocket<TcpStream>,
    next_id: u64,
}

impl JsonWsLink {
    pub(crate) fn connect(url: &str) -> Result<Self, JsonWsError> {
        let request = url.into_client_request()?;
        let addresses = loopback_addresses(url)?;
        let mut last_error = None;
        let mut stream = None;
        for address in addresses {
            match TcpStream::connect_timeout(&address, HANDSHAKE_TIMEOUT) {
                Ok(candidate) => {
                    stream = Some(candidate);
                    break;
                }
                Err(error) => last_error = Some(error),
            }
        }
        let tcp = stream.ok_or_else(|| {
            last_error.unwrap_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, "no loopback endpoint"))
        })?;
        tcp.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        tcp.set_write_timeout(Some(WRITE_TIMEOUT))?;
        tcp.set_nodelay(true)?;
        let (mut ws, _) =
            tungstenite::client::client(request, tcp).map_err(|error| JsonWsError::Handshake(error.to_string()))?;
        ws.get_mut().set_read_timeout(Some(READ_TIMEOUT))?;
        Ok(Self { ws, next_id: 1 })
    }

    pub(crate) fn call(&mut self, timeout: Duration, method: &str, params: &Value) -> JsonCallOutcome {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        if let Err(error) = self.send(&json!({ "id": id, "method": method, "params": params })) {
            return JsonCallOutcome {
                result: Err(error),
                events: Vec::new(),
            };
        }
        let deadline = Instant::now() + timeout;
        let mut events = Vec::new();
        loop {
            if Instant::now() >= deadline {
                return JsonCallOutcome {
                    result: Err(JsonWsError::Timeout {
                        method: method.to_string(),
                    }),
                    events,
                };
            }
            match self.read_value() {
                Ok(Some(value)) if value.get("id").and_then(Value::as_u64) == Some(id) => {
                    let result = response_result(method, value);
                    return JsonCallOutcome { result, events };
                }
                Ok(Some(value)) => events.push(value),
                Ok(None) => {}
                Err(error) => {
                    return JsonCallOutcome {
                        result: Err(error),
                        events,
                    };
                }
            }
        }
    }

    /// Send a command without waiting for its reply. The reply arrives
    /// through [`Self::drain`] carrying the returned `id`, so callers that
    /// must not block the driver loop can route it themselves.
    pub(crate) fn send_request(&mut self, method: &str, params: &Value) -> Result<u64, JsonWsError> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.send(&json!({ "id": id, "method": method, "params": params }))?;
        Ok(id)
    }

    pub(crate) fn drain(&mut self, max_messages: usize) -> Result<Vec<Value>, JsonWsError> {
        let mut values = Vec::new();
        for _ in 0..max_messages {
            let Some(value) = self.read_value()? else {
                break;
            };
            values.push(value);
        }
        Ok(values)
    }

    fn send(&mut self, value: &Value) -> Result<(), JsonWsError> {
        self.ws.send(Message::Text(value.to_string().into()))?;
        Ok(())
    }

    fn read_value(&mut self) -> Result<Option<Value>, JsonWsError> {
        loop {
            match self.ws.read() {
                Ok(Message::Text(text)) => return Ok(Some(serde_json::from_str(text.as_str())?)),
                Ok(Message::Close(_)) => {
                    return Err(JsonWsError::Closed {
                        method: "<event pump>".to_string(),
                    });
                }
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_)) => return Ok(None),
                Err(tungstenite::Error::Io(error)) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(tungstenite::Error::Io(error))
                    if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

fn response_result(method: &str, mut value: Value) -> Result<Value, JsonWsError> {
    let is_error = value.get("type").and_then(Value::as_str) == Some("error") || value.get("error").is_some();
    if is_error {
        let error = value.get("error").and_then(Value::as_str).unwrap_or("protocol error");
        let message = value.get("message").and_then(Value::as_str).unwrap_or_default();
        return Err(JsonWsError::Protocol {
            method: method.to_string(),
            message: format!("{error}: {message}"),
        });
    }
    Ok(value.get_mut("result").map_or(Value::Null, Value::take))
}

fn loopback_addresses(url: &str) -> Result<Vec<SocketAddr>, JsonWsError> {
    let rest = url
        .strip_prefix("ws://")
        .ok_or_else(|| JsonWsError::InvalidUrl(format!("only ws:// is supported: {url}")))?;
    let authority = rest
        .split('/')
        .next()
        .filter(|authority| !authority.is_empty())
        .ok_or_else(|| JsonWsError::InvalidUrl(format!("missing authority: {url}")))?;
    let addresses = authority
        .to_socket_addrs()
        .map_err(|error| JsonWsError::InvalidUrl(format!("{authority}: {error}")))?
        .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.iter().any(|address| !address.ip().is_loopback()) {
        return Err(JsonWsError::InvalidUrl(format!(
            "non-loopback endpoint is not allowed: {authority}"
        )));
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::loopback_addresses;

    #[test]
    fn websocket_endpoint_must_be_loopback() {
        assert!(loopback_addresses("ws://127.0.0.1:4444/session").is_ok());
        assert!(loopback_addresses("ws://localhost:4444/session").is_ok());
        assert!(loopback_addresses("wss://127.0.0.1:4444/session").is_err());
        assert!(loopback_addresses("ws://192.0.2.10:4444/session").is_err());
    }
}
