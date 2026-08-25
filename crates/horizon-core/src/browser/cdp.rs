//! `CDP` (`Chrome` `DevTools` Protocol) transport over a plain websocket.
//!
//! Horizon talks CDP to a local headless Chrome. Two topologies are in play:
//!
//! - **Browser-level** (the panel driver): one connection to
//!   `/devtools/browser/<id>` with `Target.setAutoAttach(flatten)`. Session
//!   ids are strings, and — unlike the docs for direct page connections —
//!   flattened *events* carry `sessionId` at the top level of the message
//!   while commands and `Page.screencastFrameAck` need it both at the top
//!   level (session scoping) and, for the ack, inside `params` as well.
//! - **Direct page** (an external agent): one connection to
//!   `/devtools/page/<id>`. That connection transparently follows
//!   cross-document navigations, which is exactly what an agent wants.
//!
//! Both are plain `ws://` to 127.0.0.1, so no TLS is involved.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use thiserror::Error;
use tungstenite::Message;
use tungstenite::client::IntoClientRequest;
use tungstenite::protocol::WebSocket;

/// Default per-read timeout. This is also the upper bound before the driver
/// checks UI commands again on an idle page, so keep it within one frame.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_millis(16);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Error, Debug)]
pub enum CdpError {
    #[error("websocket i/o: {0}")]
    Io(Box<std::io::Error>),
    #[error("websocket: {0}")]
    Ws(Box<tungstenite::Error>),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("websocket handshake: {0}")]
    Handshake(String),
    #[error("cdp error {code}: {message}")]
    Response { code: i64, message: String },
    #[error("invalid ws url: {0}")]
    InvalidUrl(String),
    #[error("timed out waiting for CDP response to {method}")]
    Timeout { method: String },
    #[error("websocket connection closed while waiting for {method}")]
    Closed { method: String },
    #[error("cancelled while waiting for CDP response to {method}")]
    Cancelled { method: String },
    #[error("no active page session for {method}")]
    NoPageSession { method: String },
}

impl From<std::io::Error> for CdpError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(Box::new(error))
    }
}

impl From<tungstenite::Error> for CdpError {
    fn from(error: tungstenite::Error) -> Self {
        Self::Ws(Box::new(error))
    }
}

pub type Result<T> = std::result::Result<T, CdpError>;

/// Result of a synchronous CDP roundtrip plus every unrelated message read
/// while waiting. `drained` remains available even when the call fails, so
/// callers can still acknowledge frames and advance independent state.
#[derive(Debug)]
pub struct CdpCallOutcome {
    pub result: Result<Value>,
    pub drained: Vec<CdpMsg>,
}

/// One inbound CDP message, decoded.
#[derive(Debug)]
pub enum CdpMsg {
    /// A response to a request we sent (matched by `id`).
    Response {
        id: u64,
        result: Option<Value>,
        error: Option<CdpErrorInfo>,
        session_id: Option<String>,
    },
    /// An unsolicited event.
    Event {
        method: String,
        params: Value,
        /// Top-level `sessionId` (flattened sessions). Direct page
        /// connections carry this inside `params` instead; callers that need
        /// both should use [`CdpEvent::session_id`].
        session_id: Option<String>,
    },
}

#[derive(Clone, Debug, Error, PartialEq)]
#[error("cdp error {code}: {message}")]
pub struct CdpErrorInfo {
    pub code: i64,
    pub message: String,
}

impl From<CdpErrorInfo> for CdpError {
    fn from(info: CdpErrorInfo) -> Self {
        Self::Response {
            code: info.code,
            message: info.message,
        }
    }
}

/// A CDP event (decoded). `Copy` because every field borrows.
#[derive(Clone, Copy, Debug)]
pub struct CdpEvent<'a> {
    pub method: &'a str,
    pub params: &'a Value,
    pub session_id: Option<&'a str>,
}

impl CdpMsg {
    #[must_use]
    pub fn event(&self) -> Option<CdpEvent<'_>> {
        match self {
            Self::Event {
                method,
                params,
                session_id,
            } => Some(CdpEvent {
                method,
                params,
                session_id: session_id.as_deref(),
            }),
            Self::Response { .. } => None,
        }
    }

    #[must_use]
    pub fn response_id(&self) -> Option<u64> {
        match self {
            Self::Response { id, .. } => Some(*id),
            Self::Event { .. } => None,
        }
    }
}

/// A live CDP connection. Single-threaded by design: the owning thread
/// (browser driver thread, or an external agent) alternates `read_one` with
/// `send_*` calls, mirroring how terminal panels pump their event loops.
pub struct CdpLink {
    ws: WebSocket<TcpStream>,
    next_id: u64,
}

impl CdpLink {
    /// Connect to a `ws://host:port/path` CDP endpoint.
    ///
    /// # Errors
    /// Fails when the URL, TCP connect, or the websocket handshake fails.
    pub fn connect(ws_url: &str) -> Result<Self> {
        Self::connect_with_timeout(ws_url, DEFAULT_READ_TIMEOUT)
    }

    /// Like [`CdpLink::connect`] with an explicit per-read timeout.
    ///
    /// # Errors
    /// Fails when the URL, TCP connect, or the websocket handshake fails.
    pub fn connect_with_timeout(ws_url: &str, read_timeout: Duration) -> Result<Self> {
        let request = ws_url.into_client_request()?;
        let host_port = host_port_from_ws_url(ws_url)?;
        let tcp = TcpStream::connect(host_port)?;
        // The interactive polling timeout is intentionally short, but the
        // HTTP Upgrade handshake and every later CDP write need a normal,
        // bounded network-operation budget.
        tcp.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        tcp.set_write_timeout(Some(WRITE_TIMEOUT))?;
        tcp.set_nodelay(true)?;
        let (mut ws, _response) =
            tungstenite::client::client(request, tcp).map_err(|e| CdpError::Handshake(e.to_string()))?;
        ws.get_mut().set_read_timeout(Some(read_timeout))?;
        Ok(Self { ws, next_id: 1 })
    }

    /// Read at most one message, waiting up to the configured read timeout.
    /// Returns `Ok(None)` on timeout so callers can keep doing other work.
    ///
    /// # Errors
    /// Fails on websocket errors (a closed connection is an error, not `None`).
    pub fn read_one(&mut self) -> Result<Option<CdpMsg>> {
        loop {
            match self.ws.read() {
                Ok(Message::Text(text)) => {
                    let value: Value = serde_json::from_str(text.as_str())?;
                    return Ok(Some(Self::decode_message(&value)));
                }
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_)) => {
                    // tungstenite answers pings; binary/raw frames are not expected.
                    return Ok(None);
                }
                Ok(Message::Close(_)) => {
                    return Err(CdpError::Closed {
                        method: "<close>".to_string(),
                    });
                }
                // A signal can interrupt the blocking read (EINTR); the
                // deadline is re-armed by the OS, so just try again.
                Err(tungstenite::Error::Io(error)) if error.kind() == std::io::ErrorKind::Interrupted => {}
                // The read timeout surfaces as WouldBlock on Unix
                // and TimedOut on Windows — both mean "idle, keep polling".
                Err(tungstenite::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.kind() == std::io::ErrorKind::TimedOut =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn decode_message(value: &Value) -> CdpMsg {
        let is_event = value.get("id").is_none();
        if is_event {
            let method = value
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            // Flattened sessions carry sessionId at the top level; direct
            // page sessions carry it inside params.
            let session_id = value
                .get("sessionId")
                .or_else(|| value.get("params").and_then(|p| p.get("sessionId")))
                .and_then(Value::as_str)
                .map(str::to_string);
            CdpMsg::Event {
                method,
                params: value.get("params").cloned().unwrap_or(Value::Null),
                session_id,
            }
        } else {
            let id = value.get("id").and_then(Value::as_u64).unwrap_or(0);
            let result = value.get("result").cloned();
            let error = value.get("error").map(|e| CdpErrorInfo {
                code: e.get("code").and_then(Value::as_i64).unwrap_or(-1),
                message: e.get("message").and_then(Value::as_str).unwrap_or_default().to_string(),
            });
            let session_id = value.get("sessionId").and_then(Value::as_str).map(str::to_string);
            CdpMsg::Response {
                id,
                result,
                error,
                session_id,
            }
        }
    }

    /// Send a request and return its id for later matching.
    ///
    /// # Errors
    /// Fails on socket write errors.
    pub fn send_request(&mut self, method: &str, params: &Value, session_id: Option<&str>) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session_id
            && let Some(object) = message.as_object_mut()
        {
            object.insert("sessionId".to_string(), json!(session));
        }
        self.send_raw(&message)?;
        Ok(id)
    }

    /// Send a request and block (bounded by `timeout`) until its response.
    /// Any events received in the meantime are returned so the caller can
    /// fold them into its state machine.
    ///
    /// # Errors
    /// Fails on socket errors, CDP error responses, or timeout.
    pub fn call_and_drain(
        &mut self,
        timeout: Duration,
        method: &str,
        params: &Value,
        session_id: Option<&str>,
    ) -> CdpCallOutcome {
        self.call_and_drain_until(timeout, method, params, session_id, || false)
    }

    /// Cancellation-aware variant of [`Self::call_and_drain`]. The predicate
    /// is checked at least once per socket read timeout, so browser shutdown
    /// does not wait through a sequence of full CDP call deadlines.
    pub(crate) fn call_and_drain_until(
        &mut self,
        timeout: Duration,
        method: &str,
        params: &Value,
        session_id: Option<&str>,
        mut is_cancelled: impl FnMut() -> bool,
    ) -> CdpCallOutcome {
        if is_cancelled() {
            return CdpCallOutcome {
                result: Err(CdpError::Cancelled {
                    method: method.to_string(),
                }),
                drained: Vec::new(),
            };
        }
        let id = match self.send_request(method, params, session_id) {
            Ok(id) => id,
            Err(error) => {
                return CdpCallOutcome {
                    result: Err(error),
                    drained: Vec::new(),
                };
            }
        };
        let started = Instant::now();
        let mut drained = Vec::new();
        loop {
            if is_cancelled() {
                return CdpCallOutcome {
                    result: Err(CdpError::Cancelled {
                        method: method.to_string(),
                    }),
                    drained,
                };
            }
            if started.elapsed() > timeout {
                return CdpCallOutcome {
                    result: Err(CdpError::Timeout {
                        method: method.to_string(),
                    }),
                    drained,
                };
            }
            match self.read_one() {
                Ok(Some(msg)) if msg.response_id() == Some(id) => {
                    let result = match (msg.response_error(), msg.response_result()) {
                        (Some(error), _) => Err(CdpError::from(error)),
                        (None, Some(result)) => Ok(result.clone()),
                        (None, None) => Err(CdpError::Response {
                            code: -1,
                            message: "empty CDP response".to_string(),
                        }),
                    };
                    return CdpCallOutcome { result, drained };
                }
                Ok(Some(msg)) => drained.push(msg),
                Ok(None) => {}
                Err(error) => {
                    return CdpCallOutcome {
                        result: Err(error),
                        drained,
                    };
                }
            }
        }
    }

    /// Write one CDP message to the socket.
    ///
    /// # Errors
    /// Fails on websocket write errors.
    fn send_raw(&mut self, message: &Value) -> Result<()> {
        self.ws
            .send(Message::Text(tungstenite::Utf8Bytes::from(message.to_string())))?;
        Ok(())
    }

    pub fn is_open(&self) -> bool {
        self.ws.can_read()
    }
}

impl CdpMsg {
    #[must_use]
    fn response_error(&self) -> Option<CdpErrorInfo> {
        match self {
            Self::Response { error, .. } => error.clone(),
            Self::Event { .. } => None,
        }
    }

    #[must_use]
    fn response_result(&self) -> Option<&Value> {
        match self {
            Self::Response { result, .. } => result.as_ref(),
            Self::Event { .. } => None,
        }
    }
}

fn host_port_from_ws_url(ws_url: &str) -> Result<String> {
    let rest = ws_url
        .strip_prefix("ws://")
        .ok_or_else(|| CdpError::InvalidUrl(format!("{ws_url} (only ws:// is supported)")))?;
    let authority = rest
        .split('/')
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CdpError::InvalidUrl(format!("no host in {ws_url}")))?;
    let socket = authority
        .parse::<SocketAddr>()
        .map_err(|_| CdpError::InvalidUrl(format!("invalid socket address {authority}")))?;
    match socket.ip() {
        IpAddr::V4(ip) if ip == Ipv4Addr::new(127, 0, 0, 1) => Ok(socket.to_string()),
        _ => Err(CdpError::InvalidUrl(format!(
            "non-loopback address is not allowed: {}",
            socket.ip()
        ))),
    }
}

/// Fetch a JSON document from the `DevTools` HTTP endpoint.
///
/// The browser never closes these connections even with `Connection: close`, so
/// this reads exactly `Content-Length` bytes instead of to EOF.
///
/// # Errors
/// Fails on any network or framing error, including a response header block
/// that exceeds the 64 KB bound.
pub fn fetch_json(host_port: &str, path: &str) -> std::io::Result<Value> {
    let socket = host_port.parse::<SocketAddr>().map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "devtools http: invalid socket address",
        )
    })?;
    if socket.ip() != IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "devtools http: non-loopback endpoint is not allowed",
        ));
    }
    let mut stream = TcpStream::connect_timeout(&socket, HANDSHAKE_TIMEOUT)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes())?;
    let mut buf = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        if stream.read(&mut byte)? != 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "devtools http: short header",
            ));
        }
        buf.push(byte[0]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 64 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "devtools http: header too large",
            ));
        }
    }
    let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "devtools http: malformed header",
        ));
    };
    let header_end = pos + 4;
    let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let Some(length) = headers.lines().find_map(|line| {
        line.strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse::<usize>().ok())
    }) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "devtools http: missing Content-Length",
        ));
    };
    let mut body = vec![0u8; length];
    let mut got = 0usize;
    while got < length {
        let n = stream.read(&mut body[got..])?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "devtools http: short body",
            ));
        }
        got += n;
    }
    serde_json::from_slice(&body).map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

/// Parse the `DevTools listening on ws://...` line from browser stderr.
#[must_use]
pub fn parse_devtools_ws_url(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("DevTools listening on ")?;
    let url = rest.split_whitespace().next()?;
    let url = url.trim_end_matches([')', ',']);
    if let Some(stripped) = url.strip_prefix("ws://") {
        let host_port = stripped.split('/').next().is_some_and(|h| !h.is_empty());
        if host_port {
            return Some(url.to_string());
        }
    }
    None
}

/// Pending-request bookkeeping for callers that track many in-flight
/// requests across their own event loop.
#[derive(Default)]
pub struct PendingRequests {
    by_id: HashMap<u64, PendingRequest>,
}

pub struct PendingRequest {
    pub method: String,
    pub session_id: Option<String>,
    pub sent_at: Instant,
}

impl PendingRequests {
    pub fn insert(&mut self, id: u64, method: impl Into<String>, session_id: Option<String>) {
        self.by_id.insert(
            id,
            PendingRequest {
                method: method.into(),
                session_id,
                sent_at: Instant::now(),
            },
        );
    }

    pub fn take(&mut self, id: u64) -> Option<PendingRequest> {
        self.by_id.remove(&id)
    }

    /// Ids of requests whose deadline has passed.
    pub fn expired(&mut self, timeout: Duration) -> Vec<u64> {
        let now = Instant::now();
        let expired: Vec<u64> = self
            .by_id
            .iter()
            .filter(|(_, p)| now.duration_since(p.sent_at) > timeout)
            .map(|(id, _)| *id)
            .collect();
        for id in &expired {
            self.by_id.remove(id);
        }
        expired
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_devtools_ws_url() {
        assert_eq!(
            parse_devtools_ws_url("DevTools listening on ws://127.0.0.1:39179/devtools/browser/abc"),
            Some("ws://127.0.0.1:39179/devtools/browser/abc".to_string())
        );
        assert_eq!(parse_devtools_ws_url("noise"), None);
        assert_eq!(parse_devtools_ws_url("DevTools listening on not-a-url"), None);
    }

    #[test]
    fn decodes_flattened_event_session_id() {
        let value: Value = serde_json::json!({
            "method": "Page.screencastFrame",
            "params": { "data": "x" },
            "sessionId": "ABC123"
        });
        let msg = CdpLink::decode_message(&value);
        let event = msg.event().unwrap();
        assert_eq!(event.method, "Page.screencastFrame");
        assert_eq!(event.session_id, Some("ABC123"));
    }

    #[test]
    fn decodes_response_with_error() {
        let value: Value = serde_json::json!({
            "id": 7,
            "error": { "code": -32000, "message": "boom" }
        });
        let msg = CdpLink::decode_message(&value);
        assert_eq!(msg.response_id(), Some(7));
        let error = msg.response_error().unwrap();
        assert_eq!(error.code, -32000);
        assert_eq!(error.message, "boom");
    }

    #[test]
    fn host_port_parsing() {
        assert_eq!(
            host_port_from_ws_url("ws://127.0.0.1:43977/devtools/page/xyz").unwrap(),
            "127.0.0.1:43977"
        );
        assert!(host_port_from_ws_url("http://x").is_err());
        assert!(host_port_from_ws_url("ws://localhost:43977/devtools/page/xyz").is_err());
        assert!(host_port_from_ws_url("ws://127.0.0.2:43977/devtools/page/xyz").is_err());
    }

    #[test]
    fn fetch_json_rejects_non_loopback_endpoints() {
        let err = fetch_json("192.168.0.1:9222", "/json/version").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn pending_requests_expire() {
        let mut pending = PendingRequests::default();
        pending.insert(1, "Page.enable", None);
        assert_eq!(pending.len(), 1);
        assert!(pending.expired(Duration::from_mins(1)).is_empty());
        pending.insert(2, "Page.navigate", Some("S".to_string()));
        let expired = pending.expired(Duration::from_nanos(1));
        assert_eq!(expired.len(), 2);
        assert!(pending.is_empty());
    }

    /// End-to-end framing test against a tiny mock CDP server in-process:
    /// responses match by id, events surface, and the ack round-trips.
    #[test]
    fn link_roundtrip_against_mock_server() {
        use std::net::TcpListener;
        use tungstenite::accept;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let mut ws = accept(tcp).unwrap();
            // respond to the first request, push an event, then ack the ping
            let mut got_request = false;
            while let Ok(message) = ws.read()
                && let Message::Text(text) = message
            {
                let value: Value = serde_json::from_str(text.as_str()).unwrap();
                if value.get("method").and_then(Value::as_str) == Some("Page.screencastFrameAck") {
                    let ack = json!({ "id": value.get("id"), "result": {} });
                    ws.send(Message::Text(tungstenite::Utf8Bytes::from(ack.to_string())))
                        .unwrap();
                    break;
                } else if !got_request && let Some(id) = value.get("id") {
                    got_request = true;
                    let resp = json!({ "id": id, "result": { "ok": true } });
                    ws.send(Message::Text(tungstenite::Utf8Bytes::from(resp.to_string())))
                        .unwrap();
                    let event = json!({
                        "method": "Target.targetInfoChanged",
                        "params": { "targetInfo": { "targetId": "T1", "title": "hi" } }
                    });
                    ws.send(Message::Text(tungstenite::Utf8Bytes::from(event.to_string())))
                        .unwrap();
                }
            }
        });

        let mut link =
            CdpLink::connect_with_timeout(&format!("ws://{addr}/devtools/page/x"), Duration::from_secs(2)).unwrap();
        assert_eq!(link.ws.get_ref().write_timeout().unwrap(), Some(WRITE_TIMEOUT));
        let outcome = link.call_and_drain(Duration::from_secs(5), "Page.enable", &json!({}), None);
        let result = outcome.result.unwrap();
        assert_eq!(result, json!({ "ok": true }));
        // The target-info event rides right behind the response; it is either in
        // `drained` (read before the match) or still buffered in the link.
        let is_target_info = |event: CdpEvent| event.method == "Target.targetInfoChanged" && event.session_id.is_none();
        let mut got_target_info = outcome
            .drained
            .iter()
            .find_map(CdpMsg::event)
            .is_some_and(is_target_info);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !got_target_info && Instant::now() < deadline {
            match link.read_one() {
                Ok(Some(msg)) => got_target_info = msg.event().is_some_and(is_target_info),
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        assert!(got_target_info, "target-info event");

        let ack = link
            .call_and_drain(
                Duration::from_secs(5),
                "Page.screencastFrameAck",
                &json!({ "sessionId": 1 }),
                Some("S1"),
            )
            .result
            .unwrap();
        assert_eq!(ack, json!({}));
        let _ = server.join();
    }

    #[test]
    fn link_roundtrip_honors_cancellation() {
        use std::net::TcpListener;
        use tungstenite::accept;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let mut ws = accept(tcp).unwrap();
            assert!(ws.read().is_ok());
            std::thread::sleep(Duration::from_millis(100));
        });

        let mut link =
            CdpLink::connect_with_timeout(&format!("ws://{addr}/devtools/page/x"), Duration::from_millis(10)).unwrap();
        let checks = std::cell::Cell::new(0_u32);
        let outcome = link.call_and_drain_until(Duration::from_secs(5), "Page.enable", &json!({}), None, || {
            checks.set(checks.get() + 1);
            checks.get() > 2
        });

        assert!(matches!(outcome.result, Err(CdpError::Cancelled { .. })));
        assert!(server.join().is_ok());
    }

    #[test]
    fn call_error_preserves_unrelated_messages() {
        use std::net::TcpListener;
        use tungstenite::accept;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let mut ws = accept(tcp).unwrap();
            let Message::Text(text) = ws.read().unwrap() else {
                panic!("expected request");
            };
            let request: Value = serde_json::from_str(text.as_str()).unwrap();
            let event = json!({ "method": "Page.loadEventFired", "params": {}, "sessionId": "S1" });
            ws.send(Message::Text(tungstenite::Utf8Bytes::from(event.to_string())))
                .unwrap();
            let response = json!({
                "id": request.get("id"),
                "error": { "code": -32000, "message": "navigation rejected" }
            });
            ws.send(Message::Text(tungstenite::Utf8Bytes::from(response.to_string())))
                .unwrap();
        });

        let mut link =
            CdpLink::connect_with_timeout(&format!("ws://{addr}/devtools/page/x"), Duration::from_secs(2)).unwrap();
        let outcome = link.call_and_drain(Duration::from_secs(2), "Page.navigate", &json!({}), Some("S1"));
        assert!(matches!(outcome.result, Err(CdpError::Response { code: -32000, .. })));
        assert_eq!(outcome.drained.len(), 1);
        assert_eq!(
            outcome.drained[0].event().map(|event| event.method),
            Some("Page.loadEventFired")
        );
        let _ = server.join();
    }
}
