//! Loopback HTTP/1.1 responder for transport and lifecycle tests: answers
//! one connection per scripted reply and records what each request carried.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// One scripted reply.
pub(super) struct Reply {
    pub(super) status: u16,
    pub(super) body: Vec<u8>,
    pub(super) headers: Vec<String>,
    pub(super) delay: Duration,
    /// Pause between the header block and the body, to stall a body read.
    pub(super) body_delay: Duration,
    pub(super) declared_length: Option<usize>,
}

impl Reply {
    pub(super) fn json(status: u16, body: &serde_json::Value) -> Self {
        Self {
            status,
            body: body.to_string().into_bytes(),
            headers: vec!["Content-Type: application/json".into()],
            delay: Duration::ZERO,
            body_delay: Duration::ZERO,
            declared_length: None,
        }
    }

    pub(super) fn delayed(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub(super) fn body_delayed(mut self, delay: Duration) -> Self {
        self.body_delay = delay;
        self
    }
}

#[derive(Clone, Debug)]
pub(super) struct Recorded {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) authorization: Option<String>,
    pub(super) body: String,
}

pub(super) struct Server {
    pub(super) port: u16,
    seen: Arc<Mutex<Vec<Recorded>>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Server {
    pub(super) fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let handle = thread::spawn(move || {
            for reply in replies {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let Some((head_end, request)) = read_request(&mut stream) else {
                    return;
                };
                recorder.lock().expect("lock").push(parse_request(&request, head_end));
                thread::sleep(reply.delay);
                let length = reply.declared_length.unwrap_or(reply.body.len());
                let mut response = format!(
                    "HTTP/1.1 {} Reply\r\nContent-Length: {length}\r\nConnection: close\r\n",
                    reply.status
                );
                for header in &reply.headers {
                    response.push_str(header);
                    response.push_str("\r\n");
                }
                response.push_str("\r\n");
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
                thread::sleep(reply.body_delay);
                let _ = stream.write_all(&reply.body);
                let _ = stream.flush();
            }
        });
        Self {
            port,
            seen,
            handle: Some(handle),
        }
    }

    pub(super) fn endpoint(&self, base_path: &str) -> String {
        format!("http://127.0.0.1:{}{base_path}", self.port)
    }

    pub(super) fn recorded(&self) -> Vec<Recorded> {
        self.seen.lock().expect("lock").clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Unblock accept() if a test ended early, then join.
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> Option<(usize, Vec<u8>)> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let (head_end, content_length) = loop {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buffer[..end]).to_string();
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            break (end + 4, length);
        }
    };
    while buffer.len() < head_end + content_length {
        let Ok(read) = stream.read(&mut chunk) else { break };
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Some((head_end, buffer))
}

fn parse_request(buffer: &[u8], head_end: usize) -> Recorded {
    let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let authorization = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("authorization")
            .then(|| value.trim().to_string())
    });
    let body = String::from_utf8_lossy(&buffer[head_end..]).to_string();
    Recorded {
        method,
        path,
        authorization,
        body,
    }
}
