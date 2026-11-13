//! The `DevTools` endpoint of a browser that another process started.
//!
//! A panel that drives a browser of another machine reaches it through a
//! loopback forward, and that browser names its own loopback port in its
//! answers. This module asks for the answer, rewrites the port to the forward of
//! this computer, and refuses any endpoint that the CDP link would not accept: a
//! browser that answers with another host would carry this session's input and
//! cookies through a name that is not loopback.

use std::time::Duration;

use crate::cdp::host_port_from_ws_url;

const VERSION_PATH: &str = "/json/version";
/// Bound of the name resolution, the connection and the answer of one request.
///
/// This request crosses the fresh forward to the browser of another machine, so
/// it holds the envelope of that machine's ssh steps (twenty seconds) and not
/// the bound of a loopback browser. At 350 ms of round trip the local connect is
/// instant and the answer of the far browser is what takes the time: a slower
/// bound read a slow link as a browser that gave no answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(20);
/// Bound of one `/json/version` answer, which is a small JSON document.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;
const USER_AGENT: &str = concat!("horizon/", env!("CARGO_PKG_VERSION"));
/// The path that Chromium writes beside its port in `DevToolsActivePort`.
const BROWSER_PATH: &str = "/devtools/";

/// Why this computer cannot use the `DevTools` endpoint of a browser.
#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error("the DevTools port 127.0.0.1:{port} gave no answer: {reason}")]
    NoAnswer { port: u16, reason: String },
    /// Nothing answered inside the bound. Over a forward to another machine this
    /// is a slow link or a busy machine, not a browser that is gone, so the
    /// caller says so.
    #[error("the DevTools port 127.0.0.1:{port} gave no answer within {seconds} {}: {reason}", seconds_unit(*seconds))]
    NoAnswerWithin { port: u16, seconds: u64, reason: String },
    #[error("127.0.0.1:{port} named a DevTools endpoint that this browser cannot drive: {reason}")]
    InvalidEndpoint { port: u16, reason: String },
    #[error("the DevToolsActivePort of that browser is not one endpoint: {reason}")]
    InvalidActivePort { reason: String },
}

/// The WebSocket endpoint of a browser that this session drives but does not
/// own, named with the loopback port of this computer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevtoolsEndpoint {
    ws_url: String,
}

impl DevtoolsEndpoint {
    /// Ask the `DevTools` endpoint that answers on the loopback `port` of this
    /// computer for the WebSocket endpoint of its browser.
    ///
    /// `port` is the port of the forward, not the port that the browser names:
    /// the browser reports the port of its own machine, which this computer
    /// cannot reach.
    ///
    /// # Errors
    /// Nothing answers on `port`, the answer is not a version document, or it
    /// names an endpoint that a loopback CDP link cannot take.
    pub fn from_port(port: u16) -> Result<Self, AttachError> {
        Self::from_port_within(port, ANSWER_TIMEOUT)
    }

    /// [`Self::from_port`] with the bound of one answer, so that a test of a
    /// slow answer reads no twenty seconds.
    pub(crate) fn from_port_within(port: u16, bound: Duration) -> Result<Self, AttachError> {
        Self::from_port_with(port, &version_agent(bound), bound)
    }

    fn from_port_with(port: u16, agent: &ureq::Agent, bound: Duration) -> Result<Self, AttachError> {
        let url = format!("http://127.0.0.1:{port}{VERSION_PATH}");
        let mut response = agent
            .get(&url)
            .call()
            .map_err(|error| answer_error(&error, port, bound))?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(|error| answer_error(&error, port, bound))?;
        if status != 200 {
            return Err(AttachError::NoAnswer {
                port,
                reason: format!("it answered {VERSION_PATH} with HTTP {status}"),
            });
        }
        let version: serde_json::Value =
            serde_json::from_slice(&body).map_err(|error| AttachError::InvalidEndpoint {
                port,
                reason: format!("its {VERSION_PATH} answer is not JSON: {error}"),
            })?;
        let reported = version
            .get("webSocketDebuggerUrl")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| AttachError::InvalidEndpoint {
                port,
                reason: format!("its {VERSION_PATH} answer names no webSocketDebuggerUrl"),
            })?;
        Self::rewritten(reported, port)
    }

    /// The endpoint that the `DevToolsActivePort` file of a browser names, read
    /// through the loopback `local_port` of this computer.
    ///
    /// The file holds the port of the browser and the path of its endpoint, one
    /// line each.
    ///
    /// # Errors
    /// The text is not those two lines, or it names an endpoint that a loopback
    /// CDP link cannot take.
    pub fn from_active_port_file(text: &str, local_port: u16) -> Result<Self, AttachError> {
        let mut lines = text.lines().map(str::trim);
        let port = lines.next().unwrap_or_default();
        let path = lines.next().unwrap_or_default();
        let remote_port: u16 = port.parse().map_err(|_| AttachError::InvalidActivePort {
            reason: format!("its first line `{port}` is not a port"),
        })?;
        if remote_port == 0 {
            return Err(AttachError::InvalidActivePort {
                reason: "its first line is port 0, which names no endpoint".to_string(),
            });
        }
        Self::from_path(path, local_port).map_err(|reason| AttachError::InvalidActivePort { reason })
    }

    /// The WebSocket endpoint of the browser, as the CDP link of this computer
    /// reaches it.
    #[must_use]
    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    /// The endpoint of `local_port` that `reported` names, with the path of the
    /// browser of that machine.
    fn rewritten(reported: &str, local_port: u16) -> Result<Self, AttachError> {
        host_port_from_ws_url(reported).map_err(|error| AttachError::InvalidEndpoint {
            port: local_port,
            reason: error.to_string(),
        })?;
        let rest = reported.strip_prefix("ws://").unwrap_or_default();
        let path = rest.find('/').map_or("", |index| &rest[index..]);
        Self::from_path(path, local_port).map_err(|reason| AttachError::InvalidEndpoint {
            port: local_port,
            reason,
        })
    }

    /// The endpoint of `local_port` at the browser `path`, when that is one
    /// endpoint that a loopback CDP link takes.
    fn from_path(path: &str, local_port: u16) -> Result<Self, String> {
        if !path.starts_with(BROWSER_PATH) {
            return Err(format!("`{path}` is not the path of a browser endpoint"));
        }
        if path.chars().any(char::is_whitespace) || path.chars().any(char::is_control) {
            return Err(format!("`{path}` is not one browser path"));
        }
        let ws_url = format!("ws://127.0.0.1:{local_port}{path}");
        host_port_from_ws_url(&ws_url).map_err(|error| error.to_string())?;
        Ok(Self { ws_url })
    }
}

/// The client of one `/json/version` call: loopback only, so no proxy from the
/// environment can carry the request of this computer to another host.
fn version_agent(bound: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .proxy(None)
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_connect(Some(bound))
        .timeout_recv_response(Some(bound))
        .timeout_recv_body(Some(bound))
        .user_agent(USER_AGENT)
        .build();
    ureq::Agent::new_with_config(config)
}

/// Why one answer of a `/json/version` call is missing. A bound that passed is
/// typed, because over a forward to another machine it is the link of that
/// machine that is slow, and the sentence a person reads says so.
fn answer_error(error: &ureq::Error, port: u16, bound: Duration) -> AttachError {
    match error {
        ureq::Error::Timeout(_) => AttachError::NoAnswerWithin {
            port,
            seconds: bound.as_secs().max(1),
            reason: error.to_string(),
        },
        _ => AttachError::NoAnswer {
            port,
            reason: error.to_string(),
        },
    }
}

/// The unit of the seconds of one bound: `1 second` reads as one second.
fn seconds_unit(seconds: u64) -> &'static str {
    if seconds == 1 { "second" } else { "seconds" }
}

impl AttachError {
    /// The bound that passed, when the answer is missing for that reason. The
    /// caller says that the link is slow instead of blaming the browser.
    #[must_use]
    pub fn waited_out(&self) -> Option<u64> {
        match self {
            Self::NoAnswerWithin { seconds, .. } => Some(*seconds),
            Self::NoAnswer { .. } | Self::InvalidEndpoint { .. } | Self::InvalidActivePort { .. } => None,
        }
    }
}

#[cfg(test)]
#[path = "attach/tests.rs"]
mod tests;
