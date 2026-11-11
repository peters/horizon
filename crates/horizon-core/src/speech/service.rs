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

use std::fmt;
use std::time::Duration;

use crate::speech::endpoint::Endpoint;
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
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ServiceError {
    #[error("the transcription service did not answer in {seconds} seconds")]
    NoAnswer { seconds: u64 },
    #[error("the transcription service answered {status}: {message}")]
    Refused { status: u16, message: String },
    #[error("the answer of the transcription service could not be read")]
    Unreadable,
    #[error("the transcription service answered no text")]
    NoText,
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
/// Reports a transport that did not answer inside the bound, a status other than
/// 200, an answer that cannot be read, and an answer with no `text`.
pub fn transcribe(request: &ServiceRequest<'_>) -> Result<String, ServiceError> {
    crate::ui_thread::blocking("one take is transcribed by a service");
    let boundary = boundary_for(request.file);
    let body = multipart_body(
        &boundary,
        BoundaryFields {
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
    let agent = ureq::Agent::new_with_config(config);
    let authorization = format!("Bearer {}", request.key);
    let url = request.endpoint.request_url();
    let mut response = agent
        .post(url.as_str())
        .header("Authorization", authorization.as_str())
        .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
        .header("Accept", "application/json")
        .send(body.as_slice())
        .map_err(|_| ServiceError::NoAnswer {
            seconds: request.bound.as_secs(),
        })?;
    let status = response.status().as_u16();
    let answer = response
        .body_mut()
        .with_config()
        .limit(MAX_ANSWER_BYTES)
        .read_to_vec()
        .map_err(|_| ServiceError::Unreadable)?;
    if status != 200 {
        return Err(ServiceError::Refused {
            status,
            message: message_of(&answer, request.key),
        });
    }
    let answer: Answer = serde_json::from_slice(&answer).map_err(|_| ServiceError::Unreadable)?;
    answer.text.filter(|text| !text.is_empty()).ok_or(ServiceError::NoText)
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
pub fn multipart_body(boundary: &str, fields: BoundaryFields<'_>) -> Vec<u8> {
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
mod tests {
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    use super::{
        BoundaryFields, MAX_ANSWER_BYTES, ServiceError, ServiceRequest, boundary_for, multipart_body, transcribe,
    };
    use crate::speech::endpoint::Endpoint;

    const KEY: &str = "sk-the-value-of-the-key";

    /// The expected body of one take, byte for byte. It is the shape that the
    /// service of a transcription reaches: the model, the format, the language and
    /// the file, in that order, with no other part.
    fn expected_body(boundary: &str) -> Vec<u8> {
        let mut expected = Vec::new();
        expected.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngpt-transcribe\r\n")
                .as_bytes(),
        );
        expected.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\njson\r\n")
                .as_bytes(),
        );
        expected.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\nno\r\n").as_bytes(),
        );
        expected.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"take-1.wav\"\r\n\
                 Content-Type: audio/wav\r\n\r\n"
            )
            .as_bytes(),
        );
        expected.extend_from_slice(b"RIFF....WAVE");
        expected.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        expected
    }

    #[test]
    fn the_multipart_body_of_one_take_is_the_expected_bytes() {
        let body = multipart_body(
            "a-boundary",
            BoundaryFields {
                model: "gpt-transcribe",
                language: Some("no"),
                file_name: "take-1.wav",
                file: b"RIFF....WAVE",
            },
        );
        assert_eq!(body, expected_body("a-boundary"));
    }

    #[test]
    fn a_take_without_a_language_leaves_that_part_out() {
        let body = multipart_body(
            "b",
            BoundaryFields {
                model: "m",
                language: None,
                file_name: "take.wav",
                file: b"x",
            },
        );
        let text = String::from_utf8(body).expect("a text body");
        assert!(!text.contains("name=\"language\""), "{text}");
        assert!(text.starts_with("--b\r\nContent-Disposition: form-data; name=\"model\""));
        assert_eq!(text.matches("--b").count(), 4, "{text}");
    }

    #[test]
    fn a_file_name_of_a_take_cannot_break_the_header() {
        let body = multipart_body(
            "c",
            BoundaryFields {
                model: "m",
                language: None,
                file_name: "a\"b\\c\r\nd.wav",
                file: b"x",
            },
        );
        let text = String::from_utf8(body).expect("a text body");
        assert!(text.contains("filename=\"abcd.wav\""), "{text}");
        assert_eq!(
            text.lines().filter(|line| line.starts_with("--c")).count(),
            4,
            "one boundary line for each of the three parts and one for the end"
        );
    }

    #[test]
    fn a_boundary_never_appears_in_the_file() {
        let file = b"RIFF horizon-0123456789abcdef0123456789abcdef WAVE";
        let boundary = boundary_for(file);
        assert!(
            !file.windows(boundary.len()).any(|window| window == boundary.as_bytes()),
            "the boundary {boundary} is in the file"
        );
    }

    /// One answer of a stand-in service: the status, the body and how long the
    /// service waits before it answers.
    fn answer_once(
        status: &str,
        body: &[u8],
        delay: Duration,
        echo_authorization: bool,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a stand-in service");
        let address = listener.local_addr().expect("the address of the stand-in").to_string();
        let status = status.to_owned();
        let body = body.to_vec();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept one request");
            let request = read_request(&mut stream);
            std::thread::sleep(delay);
            let mut response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .into_bytes();
            response.extend_from_slice(&body);
            let _ = std::io::Write::write_all(&mut stream, &response);
            let _ = std::io::Write::flush(&mut stream);
            if echo_authorization { request } else { String::new() }
        });
        (format!("http://{address}/v1/audio/transcriptions"), server)
    }

    fn read_request(stream: &mut TcpStream) -> String {
        use std::io::Read as _;
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        // The headers, and then the content length of the body.
        let mut expected = None;
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => request.extend_from_slice(&buffer[..read]),
            }
            if expected.is_none()
                && let Some(head) = request.windows(4).position(|window| window == b"\r\n\r\n")
            {
                let headers = String::from_utf8_lossy(&request[..head]).to_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                expected = Some(head + 4 + length);
            }
            if expected.is_some_and(|expected| request.len() >= expected) {
                break;
            }
        }
        String::from_utf8_lossy(&request).into_owned()
    }

    fn request<'a>(endpoint: &'a Endpoint, language: Option<&'a str>) -> ServiceRequest<'a> {
        ServiceRequest {
            endpoint,
            model: "gpt-transcribe",
            language,
            file_name: "take-1.wav",
            file: b"RIFF....WAVE",
            key: KEY,
            bound: Duration::from_secs(5),
        }
    }

    #[test]
    fn a_service_that_answers_json_with_text_answers_the_text() {
        let (endpoint, server) = answer_once("200 OK", br#"{"text":"heis"}"#, Duration::ZERO, true);
        let endpoint = Endpoint::parse(&endpoint).expect("the endpoint of the stand-in");
        let text = transcribe(&request(&endpoint, Some("no"))).expect("the text of the answer");
        assert_eq!(text, "heis");
        let received = server.join().expect("the request of the stand-in");
        assert!(
            received.starts_with("POST /v1/audio/transcriptions HTTP/1.1\r\n"),
            "{received}"
        );
    }

    #[test]
    fn the_request_carries_the_model_the_format_the_language_the_file_and_the_key() {
        let (endpoint, server) = answer_once("200 OK", br#"{"text":"x"}"#, Duration::ZERO, true);
        let endpoint = Endpoint::parse(&endpoint).expect("the endpoint of the stand-in");
        transcribe(&request(&endpoint, Some("no"))).expect("the text of the answer");
        let received = server.join().expect("the request of the stand-in");
        assert!(
            received.starts_with("POST /v1/audio/transcriptions HTTP/1.1\r\n"),
            "{received}"
        );
        assert!(
            received.contains("authorization: Bearer sk-the-value-of-the-key"),
            "{received}"
        );
        assert!(
            received.contains("content-type: multipart/form-data; boundary=horizon-"),
            "{received}"
        );
        assert!(received.contains("name=\"model\"\r\n\r\ngpt-transcribe"), "{received}");
        assert!(received.contains("name=\"response_format\"\r\n\r\njson"), "{received}");
        assert!(received.contains("name=\"language\"\r\n\r\nno"), "{received}");
        assert!(
            received.contains("name=\"file\"; filename=\"take-1.wav\""),
            "{received}"
        );
        assert!(received.contains("RIFF....WAVE\r\n--horizon-"), "{received}");
    }

    #[test]
    fn a_refused_key_is_one_line_with_the_status_and_the_message() {
        let (endpoint, server) = answer_once(
            "401 Unauthorized",
            br#"{"error":{"message":"Incorrect API key provided"}}"#,
            Duration::ZERO,
            false,
        );
        let endpoint = Endpoint::parse(&endpoint).expect("the endpoint of the stand-in");
        let error = transcribe(&request(&endpoint, None)).expect_err("a refusal");
        server.join().expect("the request of the stand-in");
        assert_eq!(
            error,
            ServiceError::Refused {
                status: 401,
                message: "Incorrect API key provided".to_owned()
            }
        );
        assert_eq!(
            error.to_string(),
            "the transcription service answered 401: Incorrect API key provided"
        );
    }

    #[test]
    fn a_service_that_echoes_the_key_writes_no_value_of_it() {
        // A service that answers the header that it received, in its message and in
        // a text that is not JSON.
        let echo = format!(r#"{{"error":{{"message":"you sent {KEY}"}}}}"#);
        let (endpoint, server) = answer_once("400 Bad Request", echo.as_bytes(), Duration::ZERO, false);
        let endpoint = Endpoint::parse(&endpoint).expect("the endpoint of the stand-in");
        let error = transcribe(&request(&endpoint, None)).expect_err("a refusal");
        server.join().expect("the request of the stand-in");
        for text in [error.to_string(), format!("{error:?}")] {
            assert!(!text.contains(KEY), "the key reached a text: {text}");
            assert!(!text.contains("sk-the-value"), "part of the key reached a text: {text}");
        }

        let plain = format!("the key {KEY} is not one of ours");
        let (endpoint, server) = answer_once("500 Server Error", plain.as_bytes(), Duration::ZERO, false);
        let endpoint = Endpoint::parse(&endpoint).expect("the endpoint of the stand-in");
        let error = transcribe(&request(&endpoint, None)).expect_err("a refusal");
        server.join().expect("the request of the stand-in");
        assert!(!error.to_string().contains(KEY), "{error}");
        assert!(error.to_string().contains("[redacted]"), "{error}");
    }

    #[test]
    fn a_message_of_a_service_is_one_line_of_two_hundred_characters() {
        let long = format!(r#"{{"error":{{"message":"{}"}}}}"#, "a\nb\tc".repeat(200));
        let (endpoint, server) = answer_once("400 Bad Request", long.as_bytes(), Duration::ZERO, false);
        let endpoint = Endpoint::parse(&endpoint).expect("the endpoint of the stand-in");
        let error = transcribe(&request(&endpoint, None)).expect_err("a refusal");
        server.join().expect("the request of the stand-in");
        let ServiceError::Refused { message, .. } = &error else {
            panic!("a refusal is a refusal: {error}");
        };
        assert_eq!(message.chars().count(), 200);
        assert!(!message.contains('\n') && !message.contains('\t'), "{message:?}");
    }

    #[test]
    fn a_slow_service_hits_the_bound_that_the_caller_gave() {
        let (endpoint, server) = answer_once("200 OK", br#"{"text":"late"}"#, Duration::from_millis(900), false);
        let endpoint = Endpoint::parse(&endpoint).expect("the endpoint of the stand-in");
        let mut request = request(&endpoint, None);
        request.bound = Duration::from_millis(150);
        let error = transcribe(&request).expect_err("the bound of the call passed");
        assert_eq!(error, ServiceError::NoAnswer { seconds: 0 });
        assert!(error.to_string().contains("did not answer"), "{error}");
        // The stand-in of this answer writes to a socket that is closed, which is
        // one answer that this test does not read.
        drop(server);
    }

    #[test]
    fn an_answer_without_text_is_no_text() {
        for body in [&b"{}"[..], br#"{"text":""}"#, br#"{"text":null}"#] {
            let (endpoint, server) = answer_once("200 OK", body, Duration::ZERO, false);
            let endpoint = Endpoint::parse(&endpoint).expect("the endpoint of the stand-in");
            let error = transcribe(&request(&endpoint, None)).expect_err("an answer without a text");
            server.join().expect("the request of the stand-in");
            assert_eq!(error, ServiceError::NoText, "{error}");
        }
    }

    #[test]
    fn a_ten_megabyte_answer_is_cut_at_the_limit_of_the_read() {
        // One answer far above the bound of the read: the call stops at the bound
        // and the memory of the process does not grow by it.
        let body = vec![b'a'; 10 * 1024 * 1024];
        let (endpoint, server) = answer_once("200 OK", &body, Duration::ZERO, false);
        let endpoint = Endpoint::parse(&endpoint).expect("the endpoint of the stand-in");
        let error = transcribe(&request(&endpoint, None)).expect_err("an answer above the bound");
        assert_eq!(error, ServiceError::Unreadable, "{error}");
        let _ = server.join();
        assert_eq!(MAX_ANSWER_BYTES, 1024 * 1024);
    }

    #[test]
    fn a_service_that_answers_no_json_is_unreadable() {
        let (endpoint, server) = answer_once("200 OK", b"not json", Duration::ZERO, false);
        let endpoint = Endpoint::parse(&endpoint).expect("the endpoint of the stand-in");
        let error = transcribe(&request(&endpoint, None)).expect_err("an answer that is not JSON");
        server.join().expect("the request of the stand-in");
        assert_eq!(error, ServiceError::Unreadable);
    }

    #[test]
    fn a_service_that_answers_nothing_is_no_answer() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a stand-in");
        let address = listener.local_addr().expect("the address of the stand-in").to_string();
        let server = std::thread::spawn(move || drop(listener));
        let endpoint = Endpoint::parse(&format!("http://{address}/v1")).expect("the endpoint");
        let error = transcribe(&request(&endpoint, None)).expect_err("a closed port");
        let _ = server.join();
        assert!(
            matches!(error, ServiceError::NoAnswer { .. } | ServiceError::Unreadable),
            "{error}"
        );
    }
}
