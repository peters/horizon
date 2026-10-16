use crate::{
    Error, Result,
    cancellation::{self, Cancellation, Registration},
    crypto,
};
use ring::aead;
use std::{
    collections::BTreeMap,
    io::{ErrorKind, Read, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const MESSAGE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct Transport {
    socket: TcpStream,
    cancellation: Option<Registration>,
    closed: Arc<AtomicBool>,
    security: Option<Security>,
    pending: Vec<u8>,
    offset: usize,
    sequence: u32,
    event_idle: bool,
    read_deadline: Option<Instant>,
    message_timeout: Duration,
}
struct Security {
    send: aead::LessSafeKey,
    receive: aead::LessSafeKey,
    send_counter: u64,
    receive_counter: u64,
}
/// Ends a transport from another thread, also while it waits for a message.
pub(crate) struct Closer {
    socket: TcpStream,
    closed: Arc<AtomicBool>,
}
impl Closer {
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}
pub(crate) struct Response {
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) body: Vec<u8>,
}
impl Transport {
    #[cfg(test)]
    pub(crate) fn connect(address: SocketAddr) -> Result<Self> {
        Self::connect_cancellable(address, None)
    }
    pub(crate) fn connect_cancellable(address: SocketAddr, cancellation: Option<Arc<Cancellation>>) -> Result<Self> {
        let socket = match &cancellation {
            Some(cancel) => cancel.connect(address)?,
            None => TcpStream::connect_timeout(&address, Duration::from_secs(5))?,
        };
        socket.set_read_timeout(Some(Duration::from_secs(5)))?;
        socket.set_write_timeout(Some(Duration::from_secs(5)))?;
        socket.set_nodelay(true)?;
        let cancellation = cancellation.map(|cancel| cancel.register(&socket)).transpose()?;
        Ok(Self {
            socket,
            cancellation,
            closed: Arc::new(AtomicBool::new(false)),
            security: None,
            pending: Vec::new(),
            offset: 0,
            sequence: 0,
            event_idle: false,
            read_deadline: None,
            message_timeout: MESSAGE_TIMEOUT,
        })
    }
    pub(crate) fn cancellation(&self) -> Option<Arc<Cancellation>> {
        self.cancellation
            .as_ref()
            .map(|registration| registration.cancellation().clone())
    }
    pub(crate) fn complete_setup(&mut self) -> Result<()> {
        if let Some(cancel) = &self.cancellation {
            cancel.cancellation().release()?;
        }
        self.cancellation = None;
        Ok(())
    }
    pub(crate) fn local_ip(&self) -> Result<String> {
        Ok(self.socket.local_addr()?.ip().to_string())
    }
    pub(crate) fn wait_for_events(&mut self) {
        self.event_idle = true;
    }
    pub(crate) fn closer(&self) -> Result<Closer> {
        Ok(Closer {
            socket: self.socket.try_clone()?,
            closed: self.closed.clone(),
        })
    }
    pub(crate) fn encrypt(&mut self, secret: &[u8], events: bool) -> Result<()> {
        let (salt, send, receive) = if events {
            (
                "Events-Salt",
                "Events-Read-Encryption-Key",
                "Events-Write-Encryption-Key",
            )
        } else {
            (
                "Control-Salt",
                "Control-Write-Encryption-Key",
                "Control-Read-Encryption-Key",
            )
        };
        self.security = Some(Security {
            send: crypto::key(crypto::derive(secret, salt, send)?.as_ref())?,
            receive: crypto::key(crypto::derive(secret, salt, receive)?.as_ref())?,
            send_counter: 0,
            receive_counter: 0,
        });
        Ok(())
    }
    pub(crate) fn request(&mut self, method: &str, path: &str, headers: &str, body: &[u8]) -> Result<Response> {
        if let Some(cancel) = &self.cancellation {
            cancel.cancellation().check()?;
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(Error::Protocol("request sequence exhausted"))?;
        let protocol = if matches!(method, "SETUP" | "RECORD" | "TEARDOWN" | "SETPEERS") {
            "RTSP/1.0"
        } else {
            "HTTP/1.1"
        };
        let header = format!(
            "{method} {path} {protocol}\r\nUser-Agent: AirPlay/890.1\r\nCSeq: {}\r\nContent-Length: {}\r\n{headers}\r\n",
            self.sequence,
            body.len()
        );
        let mut message = header.into_bytes();
        message.extend(body);
        self.send(&message)?;
        self.response()
    }
    pub(crate) fn send(&mut self, bytes: &[u8]) -> Result<()> {
        if let Some(security) = &mut self.security {
            for chunk in bytes.chunks(1024) {
                let next = security
                    .send_counter
                    .checked_add(1)
                    .ok_or(Error::Protocol("nonce exhausted"))?;
                let length = u16::try_from(chunk.len())
                    .map_err(|_| Error::Protocol("frame too large"))?
                    .to_le_bytes();
                let ciphertext = crypto::seal(
                    &security.send,
                    security.send_counter.to_le_bytes(),
                    &length,
                    chunk.to_vec(),
                )?;
                security.send_counter = next;
                self.socket.write_all(&length)?;
                self.socket.write_all(&ciphertext)?;
            }
        } else {
            self.socket.write_all(bytes)?;
        }
        Ok(())
    }
    fn byte(&mut self) -> Result<u8> {
        let mut byte = [0];
        self.read_exact(&mut byte)?;
        Ok(byte[0])
    }
    pub(crate) fn read_exact(&mut self, mut output: &mut [u8]) -> Result<()> {
        if self.security.is_none() {
            return self.socket_read_exact(output);
        }
        while !output.is_empty() {
            if self.offset == self.pending.len() {
                let mut length = [0; 2];
                self.socket_read_exact(&mut length)?;
                let count = usize::from(u16::from_le_bytes(length));
                if !(1..=1024).contains(&count) {
                    return Err(Error::Protocol("invalid encrypted frame length"));
                }
                let mut encrypted = vec![0; count + 16];
                self.socket_read_exact(&mut encrypted)?;
                let security = self.security.as_mut().ok_or(Error::Authentication)?;
                let next = security
                    .receive_counter
                    .checked_add(1)
                    .ok_or(Error::Protocol("nonce exhausted"))?;
                self.pending = crypto::open(
                    &security.receive,
                    security.receive_counter.to_le_bytes(),
                    &length,
                    encrypted,
                )?;
                security.receive_counter = next;
                self.offset = 0;
            }
            if self.event_idle && self.read_deadline.is_none() {
                self.read_deadline = Some(Instant::now() + self.message_timeout);
            }
            let count = output.len().min(self.pending.len() - self.offset);
            output[..count].copy_from_slice(&self.pending[self.offset..self.offset + count]);
            self.offset += count;
            output = &mut output[count..];
        }
        Ok(())
    }
    /// A closed transport, or a cancelled setup, ends a read with that reason
    /// instead of the socket error that the shutdown caused.
    fn interrupted(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(std::io::Error::new(ErrorKind::ConnectionAborted, "receiver connection closed").into());
        }
        match &self.cancellation {
            Some(registration) if !self.event_idle => registration.cancellation().check(),
            _ => Ok(()),
        }
    }
    fn socket_read_exact(&mut self, mut output: &mut [u8]) -> Result<()> {
        while !output.is_empty() {
            // On Windows a socket shutdown does not wake a blocked read, so a read
            // waits in short slices and checks for cancellation between them.
            let slice = match self.read_deadline {
                Some(deadline) => deadline
                    .checked_duration_since(Instant::now())
                    .filter(|remaining| !remaining.is_zero())
                    .ok_or_else(|| std::io::Error::new(ErrorKind::TimedOut, "receiver message deadline exceeded"))?
                    .min(cancellation::POLL),
                None => cancellation::POLL,
            };
            self.socket.set_read_timeout(Some(slice))?;
            let count = match self.socket.read(output) {
                Ok(0) => {
                    self.interrupted()?;
                    return Err(std::io::Error::from(ErrorKind::UnexpectedEof).into());
                }
                Ok(count) => count,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    self.interrupted()?;
                    continue;
                }
                Err(error) => {
                    self.interrupted()?;
                    return Err(error.into());
                }
            };
            // Event sockets may idle indefinitely; a message becomes bounded once bytes arrive.
            if self.event_idle && self.read_deadline.is_none() {
                self.read_deadline = Some(Instant::now() + self.message_timeout);
            }
            output = &mut output[count..];
        }
        Ok(())
    }
    pub(crate) fn message(&mut self) -> Result<(String, Response)> {
        self.message_with_timeout(MESSAGE_TIMEOUT)
    }
    fn message_with_timeout(&mut self, timeout: Duration) -> Result<(String, Response)> {
        self.message_timeout = timeout;
        self.read_deadline = (!self.event_idle).then(|| Instant::now() + timeout);
        let result = self.message_inner();
        self.read_deadline = None;
        result
    }
    fn message_inner(&mut self) -> Result<(String, Response)> {
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            if header.len() >= 16384 {
                return Err(Error::Protocol("headers too large"));
            }
            header.push(self.byte()?);
        }
        let text = std::str::from_utf8(&header).map_err(|_| Error::Protocol("invalid header encoding"))?;
        let mut lines = text.split("\r\n");
        let first = lines.next().ok_or(Error::Protocol("missing response"))?.to_owned();
        let mut headers = BTreeMap::new();
        for line in lines.filter(|line| !line.is_empty()) {
            let (name, value) = line.split_once(':').ok_or(Error::Protocol("invalid header"))?;
            if headers
                .insert(name.trim().to_ascii_lowercase(), value.trim().to_owned())
                .is_some()
            {
                return Err(Error::Protocol("duplicate header"));
            }
        }
        if headers.contains_key("transfer-encoding") {
            return Err(Error::Protocol("unsupported transfer encoding"));
        }
        let count = headers.get("content-length").map_or(Ok(0), |value| {
            value
                .parse::<usize>()
                .map_err(|_| Error::Protocol("invalid content length"))
        })?;
        if count > 1024 * 1024 {
            return Err(Error::Protocol("body too large"));
        }
        let mut body = vec![0; count];
        self.read_exact(&mut body)?;
        Ok((first, Response { headers, body }))
    }
    fn response(&mut self) -> Result<Response> {
        let (first, response) = self.message()?;
        let mut parts = first.split_whitespace();
        if !matches!(parts.next(), Some("RTSP/1.0" | "HTTP/1.1" | "HTTP/1.0")) {
            return Err(Error::Protocol("invalid response protocol"));
        }
        let status = parts
            .next()
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or(Error::Protocol("invalid status"))?;
        if !(200..300).contains(&status) {
            return Err(Error::Status(status));
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, thread};

    fn served(bytes: Vec<u8>, encrypted: bool) -> (Transport, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
        let mut transport = Transport::connect(listener.local_addr().expect("address")).expect("connect");
        if encrypted {
            transport.encrypt(&[9; 32], false).expect("keys");
        }
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream.set_write_timeout(Some(Duration::from_secs(2))).expect("timeout");
            // Small writes exercise TCP fragmentation independently of HAP boundaries.
            for chunk in bytes.chunks(7) {
                if stream.write_all(chunk).is_err() {
                    break;
                }
            }
        });
        (transport, worker)
    }
    fn receiver_frames(plain: &[u8]) -> Vec<u8> {
        let key = crypto::key(
            crypto::derive(&[9; 32], "Control-Salt", "Control-Read-Encryption-Key")
                .expect("derive")
                .as_ref(),
        )
        .expect("key");
        let mut bytes = Vec::new();
        for (counter, chunk) in plain.chunks(1024).enumerate() {
            let length = u16::try_from(chunk.len()).expect("length").to_le_bytes();
            bytes.extend(length);
            bytes.extend(
                crypto::seal(
                    &key,
                    u64::try_from(counter).expect("counter").to_le_bytes(),
                    &length,
                    chunk.to_vec(),
                )
                .expect("encrypt"),
            );
        }
        bytes
    }
    #[test]
    fn cancellation_handoff_preserves_established_teardown() {
        use std::sync::atomic::AtomicBool;
        for established in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
            let cancel = Arc::new(Cancellation::new(Arc::new(AtomicBool::new(false))));
            let mut transport =
                Transport::connect_cancellable(listener.local_addr().expect("address"), Some(cancel.clone()))
                    .expect("connect");
            let (mut receiver, _) = listener.accept().expect("accept");
            receiver
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("timeout");
            if established {
                transport.complete_setup().expect("release setup handle");
            }
            cancel.stop();
            if established {
                let server = thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut byte = [0];
                    while !request.ends_with(b"\r\n\r\n") {
                        receiver.read_exact(&mut byte).expect("teardown request");
                        request.push(byte[0]);
                    }
                    assert!(request.starts_with(b"TEARDOWN /stream RTSP/1.0"));
                    receiver
                        .write_all(b"RTSP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n")
                        .expect("teardown reply");
                });
                transport
                    .request("TEARDOWN", "/stream", "", &[])
                    .expect("established teardown survives cancellation");
                server.join().expect("server");
            } else {
                assert!(transport.complete_setup().is_err());
                assert!(transport.request("POST", "/pair-verify", "", &[]).is_err());
                assert_eq!(receiver.read(&mut [0]).expect("closed setup socket"), 0);
            }
        }
    }

    // The receiver keeps its side open in both tests: on Windows a shutdown alone
    // does not wake the read, so only the sliced wait can end it before its timeout.
    #[test]
    fn cancellation_ends_a_response_read_while_the_receiver_stays_open() {
        use std::sync::atomic::AtomicBool;
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let cancel = Arc::new(Cancellation::new(Arc::new(AtomicBool::new(false))));
        let mut transport =
            Transport::connect_cancellable(listener.local_addr().expect("address"), Some(cancel.clone()))
                .expect("connect");
        let (mut receiver, _) = listener.accept().expect("accept");
        let worker = thread::spawn(move || transport.request("SETUP", "/stream", "", &[]).err());
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            receiver.read_exact(&mut byte).expect("request");
            request.push(byte[0]);
        }
        cancel.stop();
        let error = worker.join().expect("worker");
        assert!(matches!(error, Some(Error::Protocol("casting cancelled"))), "{error:?}");
        drop(receiver);
    }

    #[test]
    fn closing_ends_an_idle_event_read_while_the_receiver_stays_open() {
        use std::sync::mpsc;
        let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
        let mut transport = Transport::connect(listener.local_addr().expect("address")).expect("connect");
        let (receiver, _) = listener.accept().expect("accept");
        transport.wait_for_events();
        let closer = transport.closer().expect("closer");
        let (send, ended) = mpsc::channel();
        thread::spawn(move || send.send(transport.message().err()));
        // Gives the read time to block; closing before it starts must also end it.
        thread::sleep(Duration::from_millis(100));
        closer.close();
        // Only guards against a hang: an idle event read has no deadline of its own.
        let error = ended
            .recv_timeout(Duration::from_secs(60))
            .expect("idle event read ended");
        assert!(
            matches!(&error, Some(Error::Io(error)) if error.kind() == std::io::ErrorKind::ConnectionAborted),
            "{error:?}"
        );
        drop(receiver);
    }

    #[test]
    fn reads_fragmented_encrypted_messages_across_frames() {
        let mut plain = b"HTTP/1.1 200 OK\r\nContent-Length: 2050\r\n\r\n".to_vec();
        plain.extend([42; 2050]);
        plain.extend(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        let (mut transport, worker) = served(receiver_frames(&plain), true);
        assert_eq!(transport.response().expect("response").body, [42; 2050]);
        assert!(transport.response().expect("second response").body.is_empty());
        worker.join().expect("worker");
    }
    #[test]
    fn rejects_tampering_without_exposing_plaintext() {
        let mut encrypted = receiver_frames(b"private response");
        let last = encrypted.last_mut().expect("tag");
        *last ^= 1;
        let (mut transport, worker) = served(encrypted, true);
        let mut output = [123; 16];
        assert!(matches!(transport.read_exact(&mut output), Err(Error::Authentication)));
        assert_eq!(output, [123; 16]);
        worker.join().expect("worker");
    }
    #[test]
    fn buffered_partial_event_starts_its_own_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
        let mut transport = Transport::connect(listener.local_addr().expect("address")).expect("connect");
        transport.encrypt(&[9; 32], true).expect("event encryption");
        transport.wait_for_events();
        let worker = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            let plain = b"POST /event RTSP/1.0\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\nPOST ";
            let length = u16::try_from(plain.len()).expect("length").to_le_bytes();
            let key = crypto::key(
                crypto::derive(&[9; 32], "Events-Salt", "Events-Write-Encryption-Key")
                    .expect("derive")
                    .as_ref(),
            )
            .expect("key");
            socket.write_all(&length).expect("length");
            socket
                .write_all(&crypto::seal(&key, 0u64.to_le_bytes(), &length, plain.to_vec()).expect("encrypted events"))
                .expect("events");
            thread::sleep(Duration::from_millis(500));
        });
        transport
            .message_with_timeout(Duration::from_millis(50))
            .expect("first event");
        assert!(transport.offset < transport.pending.len());
        let started = Instant::now();
        let error = transport
            .message_with_timeout(Duration::from_millis(50))
            .err()
            .expect("partial event deadline");
        assert!(
            matches!(error, Error::Io(error) if matches!(error.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock))
        );
        assert!(started.elapsed() < Duration::from_millis(400));
        worker.join().expect("worker");
    }

    #[test]
    fn event_socket_may_idle_before_the_next_message() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
        let mut transport = Transport::connect(listener.local_addr().expect("address")).expect("connect");
        transport.wait_for_events();
        let worker = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            thread::sleep(Duration::from_millis(150));
            socket
                .write_all(b"POST /event RTSP/1.0\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n")
                .expect("event");
        });
        let (request, response) = transport
            .message_with_timeout(Duration::from_millis(50))
            .expect("idle event");
        assert_eq!(request, "POST /event RTSP/1.0");
        assert!(response.body.is_empty());
        worker.join().expect("worker");
    }

    #[test]
    fn slow_partial_headers_cannot_extend_the_message_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
        let mut transport = Transport::connect(listener.local_addr().expect("address")).expect("connect");
        let worker = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            for byte in b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n" {
                if socket.write_all(&[*byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        let started = Instant::now();
        let error = transport
            .message_with_timeout(Duration::from_millis(120))
            .err()
            .expect("deadline");
        assert!(
            matches!(error, Error::Io(error) if matches!(error.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock))
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(transport);
        worker.join().expect("worker");
    }

    #[test]
    fn rejects_oversized_and_ambiguous_responses() {
        for response in [
            "HTTP/1.1 200 OK\r\nContent-Length: 1048577\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\ncontent-length: 1\r\n\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort",
            "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n",
        ] {
            let (mut transport, worker) = served(response.as_bytes().to_vec(), false);
            assert!(transport.response().is_err());
            worker.join().expect("worker");
        }
    }
}
