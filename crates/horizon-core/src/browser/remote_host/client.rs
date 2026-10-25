//! The MCP client of one browser host. One request at a time, one line of
//! JSON-RPC each way, over the standard streams of the channel: the handshake,
//! then a tool call per answer. The reader runs on its own thread so a host that
//! stops answering ends as a typed failure instead of a blocked panel.

use std::fmt;
use std::io::{BufRead, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Largest line this client reads. An inlined PNG of 8 MiB is about 11 MiB of
/// base64 text, so this bound holds one such result and refuses a longer one.
const MAX_LINE_BYTES: usize = 24 * 1024 * 1024;
/// The protocol version that the server of a host of this build negotiates.
const PROTOCOL_VERSION: &str = "2025-11-25";

/// Why one request of this client did not give a result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ClientError {
    /// The host answered with its own refusal, unchanged.
    Refused(String),
    /// The host closed the channel.
    Stopped,
    /// The host gave no answer within the bound.
    NoAnswer(Duration),
    /// The request could not be written to the channel.
    Write(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(reason) => write!(formatter, "{reason}"),
            Self::Stopped => formatter.write_str("the browser host closed the channel"),
            Self::NoAnswer(timeout) => write!(
                formatter,
                "the browser host gave no answer within {} seconds",
                timeout.as_secs()
            ),
            Self::Write(reason) => write!(formatter, "the channel to the browser host refused a write: {reason}"),
        }
    }
}

/// One finished tool call of the host: the structured result, and the base64
/// PNG of a screenshot when the result carries one.
pub(super) struct ToolResult {
    pub(super) value: serde_json::Value,
    pub(super) image: Option<String>,
}

/// The client of the MCP contract of one browser host.
pub(super) struct McpClient {
    input: Box<dyn Write + Send>,
    lines: Receiver<String>,
    next_id: u64,
}

impl McpClient {
    /// Serve one channel with the streams of its server. The reader thread ends
    /// when the server closes its output, and a closed channel then answers a
    /// request with [`ClientError::Stopped`].
    pub(super) fn start(input: Box<dyn Write + Send>, output: Box<dyn BufRead + Send>) -> Self {
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            let mut output = output;
            while let Some(line) = read_line(&mut output) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            input,
            lines,
            next_id: 1,
        }
    }

    /// Negotiate the protocol with the server of the host.
    ///
    /// # Errors
    /// Returns why the host did not complete the handshake in `timeout`.
    pub(super) fn initialize(&mut self, timeout: Duration) -> Result<(), ClientError> {
        self.request(
            "initialize",
            &serde_json::json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "horizon-browser-panel",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            }),
            timeout,
        )?;
        self.notify("notifications/initialized")
    }

    /// Call one tool of the host and give its result, or the reason it refused.
    ///
    /// # Errors
    /// Returns [`ClientError::Refused`] with the reason of the host, or the
    /// transport failure that ended the request.
    pub(super) fn call(
        &mut self,
        tool: &str,
        arguments: &serde_json::Value,
        timeout: Duration,
    ) -> Result<ToolResult, ClientError> {
        let result = self.request(
            "tools/call",
            &serde_json::json!({ "name": tool, "arguments": arguments }),
            timeout,
        )?;
        if result.get("isError") == Some(&serde_json::Value::Bool(true)) {
            let reason = result
                .get("content")
                .and_then(serde_json::Value::as_array)
                .and_then(|blocks| blocks.iter().find_map(|block| block.get("text")?.as_str()))
                .unwrap_or("the browser host refused the call");
            return Err(ClientError::Refused(reason.to_string()));
        }
        let image = result
            .get("content")
            .and_then(serde_json::Value::as_array)
            .and_then(|blocks| {
                blocks
                    .iter()
                    .find_map(|block| (block.get("type")?.as_str()? == "image").then(|| block.get("data")?.as_str())?)
            })
            .map(str::to_string);
        // A server of this contract answers the structured value in
        // `structuredContent`; a server that holds only the text block answers
        // the same JSON there, so both shapes give the same value.
        let value = result
            .get("structuredContent")
            .filter(|value| !value.is_null())
            .cloned()
            .or_else(|| {
                result
                    .get("content")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|blocks| blocks.iter().find_map(|block| block.get("text")?.as_str()))
                    .and_then(|text| serde_json::from_str(text).ok())
            })
            .unwrap_or(serde_json::Value::Null);
        Ok(ToolResult { value, image })
    }

    fn notify(&mut self, method: &str) -> Result<(), ClientError> {
        self.write(&serde_json::json!({ "jsonrpc": "2.0", "method": method }))
    }

    fn request(
        &mut self,
        method: &str,
        params: &serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = match self.lines.recv_timeout(left) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => return Err(ClientError::NoAnswer(timeout)),
                Err(RecvTimeoutError::Disconnected) => return Err(ClientError::Stopped),
            };
            let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if message.get("id").and_then(serde_json::Value::as_u64) != Some(id) {
                // A notification, or the answer to a request that this client
                // already stopped waiting for.
                continue;
            }
            if let Some(error) = message.get("error") {
                let reason = error
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("the browser host refused the request");
                return Err(ClientError::Refused(reason.to_string()));
            }
            return Ok(message.get("result").cloned().unwrap_or(serde_json::Value::Null));
        }
    }

    fn write(&mut self, message: &serde_json::Value) -> Result<(), ClientError> {
        serde_json::to_writer(&mut self.input, message).map_err(|error| ClientError::Write(error.to_string()))?;
        self.input
            .write_all(b"\n")
            .and_then(|()| self.input.flush())
            .map_err(|error| ClientError::Write(error.to_string()))
    }
}

/// One line of the stream, without the memory that a line of any length would
/// take: a line longer than [`MAX_LINE_BYTES`] ends the channel instead.
fn read_line(output: &mut impl BufRead) -> Option<String> {
    let mut line = Vec::new();
    loop {
        let buffer = output.fill_buf().ok()?;
        if buffer.is_empty() {
            return None;
        }
        if let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
            line.extend_from_slice(&buffer[..=end]);
            output.consume(end + 1);
            break;
        }
        let read = buffer.len();
        line.extend_from_slice(buffer);
        output.consume(read);
        if line.len() > MAX_LINE_BYTES {
            return None;
        }
    }
    String::from_utf8(line).ok()
}
