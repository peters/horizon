//! A loopback listener that answers one `/json/version` call, rather than a
//! browser.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use super::{AttachError, DevtoolsEndpoint};

const BROWSER_PATH: &str = "/devtools/browser/6a0c1f4e-0b7d-4d1e-9c0b-2a7f4c5d6e70";

/// One listener that reads one HTTP request and answers it, on a loopback port
/// that the caller cannot collide with.
fn answer(status: &str, body: &str) -> (u16, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the mock");
    let port = listener.local_addr().expect("mock address").port();
    let status = status.to_string();
    let body = body.to_string();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept the mock request");
        let request = read_request(&mut stream);
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
        request
    });
    (port, server)
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0u8; 512];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => request.extend_from_slice(&buffer[..read]),
        }
    }
    String::from_utf8_lossy(&request).into_owned()
}

fn version_document(url: &str) -> String {
    format!(r#"{{"Browser":"Chrome/156.0.1","Protocol-Version":"1.3","webSocketDebuggerUrl":"{url}"}}"#)
}

/// A loopback port that nobody listens on.
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a port");
    let port = listener.local_addr().expect("address").port();
    drop(listener);
    port
}

#[test]
fn the_answer_of_a_browser_names_its_endpoint_at_the_port_of_this_computer() {
    let reported = format!("ws://127.0.0.1:9222{BROWSER_PATH}");
    let (port, server) = answer("200 OK", &version_document(&reported));
    let endpoint = DevtoolsEndpoint::from_port(port).expect("the endpoint of the mock browser");
    let request = server.join().expect("the mock request");

    assert_eq!(
        endpoint.ws_url(),
        format!("ws://127.0.0.1:{port}{BROWSER_PATH}"),
        "the reported port of the browser of that machine is not the one this computer reaches"
    );
    assert!(
        request.starts_with("GET /json/version HTTP/1.1\r\n"),
        "the request reads the version document: {request}"
    );
}

#[test]
fn an_endpoint_on_a_host_that_is_not_loopback_is_refused() {
    for (reported, reason) in [
        (format!("ws://10.1.2.3:9222{BROWSER_PATH}"), "not allowed"),
        (format!("ws://localhost:9222{BROWSER_PATH}"), "invalid socket address"),
        (format!("wss://127.0.0.1:9222{BROWSER_PATH}"), "only ws://"),
    ] {
        let (port, server) = answer("200 OK", &version_document(&reported));
        let error = DevtoolsEndpoint::from_port(port).expect_err(&reported);
        server.join().expect("the mock request");
        assert!(
            matches!(&error, AttachError::InvalidEndpoint { port: refused, .. } if *refused == port),
            "{reported}: {error}"
        );
        assert!(error.to_string().contains(reason), "{reported}: {error}");
    }
}

#[test]
fn an_endpoint_without_a_browser_path_is_refused() {
    let (port, server) = answer("200 OK", &version_document("ws://127.0.0.1:9222"));
    let error = DevtoolsEndpoint::from_port(port).expect_err("an endpoint without a path");
    server.join().expect("the mock request");
    assert!(matches!(error, AttachError::InvalidEndpoint { .. }), "{error}");
    assert!(
        error.to_string().contains("not the path of a browser endpoint"),
        "{error}"
    );
}

#[test]
fn an_answer_that_is_not_a_version_document_is_refused() {
    let (port, server) = answer("200 OK", "<html>hello</html>");
    let error = DevtoolsEndpoint::from_port(port).expect_err("an answer that is not JSON");
    server.join().expect("the mock request");
    assert!(matches!(error, AttachError::InvalidEndpoint { .. }), "{error}");
    assert!(error.to_string().contains("is not JSON"), "{error}");

    let (port, server) = answer("200 OK", "{}");
    let error = DevtoolsEndpoint::from_port(port).expect_err("an answer without an endpoint");
    server.join().expect("the mock request");
    assert!(error.to_string().contains("names no webSocketDebuggerUrl"), "{error}");
}

#[test]
fn a_port_of_a_browser_that_answers_nothing_gives_no_answer() {
    let port = closed_port();
    let started = Instant::now();
    let error = DevtoolsEndpoint::from_port(port).expect_err("a port that nobody listens on");
    assert!(
        matches!(&error, AttachError::NoAnswer { port: refused, .. } if *refused == port),
        "{error}"
    );
    assert!(started.elapsed() < Duration::from_secs(5), "{error}");
}

#[test]
fn an_answer_that_is_not_a_success_gives_no_answer() {
    let (port, server) = answer("404 Not Found", "");
    let error = DevtoolsEndpoint::from_port(port).expect_err("a refused request");
    server.join().expect("the mock request");
    assert!(matches!(error, AttachError::NoAnswer { .. }), "{error}");
    assert!(error.to_string().contains("HTTP 404"), "{error}");
}

#[test]
fn the_port_file_of_a_running_browser_names_the_endpoint_of_the_forward() {
    let text = format!("41000\n{BROWSER_PATH}\n");
    let endpoint = DevtoolsEndpoint::from_active_port_file(&text, 51234).expect("the endpoint of the file");
    assert_eq!(endpoint.ws_url(), format!("ws://127.0.0.1:51234{BROWSER_PATH}"));
    let without_newline = DevtoolsEndpoint::from_active_port_file(&format!("41000\n{BROWSER_PATH}"), 51234)
        .expect("the last line needs no newline");
    assert_eq!(without_newline, endpoint);
}

#[test]
fn a_port_file_that_is_not_two_lines_around_a_port_is_refused() {
    for text in [
        "",
        "\n",
        "41000",
        "41000\n",
        "port\n/devtools/browser/abc",
        "0\n/devtools/browser/abc",
        "41000\n/tmp/something",
        "41000\n/devtools/browser/a b",
        "41000\n\n/devtools/browser/abc",
    ] {
        let error =
            DevtoolsEndpoint::from_active_port_file(text, 51234).expect_err(&format!("{text:?} is not one endpoint"));
        assert!(
            matches!(error, AttachError::InvalidActivePort { .. }),
            "{text:?}: {error}"
        );
    }
}

/// One listener that reads one request and answers it only after `delay`, so a
/// test reads what a slow forward to a browser of another machine does.
fn answer_late(delay: Duration) -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the mock");
    let port = listener.local_addr().expect("mock address").port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept the mock request");
        let _ = read_request(&mut stream);
        std::thread::sleep(delay);
        let body = version_document(&format!("ws://127.0.0.1:9222{BROWSER_PATH}"));
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    });
    (port, server)
}

#[test]
fn an_answer_that_passes_its_bound_is_no_browser_without_an_endpoint() {
    let (port, server) = answer_late(Duration::from_millis(400));
    let error = DevtoolsEndpoint::from_port_within(port, Duration::from_millis(150))
        .expect_err("the answer came after the bound");
    assert_eq!(
        error.waited_out(),
        Some(1),
        "a bound that passed is typed, so the caller names the link: {error}"
    );
    assert!(
        error.to_string().contains("gave no answer within 1 second"),
        "the sentence holds the wait: {error}"
    );
    let _ = server.join();
}

#[test]
fn the_answer_bound_of_a_fresh_forward_fits_a_browser_on_another_machine() {
    // The envelope of the ssh steps of a browser host is twenty seconds; a
    // loopback bound of five seconds read a slow link as a missing browser.
    assert!(
        super::ANSWER_TIMEOUT >= Duration::from_secs(20),
        "the answer bound of a forwarded browser is {} s",
        super::ANSWER_TIMEOUT.as_secs()
    );
}
