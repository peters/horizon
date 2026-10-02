use crate::{Error, Result, session::lock};
use std::{
    io::ErrorKind,
    net::{Shutdown, SocketAddr, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_POLL: Duration = Duration::from_millis(100);

#[derive(Default)]
struct Sockets {
    next: usize,
    connected: Vec<(usize, TcpStream)>,
}
pub(crate) struct Cancellation {
    stopped: Arc<AtomicBool>,
    sockets: Mutex<Sockets>,
}
pub(crate) struct Registration {
    cancellation: Arc<Cancellation>,
    id: usize,
}
impl Registration {
    pub(crate) fn cancellation(&self) -> &Arc<Cancellation> {
        &self.cancellation
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        lock(&self.cancellation.sockets)
            .connected
            .retain(|(id, _)| *id != self.id);
    }
}
impl Cancellation {
    pub(crate) fn new(stopped: Arc<AtomicBool>) -> Self {
        Self {
            stopped,
            sockets: Mutex::new(Sockets::default()),
        }
    }
    pub(crate) fn check(&self) -> Result<()> {
        if self.stopped.load(Ordering::Relaxed) {
            Err(Error::Protocol("casting cancelled"))
        } else {
            Ok(())
        }
    }
    pub(crate) fn connect(&self, address: SocketAddr) -> Result<TcpStream> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        loop {
            self.check()?;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| std::io::Error::new(ErrorKind::TimedOut, "receiver connection deadline exceeded"))?;
            match TcpStream::connect_timeout(&address, remaining.min(CONNECT_POLL)) {
                Ok(socket) => {
                    self.check()?;
                    return Ok(socket);
                }
                Err(error) if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    pub(crate) fn register(self: &Arc<Self>, socket: &TcpStream) -> Result<Registration> {
        let mut sockets = lock(&self.sockets);
        self.check()?;
        let id = sockets.next;
        sockets.next = id.checked_add(1).ok_or(Error::Protocol("socket identity exhausted"))?;
        sockets.connected.push((id, socket.try_clone()?));
        Ok(Registration {
            cancellation: self.clone(),
            id,
        })
    }
    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        for (_, socket) in &lock(&self.sockets).connected {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
    pub(crate) fn release(&self) -> Result<()> {
        let mut sockets = lock(&self.sockets);
        self.check()?;
        // Established streaming owns normal TEARDOWN; setup cancellation must no longer close its sockets.
        sockets.connected.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    fn connected() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let socket = TcpStream::connect(listener.local_addr().expect("address")).expect("connect");
        let (peer, _) = listener.accept().expect("accept");
        peer.set_read_timeout(Some(Duration::from_millis(100)))
            .expect("timeout");
        (socket, peer)
    }
    #[test]
    fn dropping_one_registration_does_not_remove_sibling_setup_sockets() {
        let cancel = Arc::new(Cancellation::new(Arc::new(AtomicBool::new(false))));
        let (first, mut first_peer) = connected();
        let (second, mut second_peer) = connected();
        let (third, mut third_peer) = connected();
        let first_guard = cancel.register(&first).expect("first");
        let _second_guard = cancel.register(&second).expect("second");
        let _third_guard = cancel.register(&third).expect("third");
        drop(first_guard);
        cancel.stop();
        assert_eq!(second_peer.read(&mut [0]).expect("second closed"), 0);
        assert_eq!(third_peer.read(&mut [0]).expect("third closed"), 0);
        assert!(first_peer.read(&mut [0]).is_err(), "unregistered socket remains open");
        assert!(cancel.register(&first).is_err());
        drop(first);
        assert_eq!(first_peer.read(&mut [0]).expect("caller dropped rejected socket"), 0);
    }
    #[test]
    fn release_preserves_all_established_sockets_and_cancelled_dial_is_rejected() {
        let cancel = Arc::new(Cancellation::new(Arc::new(AtomicBool::new(false))));
        let (mut first, mut first_peer) = connected();
        let (mut second, mut second_peer) = connected();
        let _first_guard = cancel.register(&first).expect("first");
        let second_guard = cancel.register(&second).expect("second");
        cancel.release().expect("established handoff");
        drop(second_guard);
        cancel.stop();
        first.write_all(&[1]).expect("first remains usable");
        second.write_all(&[2]).expect("second remains usable");
        let mut byte = [0];
        first_peer.read_exact(&mut byte).expect("first data");
        assert_eq!(byte, [1]);
        second_peer.read_exact(&mut byte).expect("second data");
        assert_eq!(byte, [2]);
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        assert!(cancel.connect(listener.local_addr().expect("address")).is_err());
    }
}
