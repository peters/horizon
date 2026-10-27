//! A fake GitHub on this machine for the tests of the module: it answers each connection
//! with the next scripted response and keeps what it received.
use super::Client;
use serde_json::Value;
use std::{
    io::{Read as _, Write as _},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

/// One request as the fake GitHub received it.
#[derive(Debug)]
pub(super) struct Request {
    /// The request line, such as `POST /login/device/code HTTP/1.1`.
    pub line: String,
    /// Each header, with a lowercase name and a trimmed value.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Request {
    /// The value of the header `name`, which is lowercase.
    pub(super) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Each request, as the fake GitHub received it.
pub(super) type Requests = Arc<Mutex<Vec<Request>>>;

/// A fake GitHub answering each connection with the next scripted response.
pub(super) fn github(responses: Vec<(u16, Value)>) -> (Client, Requests, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = Client::loopback(listener.local_addr().unwrap()).unwrap();
    let requests = Requests::default();
    let observed = requests.clone();
    let task = thread::spawn(move || {
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut input = Vec::new();
            let mut buffer = [0; 4096];
            let (head, body_start, length) = loop {
                let read = stream.read(&mut buffer).unwrap();
                input.extend_from_slice(&buffer[..read]);
                if let Some(end) = input.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&input[..end]).into_owned();
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    break (head, end + 4, length);
                }
            };
            while input.len() < body_start + length {
                let read = stream.read(&mut buffer).unwrap();
                input.extend_from_slice(&buffer[..read]);
            }
            let mut lines = head.lines();
            let line = lines.next().unwrap_or_default().to_owned();
            let headers = lines
                .filter_map(|header| header.split_once(':'))
                .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
                .collect();
            let sent = String::from_utf8_lossy(&input[body_start..body_start + length]).into_owned();
            observed.lock().unwrap().push(Request {
                line,
                headers,
                body: sent,
            });
            let text = body.to_string();
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            )
            .unwrap();
        }
    });
    (client, requests, task)
}
