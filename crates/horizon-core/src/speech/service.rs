//! The call of one transcription service, and the multipart body that carries the
//! take.
//!
//! One request leaves this computer: one `multipart/form-data` body with `model`,
//! `response_format=json`, `language` when the settings name one, and `file`, and
//! one `Authorization` header with the value of one named secret. The value is in
//! that header and in no other byte of the request, no log line and no error text:
//! a message of a service that echoes the header is redacted before it reaches a
//! line, and the request has no `Debug` that shows the value.
//!
//! The answer is read with a bound of [`MAX_ANSWER_BYTES`], so a service cannot
//! fill the memory of this process, and the whole call is bounded by the time that
//! the caller gives.
//!
//! Every failure of one call has its own sentence, and each sentence names the
//! host of the endpoint and no path and no key: a name that does not resolve, a
//! connection that is refused or cannot be reached, a TLS handshake that fails, a
//! call that outlives its bound, and an answer that ended early or has another
//! shape are told apart. Only the bound names the seconds.

use std::fmt;
use std::time::Duration;

use zeroize::Zeroizing;

use crate::speech::endpoint::Endpoint;
use crate::speech::resolver::{self, SeamResolver};
use crate::speech::text::clean_text;

/// The most bytes of one answer of a service.
pub const MAX_ANSWER_BYTES: u64 = 1024 * 1024;

/// The most characters of the message of a service that Horizon keeps.
pub const MAX_MESSAGE_CHARS: usize = 200;

/// The name of the form field of the recording.
const FILE_FIELD: &str = "file";

/// The name of the form field of the model.
const MODEL_FIELD: &str = "model";

/// The name of the form field of the language hint.
const LANGUAGE_FIELD: &str = "language";

/// The name of the form field of the answer format.
const FORMAT_FIELD: &str = "response_format";

/// The answer format of one take: one JSON object with the text.
const FORMAT: &str = "json";

/// One request of one transcription service.
pub struct ServiceRequest<'a> {
    pub endpoint: &'a Endpoint,
    pub model: &'a str,
    /// The language hint, such as `no`, or `None` when the settings leave it out.
    pub language: Option<&'a str>,
    /// The name of the take file inside the form, which is Horizon's own name.
    pub file_name: &'a str,
    pub file: &'a [u8],
    /// The value of the named secret of the key. It reaches the `Authorization`
    /// header and no other byte of the request.
    pub key: &'a str,
    /// The most time that the whole call may take.
    pub bound: Duration,
}

impl fmt::Debug for ServiceRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The value of the key is not in a debug text, not even the length of it.
        formatter
            .debug_struct("ServiceRequest")
            .field("endpoint", &self.endpoint.host())
            .field("model", &self.model)
            .field("language", &self.language)
            .field("file_name", &self.file_name)
            .field("file_bytes", &self.file.len())
            .field("bound", &self.bound)
            .finish_non_exhaustive()
    }
}

/// Why one call of a transcription service answered no text.
///
/// `host` is the host of the endpoint with its port, when the endpoint names one.
/// It is never the path of the endpoint and never the value of the key.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ServiceError {
    #[error("Could not find the host {host}: the name does not resolve")]
    Unresolved { host: String },
    #[error("Could not connect to {host}: the connection was refused or the host cannot be reached")]
    Unreachable { host: String },
    #[error("Could not start TLS with {host}: {reason}")]
    Tls { host: String, reason: String },
    #[error("The transcription service at {host} did not answer in {seconds} {}", seconds_unit(*seconds))]
    NoAnswer { host: String, seconds: u64 },
    #[error("The answer of the transcription service at {host} ended early or was not the expected shape")]
    EndedEarly { host: String },
    #[error("The transcription service at {host} answered {status}: {message}")]
    Refused { host: String, status: u16, message: String },
    #[error("The transcription service at {host} answered no text")]
    NoText { host: String },
}

/// The unit of the seconds of one bound: `1 second` reads as one second.
fn seconds_unit(seconds: u64) -> &'static str {
    if seconds == 1 { "second" } else { "seconds" }
}

/// One answer of one transcription service.
#[derive(Debug, serde::Deserialize)]
struct Answer {
    text: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ErrorAnswer {
    error: Option<ErrorMessage>,
    message: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ErrorMessage {
    message: Option<String>,
}

/// The text of one take, as `request` answered it.
///
/// # Errors
/// Reports a name that does not resolve, a connection that is refused or cannot
/// be reached, a TLS handshake that fails, a call that outlives its bound, an
/// answer that ended early or cannot be read, a status other than 200, and an
/// answer with no `text`.
pub fn transcribe(request: &ServiceRequest<'_>) -> Result<String, ServiceError> {
    transcribe_with(SeamResolver::system(), request)
}

/// [`transcribe`] with the name lookup of a caller, so that a test of a name reads
/// no resolver of this computer.
fn transcribe_with(resolver: SeamResolver, request: &ServiceRequest<'_>) -> Result<String, ServiceError> {
    crate::ui_thread::blocking("one take is transcribed by a service");
    let boundary = boundary_for(request.file);
    let body = multipart_body(
        &boundary,
        &BoundaryFields {
            model: request.model,
            language: request.language,
            file_name: request.file_name,
            file: request.file,
        },
    );
    let config = ureq::Agent::config_builder()
        // The endpoint is the decision of the person: a redirect would carry the
        // take and the key to a host that they did not name.
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(request.bound))
        .build();
    let agent = resolver::agent(config, resolver);
    // The value of the key travels in this header and in no other byte, and the
    // line is wiped when the request ends.
    let authorization = Zeroizing::new(format!("Bearer {}", request.key));
    let host = request.endpoint.host_and_port();
    let url = request.endpoint.request_url();
    let mut response = agent
        .post(url.as_str())
        .header("Authorization", authorization.as_str())
        .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
        .header("Accept", "application/json")
        .send(body.as_slice())
        .map_err(|error| transport_error(&error, &host, request.bound))?;
    let status = response.status().as_u16();
    let answer = response
        .body_mut()
        .with_config()
        .limit(MAX_ANSWER_BYTES)
        .read_to_vec()
        .map_err(|error| answer_error(&error, &host, request.bound))?;
    if status != 200 {
        return Err(ServiceError::Refused {
            host,
            status,
            message: message_of(&answer, request.key),
        });
    }
    let Ok(answer) = serde_json::from_slice::<Answer>(&answer) else {
        return Err(ServiceError::EndedEarly { host });
    };
    answer
        .text
        .filter(|text| !text.is_empty())
        .ok_or(ServiceError::NoText { host })
}

/// The sentence of a request that reached no answer of a service.
fn transport_error(error: &ureq::Error, host: &str, bound: Duration) -> ServiceError {
    let host = host.to_owned();
    match error {
        ureq::Error::HostNotFound => ServiceError::Unresolved { host },
        ureq::Error::Tls(reason) => ServiceError::Tls {
            host,
            reason: (*reason).to_owned(),
        },
        ureq::Error::Rustls(error) => ServiceError::Tls {
            host,
            reason: error.to_string(),
        },
        ureq::Error::TlsRequired => ServiceError::Tls {
            host,
            reason: "the endpoint needs TLS and this call made a plain connection".to_owned(),
        },
        // rustls reports every failure of one handshake — a certificate that this
        // computer does not trust, and a protocol that the peer does not speak — as
        // an invalid byte of io, and no other step of one request does.
        ureq::Error::Io(error) if error.kind() == std::io::ErrorKind::InvalidData => ServiceError::Tls {
            host,
            reason: error.to_string(),
        },
        ureq::Error::Timeout(_) => no_answer(host, bound),
        // The bytes of an answer arrived and their shape was not the one of HTTP:
        // this call reached the host, so it is no failure of a connection.
        ureq::Error::Protocol(_) => ServiceError::EndedEarly { host },
        ureq::Error::Io(_) | ureq::Error::ConnectionFailed | ureq::Error::ConnectProxyFailed(_) => {
            ServiceError::Unreachable { host }
        }
        // The shape of the request and not the reach of the host: the endpoint is
        // checked before this call, so this arm answers for an error that no
        // endpoint of the settings can produce.
        _ => ServiceError::Unreachable { host },
    }
}

/// The sentence of an answer that ended before its body, or of a body that the
/// bound of the call cut.
fn answer_error(error: &ureq::Error, host: &str, bound: Duration) -> ServiceError {
    let host = host.to_owned();
    match error {
        ureq::Error::Timeout(_) => no_answer(host, bound),
        _ => ServiceError::EndedEarly { host },
    }
}

/// The sentence of a call that outlived its bound.
fn no_answer(host: String, bound: Duration) -> ServiceError {
    ServiceError::NoAnswer {
        host,
        seconds: bound.as_secs(),
    }
}

/// The parts of one multipart body.
pub struct BoundaryFields<'a> {
    pub model: &'a str,
    pub language: Option<&'a str>,
    pub file_name: &'a str,
    pub file: &'a [u8],
}

/// The body of one `multipart/form-data` request.
#[must_use]
pub fn multipart_body(boundary: &str, fields: &BoundaryFields<'_>) -> Vec<u8> {
    let mut body = Vec::with_capacity(fields.file.len() + 512);
    for (name, value) in [
        (MODEL_FIELD, fields.model),
        (FORMAT_FIELD, FORMAT),
        (LANGUAGE_FIELD, fields.language.unwrap_or("")),
    ] {
        if value.is_empty() {
            continue;
        }
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes());
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"{FILE_FIELD}\"; filename=\"{}\"\r\n",
            quote_name(fields.file_name)
        )
        .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: audio/wav\r\n\r\n");
    body.extend_from_slice(fields.file);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// A boundary that the file does not hold, so the body has exactly the parts that
/// this module wrote.
#[must_use]
pub fn boundary_for(file: &[u8]) -> String {
    for _ in 0..8 {
        let boundary = format!("horizon-{}", uuid::Uuid::new_v4().simple());
        if !file.windows(boundary.len()).any(|window| window == boundary.as_bytes()) {
            return boundary;
        }
    }
    // Sixteen random bytes of two boundaries: a file that holds both is not a
    // recording, and a boundary of this text still ends the same parts.
    format!("horizon-{}", uuid::Uuid::new_v4().simple())
}

/// A file name of a Content-Disposition header: a quote, a backslash and a
/// control character are removed, because the header is one line.
fn quote_name(name: &str) -> String {
    name.chars()
        .filter(|character| !matches!(character, '"' | '\\') && !character.is_control())
        .collect()
}

/// The message of one answer of a service, as one line of at most
/// [`MAX_MESSAGE_CHARS`] characters, with the value of the key removed.
fn message_of(answer: &[u8], key: &str) -> String {
    let message = serde_json::from_slice::<ErrorAnswer>(answer)
        .ok()
        .and_then(|answer| answer.error.and_then(|error| error.message).or(answer.message))
        .unwrap_or_else(|| String::from_utf8_lossy(answer).into_owned());
    let line = clean_text(&redact(&message, key));
    line.chars().take(MAX_MESSAGE_CHARS).collect()
}

/// `text` without the value of the key, so a service that echoes the header writes
/// no value into an error text.
fn redact(text: &str, key: &str) -> String {
    if key.is_empty() {
        return text.to_owned();
    }
    text.replace(key, "[redacted]")
}

#[cfg(test)]
mod tests;
