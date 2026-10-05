//! Minimal HTTP server that hands the playlist and segments to the
//! receiver. Paths carry a random token so other LAN hosts cannot guess them.
use super::hls::Segmenter;
use crate::Result;
use std::{
    hash::{BuildHasher, RandomState},
    io::{ErrorKind, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const ACCEPT_POLL: Duration = Duration::from_millis(20);
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CONNECTIONS: usize = 8;
const MAX_REQUEST: usize = 8 * 1024;

pub(crate) struct HttpServer {
    port: u16,
    token: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl HttpServer {
    pub(crate) fn start(segments: Arc<Mutex<Segmenter>>) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let token = format!("{:016x}", RandomState::new().hash_one(port));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (stop, token) = (stop.clone(), token.clone());
            std::thread::Builder::new()
                .name("chromecast-http".to_owned())
                .spawn(move || accept_loop(&listener, &segments, &token, &stop))?
        };
        Ok(Self {
            port,
            token,
            stop,
            thread: Some(thread),
        })
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn accept_loop(listener: &TcpListener, segments: &Arc<Mutex<Segmenter>>, token: &str, stop: &AtomicBool) {
    let active = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((socket, peer)) => {
                if active.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
                    active.fetch_sub(1, Ordering::AcqRel);
                    continue;
                }
                let (segments, token, slot) = (segments.clone(), token.to_owned(), active.clone());
                let spawned = std::thread::Builder::new()
                    .name("chromecast-http-conn".to_owned())
                    .spawn(move || {
                        if let Err(error) = serve(socket, &segments, &token) {
                            tracing::debug!(%error, %peer, "chromecast HTTP request failed");
                        }
                        slot.fetch_sub(1, Ordering::AcqRel);
                    });
                if spawned.is_err() {
                    // The closure never ran, so release its slot here.
                    active.fetch_sub(1, Ordering::AcqRel);
                    tracing::warn!("could not start a chromecast HTTP connection thread");
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => std::thread::sleep(ACCEPT_POLL),
            Err(error) => {
                tracing::warn!(%error, "chromecast HTTP accept failed");
                std::thread::sleep(ACCEPT_POLL);
            }
        }
    }
}

fn serve(mut socket: TcpStream, segments: &Mutex<Segmenter>, token: &str) -> std::io::Result<()> {
    // Accepted sockets inherit non-blocking mode on some platforms.
    socket.set_nonblocking(false)?;
    socket.set_write_timeout(Some(IO_TIMEOUT))?;
    // One deadline for the whole header, so a slow trickle cannot hold a slot.
    let deadline = Instant::now() + IO_TIMEOUT;
    let mut request = Vec::new();
    let mut chunk = [0; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        socket.set_read_timeout(Some(remaining))?;
        let read = socket.read(&mut chunk)?;
        if read == 0 || request.len() > MAX_REQUEST {
            return Ok(());
        }
        request.extend_from_slice(&chunk[..read]);
    }
    let line = String::from_utf8_lossy(&request);
    let mut words = line.split_whitespace();
    let (method, path) = (words.next().unwrap_or_default(), words.next().unwrap_or_default());
    let response = route(method, path, token, segments);
    socket.write_all(&response.head())?;
    if method == "GET"
        && let Some(body) = response.body
    {
        socket.write_all(&body)?;
    }
    socket.flush()
}

struct Response {
    status: &'static str,
    content_type: &'static str,
    body: Option<Arc<[u8]>>,
}

impl Response {
    fn empty(status: &'static str) -> Self {
        Self {
            status,
            content_type: "text/plain",
            body: None,
        }
    }

    fn head(&self) -> Vec<u8> {
        let length = self.body.as_ref().map_or(0, |body| body.len());
        format!(
            "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {length}\r\nCache-Control: no-cache\r\n\
             Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: *\r\n\
             Access-Control-Allow-Methods: GET, HEAD, OPTIONS\r\nConnection: close\r\n\r\n",
            self.status, self.content_type
        )
        .into_bytes()
    }
}

fn route(method: &str, path: &str, token: &str, segments: &Mutex<Segmenter>) -> Response {
    match method {
        "OPTIONS" => return Response::empty("204 No Content"),
        "GET" | "HEAD" => {}
        _ => return Response::empty("405 Method Not Allowed"),
    }
    let Some(name) = path
        .strip_prefix('/')
        .and_then(|p| p.strip_prefix(token))
        .and_then(|p| p.strip_prefix('/'))
    else {
        return Response::empty("404 Not Found");
    };
    let segments = segments.lock().unwrap_or_else(PoisonError::into_inner);
    if name == "live.m3u8" {
        return Response {
            status: "200 OK",
            content_type: "application/vnd.apple.mpegurl",
            body: Some(segments.playlist().into_bytes().into()),
        };
    }
    let segment = name
        .strip_prefix("seg")
        .and_then(|n| n.strip_suffix(".ts"))
        .and_then(|n| n.parse().ok())
        .and_then(|sequence| segments.segment(sequence));
    match segment {
        Some(body) => Response {
            status: "200 OK",
            content_type: "video/mp2t",
            body: Some(body),
        },
        None => Response::empty("404 Not Found"),
    }
}

/// The local address the OS would use to reach `receiver`.
pub(crate) fn route_address(receiver: SocketAddr) -> std::io::Result<std::net::IpAddr> {
    let probe = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    probe.connect(receiver)?;
    Ok(probe.local_addr()?.ip())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(port: u16, path: &str) -> String {
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        write!(socket, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut response = Vec::new();
        socket.read_to_end(&mut response).unwrap();
        String::from_utf8_lossy(&response).into_owned()
    }

    #[test]
    fn serves_playlist_and_segments_behind_the_token() {
        let segmenter = Arc::new(Mutex::new(Segmenter::new(Duration::from_secs(1), 3)));
        {
            let mut segmenter = segmenter.lock().unwrap();
            for frame in 0..25u64 {
                segmenter.push(
                    &[0, 0, 0, 1, 0x65, 1],
                    Duration::from_millis(frame * 100),
                    frame % 10 == 0,
                );
            }
        }
        let server = HttpServer::start(segmenter).unwrap();
        let prefix = format!("/{}", server.token());
        let playlist = get(server.port(), &format!("{prefix}/live.m3u8"));
        assert!(playlist.starts_with("HTTP/1.1 200 OK\r\n"), "{playlist}");
        assert!(playlist.contains("Access-Control-Allow-Origin: *"));
        assert!(playlist.contains("seg1.ts"));
        let segment = get(server.port(), &format!("{prefix}/seg1.ts"));
        assert!(segment.contains("Content-Type: video/mp2t"));
        assert!(get(server.port(), "/live.m3u8").starts_with("HTTP/1.1 404"));
        assert!(get(server.port(), &format!("{prefix}/seg9.ts")).starts_with("HTTP/1.1 404"));
    }
}
