//! The body of one request, the sentences of its failures, and the answers of a
//! stand-in service.
//!
//! The cases of a transport that a person can reach are here as real listeners on
//! `127.0.0.1` port 0: a port that nothing listens on, a listener that accepts and
//! never answers, a listener that closes after half an answer, and a listener that
//! answers plain text where a TLS record belongs. A name that does not resolve
//! goes through the lookup seam, so no test of this module reads the resolver of
//! this computer.

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use horizon_browser::lookup::NameLookup;

use super::{
    BoundaryFields, MAX_ANSWER_BYTES, ServiceError, ServiceRequest, boundary_for, multipart_body, transcribe,
    transcribe_with,
};

use crate::speech::endpoint::Endpoint;
use crate::speech::resolver::SeamResolver;

const KEY: &str = "sk-the-value-of-the-key";

/// The expected body of one take, byte for byte. It is the shape that the
/// service of a transcription reaches: the model, the format, the language and
/// the file, in that order, with no other part.
fn expected_body(boundary: &str) -> Vec<u8> {
    let mut expected = Vec::new();
    expected.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngpt-transcribe\r\n").as_bytes(),
    );
    expected.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\njson\r\n").as_bytes(),
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
        &BoundaryFields {
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
        &BoundaryFields {
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
        &BoundaryFields {
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
        upload: Duration::from_secs(30),
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
    let (url, server) = answer_once(
        "401 Unauthorized",
        br#"{"error":{"message":"Incorrect API key provided"}}"#,
        Duration::ZERO,
        false,
    );
    let endpoint = Endpoint::parse(&url).expect("the endpoint of the stand-in");
    let host = endpoint.host_and_port();
    let error = transcribe(&request(&endpoint, None)).expect_err("a refusal");
    server.join().expect("the request of the stand-in");
    assert_eq!(
        error,
        ServiceError::Refused {
            host: host.clone(),
            status: 401,
            message: "Incorrect API key provided".to_owned()
        }
    );
    assert_eq!(
        error.to_string(),
        format!("The transcription service at {host} answered 401: Incorrect API key provided")
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
fn a_service_that_accepts_and_never_answers_names_the_seconds_of_the_bound() {
    // The listener accepts the request and holds the connection open, as a
    // service that waits behind a lock does.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a stand-in service");
    let address = listener.local_addr().expect("the address of the stand-in").to_string();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept one request");
        let _request = read_request(&mut stream);
        std::thread::sleep(Duration::from_secs(3));
    });
    let endpoint = Endpoint::parse(&format!("http://{address}/v1/audio/transcriptions")).expect("the endpoint");
    let mut request = request(&endpoint, None);
    // A short bound of the test, on the code path of the bound of 30 seconds.
    request.bound = Duration::from_millis(1100);
    let error = transcribe(&request).expect_err("the bound of the call passed");
    assert_eq!(
        error,
        ServiceError::NoAnswer {
            host: address.clone(),
            seconds: 1
        }
    );
    assert_eq!(
        error.to_string(),
        format!("The transcription service at {address} did not answer in 1 second")
    );
    // The stand-in of this answer writes to a socket that is closed, which is
    // one answer that this test does not read.
    drop(server);
}

#[test]
fn a_port_that_nothing_listens_on_says_that_the_connection_was_refused() {
    // The port of a listener that is dropped: nothing listens on it now.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a port of the test");
    let address = listener.local_addr().expect("the address of the test").to_string();
    drop(listener);
    let endpoint = Endpoint::parse(&format!("http://{address}/v1/audio/transcriptions")).expect("the endpoint");
    let error = transcribe(&request(&endpoint, None)).expect_err("a port with no listener");
    assert_eq!(error, ServiceError::Unreachable { host: address.clone() });
    assert_eq!(
        error.to_string(),
        format!("Could not connect to {address}: the connection was refused or the host cannot be reached")
    );
    let sentence = error.to_string();
    assert!(!sentence.contains("seconds"), "a refusal is not a bound: {sentence}");
    assert!(!sentence.contains(KEY), "the key reached a sentence: {sentence}");
}

#[test]
fn an_answer_that_ends_after_half_of_its_body_ended_early() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a stand-in service");
    let address = listener.local_addr().expect("the address of the stand-in").to_string();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept one request");
        let _request = read_request(&mut stream);
        // A body of 40 bytes of which this stand-in writes five, and then it
        // closes the connection.
        let response = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 40\r\n\r\n{\"tex";
        let _ = std::io::Write::write_all(&mut stream, response);
        let _ = std::io::Write::flush(&mut stream);
    });
    let endpoint = Endpoint::parse(&format!("http://{address}/v1/audio/transcriptions")).expect("the endpoint");
    let error = transcribe(&request(&endpoint, None)).expect_err("an answer that ended early");
    let _ = server.join();
    assert_eq!(error, ServiceError::EndedEarly { host: address.clone() });
    assert_eq!(
        error.to_string(),
        format!("The answer of the transcription service at {address} ended early or was not the expected shape")
    );
    assert!(!error.to_string().contains("seconds"), "{error}");
}

#[test]
fn an_answer_that_is_not_http_ended_early() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a stand-in service");
    let address = listener.local_addr().expect("the address of the stand-in").to_string();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept one request");
        let _request = read_request(&mut stream);
        let _ = std::io::Write::write_all(&mut stream, b"not an HTTP answer\r\n\r\n");
        let _ = std::io::Write::flush(&mut stream);
    });
    let endpoint = Endpoint::parse(&format!("http://{address}/v1/audio/transcriptions")).expect("the endpoint");
    let error = transcribe(&request(&endpoint, None)).expect_err("an answer that is not HTTP");
    let _ = server.join();
    assert_eq!(error, ServiceError::EndedEarly { host: address.clone() });
    assert!(!error.to_string().contains("seconds"), "{error}");
    assert!(!error.to_string().contains("Could not connect"), "{error}");
}

#[test]
fn a_tls_handshake_that_fails_says_so_and_names_the_host() {
    // A listener that answers plain text on a port that the endpoint reaches
    // over https: the handshake reads a byte where a record begins.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a stand-in service");
    let address = listener.local_addr().expect("the address of the stand-in").to_string();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept one connection");
        let _ = std::io::Write::write_all(&mut stream, b"HTTP/1.1 400 Bad Request\r\n\r\n");
        let _ = std::io::Write::flush(&mut stream);
        std::thread::sleep(Duration::from_millis(200));
    });
    let endpoint = Endpoint::parse(&format!("https://{address}/v1/audio/transcriptions")).expect("the endpoint");
    let error = transcribe(&request(&endpoint, None)).expect_err("a handshake that fails");
    let _ = server.join();
    let ServiceError::Tls { host, .. } = &error else {
        panic!("a TLS failure is a TLS failure: {error}");
    };
    assert_eq!(host, &address);
    let sentence = error.to_string();
    assert!(
        sentence.starts_with(&format!("Could not start TLS with {address}: ")),
        "{sentence}"
    );
    assert!(!sentence.contains("seconds"), "{sentence}");
    assert!(!sentence.contains(KEY), "the key reached a sentence: {sentence}");
}

/// A lookup that answers no address for any name, as the resolver of a
/// computer that has no record of a name does.
struct NothingLookup;

impl NameLookup for NothingLookup {
    fn resolve(&self, _host: &str, _port: u16) -> std::io::Result<Vec<SocketAddr>> {
        Err(std::io::Error::from(std::io::ErrorKind::NotFound))
    }
}

#[test]
fn a_name_that_does_not_resolve_says_so_and_names_the_host() {
    let endpoint = Endpoint::parse("https://no-such-host.invalid/v1/audio/transcriptions").expect("the endpoint");
    let resolver = SeamResolver::new(Arc::new(NothingLookup));
    let error = transcribe_with(resolver, &request(&endpoint, None)).expect_err("a name with no address");
    assert_eq!(
        error,
        ServiceError::Unresolved {
            host: "no-such-host.invalid".to_owned()
        }
    );
    assert_eq!(
        error.to_string(),
        "Could not find the host no-such-host.invalid: the name does not resolve"
    );
    let sentence = error.to_string();
    assert!(
        !sentence.contains("seconds"),
        "a name that does not resolve is not a bound: {sentence}"
    );
    assert!(
        !sentence.contains("transcriptions"),
        "a sentence names no path: {sentence}"
    );
    assert!(!sentence.contains(KEY), "the key reached a sentence: {sentence}");
}

#[test]
fn an_answer_without_text_is_no_text() {
    for body in [&b"{}"[..], br#"{"text":""}"#, br#"{"text":null}"#] {
        let (url, server) = answer_once("200 OK", body, Duration::ZERO, false);
        let endpoint = Endpoint::parse(&url).expect("the endpoint of the stand-in");
        let host = endpoint.host_and_port();
        let error = transcribe(&request(&endpoint, None)).expect_err("an answer without a text");
        server.join().expect("the request of the stand-in");
        assert_eq!(error, ServiceError::NoText { host }, "{error}");
    }
}

#[test]
fn a_ten_megabyte_answer_is_cut_at_the_limit_of_the_read() {
    // One answer far above the bound of the read: the call stops at the bound
    // and the memory of the process does not grow by it.
    let body = vec![b'a'; 10 * 1024 * 1024];
    let (url, server) = answer_once("200 OK", &body, Duration::ZERO, false);
    let endpoint = Endpoint::parse(&url).expect("the endpoint of the stand-in");
    let host = endpoint.host_and_port();
    let error = transcribe(&request(&endpoint, None)).expect_err("an answer above the bound");
    assert_eq!(error, ServiceError::EndedEarly { host }, "{error}");
    let _ = server.join();
    assert_eq!(MAX_ANSWER_BYTES, 1024 * 1024);
}

#[test]
fn a_service_that_answers_no_json_ended_early() {
    let (url, server) = answer_once("200 OK", b"not json", Duration::ZERO, false);
    let endpoint = Endpoint::parse(&url).expect("the endpoint of the stand-in");
    let host = endpoint.host_and_port();
    let error = transcribe(&request(&endpoint, None)).expect_err("an answer that is not JSON");
    server.join().expect("the request of the stand-in");
    assert_eq!(error, ServiceError::EndedEarly { host }, "{error}");
}
