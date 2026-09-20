//! An owned loopback listener relaying the worker desktop through OpenSSH.
mod relay;

use super::{Cancellation, Error, Result, ssh::Connection};
use std::{
    io::{self, Read},
    net::{SocketAddr, TcpListener, TcpStream},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const READY_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CONNECTIONS: usize = 4;

#[derive(Debug)]
pub struct DesktopTunnel {
    pub endpoint: SocketAddr,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
}

impl DesktopTunnel {
    /// # Errors
    /// Fails if the owned listener cannot bind or the worker desktop is unavailable.
    pub fn open(connection: &Connection, cancel: &Cancellation) -> Result<Self> {
        cancel.check()?;
        let connection = connection.clone();
        let tunnel = Self::listen(move || {
            let mut command = Command::new("ssh");
            command.args(["-T", "-W", "127.0.0.1:5900"]).args(connection.args());
            command
        })?;
        wait_ready(tunnel.endpoint, cancel, READY_TIMEOUT)?;
        Ok(tunnel)
    }

    fn listen(launch: impl Fn() -> Command + Send + 'static) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let endpoint = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        // Keep this listener for the full lifetime; another service cannot take its endpoint.
        let server = thread::Builder::new()
            .name("cloud-desktop-relay".into())
            .spawn(move || {
                let mut relays = Vec::<relay::Relay>::new();
                while !stopped.load(Ordering::Acquire) {
                    relays.retain(|relay| !relay.finished());
                    match listener.accept() {
                        Ok((socket, _)) if relays.len() < MAX_CONNECTIONS => {
                            if let Ok(relay) = relay::Relay::start(socket, launch()) {
                                relays.push(relay);
                            }
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
                // Drop joins each transport after closing its socket and child pipes.
                drop(relays);
            })?;
        Ok(Self {
            endpoint,
            stop,
            server: Some(server),
        })
    }
}

fn remaining(deadline: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
}

fn wait_ready(endpoint: SocketAddr, cancel: &Cancellation, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while let Some(left) = remaining(deadline) {
        cancel.check()?;
        if let Ok(mut socket) = TcpStream::connect_timeout(&endpoint, left.min(POLL_INTERVAL)) {
            let mut banner = [0; 12];
            let mut received = 0;
            while let Some(left) = remaining(deadline) {
                cancel.check()?;
                socket.set_read_timeout(Some(left.min(POLL_INTERVAL)))?;
                match socket.read(&mut banner[received..]) {
                    Ok(0) => break,
                    Ok(count) => {
                        received += count;
                        if received == banner.len() {
                            if banner.starts_with(b"RFB ") {
                                return Ok(());
                            }
                            break;
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
                        ) => {}
                    Err(_) => break,
                }
            }
        }
        if let Some(left) = remaining(deadline) {
            thread::sleep(left.min(POLL_INTERVAL));
        }
    }
    Err(Error::Invalid("Worker desktop did not become ready"))
}

impl Drop for DesktopTunnel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

#[cfg(test)]
mod tests;
