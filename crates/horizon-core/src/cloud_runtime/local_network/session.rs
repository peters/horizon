//! The one SSH process per bridge: it forwards the worker's bridge socket to the local proxy
//! and keeps the worker helper alive with a heartbeat. It is restarted with backoff while on.
use super::{Shared, State};
use crate::cloud_runtime::{Cancellation, command::Runner, ssh::Connection};
use horizon_cloud_protocol::local_network::{HEARTBEAT_INTERVAL, Nonce, PREPARE_COMMAND, PREPARED, Ready, Subnet};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex, PoisonError, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const PREPARE_TIMEOUT: Duration = Duration::from_secs(40);
const READY_TIMEOUT: Duration = Duration::from_secs(40);
const POLL: Duration = Duration::from_millis(100);
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LAST_RETRY: Duration = Duration::from_secs(30);
/// A session that stayed up this long starts the next retry from [`FIRST_RETRY`] again.
const STABLE: Duration = Duration::from_secs(60);
const MAX_LINE: u64 = 4096;
pub(super) const UNSUPPORTED: &str =
    "This cloud's worker image does not support Local Network Bridge yet; rebuild the image";
const CLOSED: &str = "The bridge connection to the worker closed";
const UNCONFIRMED: &str = "The worker did not confirm the bridge";

pub(super) trait Transport: Send + 'static {
    /// Must print [`PREPARED`] on a worker that supports the bridge.
    fn prepare(&self) -> Command;
    /// Holds one session: the worker's bridge socket forwarded to `127.0.0.1:proxy_port`
    /// here, and the worker helper reading heartbeats from standard input.
    fn hold(&self, nonce: &Nonce, subnet: Subnet, proxy_port: u16) -> Command;
    fn heartbeat(&self) -> Duration {
        HEARTBEAT_INTERVAL
    }
}

/// The cloud's own pinned SSH connection: known host key only, no agent or X11 forwarding.
pub(super) struct Ssh(pub(super) Connection);

impl Transport for Ssh {
    fn prepare(&self) -> Command {
        self.0.pinned_command(PREPARE_COMMAND)
    }

    fn hold(&self, nonce: &Nonce, subnet: Subnet, proxy_port: u16) -> Command {
        let mut command = Command::new("ssh");
        // OpenSSH keeps the first value of an option, so these precede the connection's own.
        command
            .args(["-T", "-a", "-x", "-o", "ExitOnForwardFailure=yes", "-R"])
            .arg(format!("{}:127.0.0.1:{proxy_port}", nonce.bridge_socket()))
            .args(self.0.pinned_args())
            .arg(nonce.hold_command(subnet));
        command
    }
}

enum Ended {
    Cancelled,
    Unsupported,
    Lost(String),
}

pub(super) fn supervise(
    transport: &dyn Transport,
    subnet: Subnet,
    proxy_port: u16,
    shared: &Shared,
    cancel: &Cancellation,
) {
    let mut retry = FIRST_RETRY;
    while !cancel.is_cancelled() {
        let started = Instant::now();
        let error = match attempt(transport, subnet, proxy_port, shared, cancel) {
            Ended::Cancelled => return,
            Ended::Unsupported => {
                shared.set(State::Failed {
                    error: UNSUPPORTED.into(),
                });
                return;
            }
            Ended::Lost(error) => error,
        };
        tracing::info!(%error, "local network bridge session ended");
        shared.set(State::Reconnecting { error });
        if started.elapsed() >= STABLE {
            retry = FIRST_RETRY;
        }
        let resume = Instant::now() + retry;
        while Instant::now() < resume {
            if cancel.is_cancelled() {
                return;
            }
            thread::sleep(POLL);
        }
        retry = (retry * 2).min(LAST_RETRY);
    }
}

fn attempt(
    transport: &dyn Transport,
    subnet: Subnet,
    proxy_port: u16,
    shared: &Shared,
    cancel: &Cancellation,
) -> Ended {
    let runner = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let prepared = runner.run("Local network preparation", &mut transport.prepare(), PREPARE_TIMEOUT);
    if cancel.is_cancelled() {
        return Ended::Cancelled;
    }
    match prepared {
        Ok(output) if output.lines().any(|line| line.trim() == PREPARED) => {}
        Ok(_) => return Ended::Unsupported,
        Err(_) => return Ended::Lost("Cannot reach the worker over SSH".into()),
    }
    let nonce = Nonce::random();
    let mut command = transport.hold(&nonce, subnet, proxy_port);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut hold = match Hold::spawn(command) {
        Ok(hold) => hold,
        Err(error) => return Ended::Lost(format!("Cannot start SSH: {error}")),
    };
    let deadline = Instant::now() + READY_TIMEOUT;
    let ready = loop {
        if cancel.is_cancelled() {
            return Ended::Cancelled;
        }
        if let Some(ended) = hold.exited() {
            return ended;
        }
        match hold.ready.recv_timeout(POLL) {
            Ok(ready) => break ready,
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
            Err(mpsc::RecvTimeoutError::Timeout) => return Ended::Lost(UNCONFIRMED.into()),
            // Output ended without the ready line; the exit status explains why.
            Err(mpsc::RecvTimeoutError::Disconnected) => return hold.finish(),
        }
    };
    shared.set(State::Active { proxy: ready.proxy });
    let mut beat = Instant::now() + transport.heartbeat();
    loop {
        if cancel.is_cancelled() {
            return Ended::Cancelled;
        }
        if let Some(ended) = hold.exited() {
            return ended;
        }
        if Instant::now() >= beat {
            if hold.beat().is_err() {
                return hold.exited().unwrap_or_else(|| Ended::Lost(CLOSED.into()));
            }
            beat = Instant::now() + transport.heartbeat();
        }
        thread::sleep(POLL);
    }
}

/// The running SSH process, killed and reaped with its reader threads on drop.
struct Hold {
    child: Child,
    input: Option<ChildStdin>,
    ready: mpsc::Receiver<Ready>,
    last_error: Arc<Mutex<String>>,
    readers: Vec<JoinHandle<()>>,
}

impl Hold {
    fn spawn(mut command: Command) -> std::io::Result<Self> {
        let mut child = command.spawn()?;
        let input = child.stdin.take();
        let (sender, ready) = mpsc::channel();
        let last_error = Arc::new(Mutex::new(String::new()));
        let mut readers = Vec::with_capacity(2);
        if let Some(output) = child.stdout.take() {
            readers.push(spawn_reader("local-network-ready", output, move |line| {
                if let Ok(ready) = serde_json::from_str::<Ready>(line) {
                    let _ = sender.send(ready);
                }
            })?);
        }
        if let Some(errors) = child.stderr.take() {
            let last = Arc::clone(&last_error);
            readers.push(spawn_reader("local-network-errors", errors, move |line| {
                let line = line.trim();
                if !line.is_empty() {
                    *last.lock().unwrap_or_else(PoisonError::into_inner) = line.chars().take(300).collect();
                }
            })?);
        }
        Ok(Self {
            child,
            input,
            ready,
            last_error,
            readers,
        })
    }

    fn exited(&mut self) -> Option<Ended> {
        self.child.try_wait().ok()??;
        // Let the reader keep the process's final words before reporting them.
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
        let error = self.last_error.lock().unwrap_or_else(PoisonError::into_inner).clone();
        Some(Ended::Lost(if error.is_empty() { CLOSED.into() } else { error }))
    }

    /// Reports how the process ended, stopping it if it has not ended within a moment.
    fn finish(&mut self) -> Ended {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if let Some(ended) = self.exited() {
                return ended;
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ended::Lost(UNCONFIRMED.into())
    }

    fn beat(&mut self) -> std::io::Result<()> {
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| std::io::Error::other("SSH input unavailable"))?;
        input.write_all(b"\n")?;
        input.flush()
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        drop(self.input.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

fn spawn_reader(
    name: &str,
    stream: impl Read + Send + 'static,
    mut each: impl FnMut(&str) + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new().name(name.into()).spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            match (&mut reader).take(MAX_LINE).read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => each(&line),
            }
        }
    })
}
