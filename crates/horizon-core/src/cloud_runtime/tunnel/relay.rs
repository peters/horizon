//! One bidirectional VNC transport and its shutdown ownership.
use std::{
    io::{self, Read, Write},
    net::{Shutdown, TcpStream},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

struct Transport {
    socket: TcpStream,
    child: Mutex<Child>,
    finished: AtomicBool,
}

impl Transport {
    fn stop(&self) {
        let _ = self.socket.shutdown(Shutdown::Both);
        let mut child = self.child.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = child.kill();
        let _ = child.wait();
        self.finished.store(true, Ordering::Release);
    }
}

pub(super) struct Relay {
    transport: Arc<Transport>,
    copies: Vec<JoinHandle<()>>,
}

impl Relay {
    pub(super) fn start(socket: TcpStream, mut command: Command) -> io::Result<Self> {
        // Accepted sockets inherit the listener's nonblocking mode on Windows.
        socket.set_nonblocking(false)?;
        let mut incoming = socket.try_clone()?;
        let mut outgoing = socket.try_clone()?;
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let transport = Arc::new(Transport {
            socket,
            child: Mutex::new(child),
            finished: AtomicBool::new(false),
        });
        let mut relay = Self {
            transport,
            copies: Vec::with_capacity(2),
        };
        let (mut input, mut output) = {
            let mut child = relay
                .transport
                .child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let input = child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("Desktop SSH input unavailable"))?;
            let output = child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("Desktop SSH output unavailable"))?;
            (input, output)
        };
        let transport = relay.transport.clone();
        relay
            .copies
            .push(thread::Builder::new().name("cloud-vnc-input".into()).spawn(move || {
                let _ = forward(&mut incoming, &mut input);
                transport.stop();
            })?);
        let transport = relay.transport.clone();
        relay
            .copies
            .push(thread::Builder::new().name("cloud-vnc-output".into()).spawn(move || {
                let _ = forward(&mut output, &mut outgoing);
                transport.stop();
            })?);
        Ok(relay)
    }

    pub(super) fn finished(&self) -> bool {
        self.transport.finished.load(Ordering::Acquire)
    }
}

fn forward(input: &mut impl Read, output: &mut impl Write) -> io::Result<()> {
    let mut buffer = [0; 16 * 1024];
    loop {
        match input.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => output.write_all(&buffer[..count])?,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.transport.stop();
        for copy in self.copies.drain(..) {
            let _ = copy.join();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{net::TcpListener, time::Duration};

    #[test]
    fn nonblocking_accept_is_normalized_and_transport_drop_reaps_child() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        accepted.set_nonblocking(true).unwrap();
        let mut command = Command::new("cat");
        command.env_clear();
        let relay = Relay::start(accepted, command).unwrap();
        let transport = relay.transport.clone();
        thread::sleep(Duration::from_millis(150));
        client.write_all(b"ping").unwrap();
        let mut echo = [0; 4];
        client.read_exact(&mut echo).unwrap();
        assert_eq!(&echo, b"ping");
        assert!(!relay.finished());
        drop(relay);
        assert!(transport.finished.load(Ordering::Acquire));
        assert!(transport.child.lock().unwrap().try_wait().unwrap().is_some());
        assert_eq!(client.read(&mut [0]).unwrap(), 0);
    }
}
