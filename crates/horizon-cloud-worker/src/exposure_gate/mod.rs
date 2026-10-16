//! Default-off worker TCP gate. CIDRs use only the original accepted socket peer.
//! Configuration is trusted-root input, not owner approval or authenticated identity.
mod relay;
mod state;
use horizon_cloud_protocol::exposure::{
    EXPOSURE_LEASE_SECONDS, ExposureMode, ExposureRequest, RESERVED_INGRESS_END, RESERVED_INGRESS_START,
};
use socket2::{Domain, Protocol, Socket, Type};
use state::{Shared, State, lock};
use std::{
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
    task::{JoinHandle, JoinSet},
    time::Instant,
};

const MAX_SOURCES: usize = 256;
const BUFFER_BYTES: usize = 8 * 1024;
const SOCKET_BUFFER_BYTES: usize = 16 * 1024;
const MAX_SOCKET_BUFFER_BYTES: usize = 64 * 1024;
const MAX_GATE_LIFETIME: Duration = Duration::from_secs(EXPOSURE_LEASE_SECONDS);

#[derive(Clone, Copy, Debug)]
pub struct Budgets {
    pub gate_bytes: u64,
    pub source_bytes: u64,
    pub connection_bytes: u64,
    pub connect_timeout: Duration,
    pub idle_timeout: Duration,
    pub connection_lifetime: Duration,
    pub gate_lifetime: Duration,
}
impl Default for Budgets {
    fn default() -> Self {
        Self {
            gate_bytes: 1024 * 1024 * 1024,
            source_bytes: 256 * 1024 * 1024,
            connection_bytes: 16 * 1024 * 1024,
            connect_timeout: Duration::from_secs(3),
            idle_timeout: Duration::from_secs(60),
            connection_lifetime: Duration::from_mins(15),
            gate_lifetime: MAX_GATE_LIFETIME,
        }
    }
}
impl Budgets {
    fn validate(self) -> Result<Self, Error> {
        if self.connection_bytes == 0
            || self.connection_bytes > self.source_bytes
            || self.source_bytes > self.gate_bytes
            || self.gate_bytes > 64 * 1024 * 1024 * 1024
            || self.connect_timeout.is_zero()
            || self.connect_timeout > Duration::from_secs(3)
            || self.idle_timeout.is_zero()
            || self.idle_timeout > Duration::from_secs(60)
            || self.connection_lifetime.is_zero()
            || self.connection_lifetime > Duration::from_mins(15)
            || self.gate_lifetime.is_zero()
            || self.gate_lifetime > MAX_GATE_LIFETIME
        {
            return Err(Error::Budgets);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Counters {
    pub active_connections: usize,
    pub recorded_sources: usize,
    pub refused: u64,
    pub forwarded_bytes: u64,
    pub charged_bytes: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Gate configuration requires the trusted worker root controller")]
    Root,
    #[error("Gate is off; no listener was created")]
    Off,
    #[error("Gate ingress port is outside the reserved pool")]
    IngressPort,
    #[error("Invalid bounded TCP gate budgets")]
    Budgets,
    #[error("Gate lifetime cannot be represented")]
    Clock,
    #[error("Gate task failed before a clean close acknowledgement")]
    Task,
    #[error(transparent)]
    Policy(#[from] horizon_cloud_protocol::exposure::Error),
    #[error("TCP gate transport failed")]
    Io(#[from] io::Error),
}

/// One immutable gate generation. There is no renewal or policy-mutation API.
pub struct Gate {
    shared: Shared,
    cancel: watch::Sender<bool>,
    task: Option<JoinHandle<io::Result<()>>>,
}
impl Gate {
    /// Bind both wildcard families at an exclusively owned reserved ingress port.
    /// The target is always the literal worker loopback, never DNS or request metadata.
    /// Call inside an I/O- and timer-enabled Tokio runtime.
    ///
    /// # Errors
    /// Refuses unsupported policy before binding, non-root controllers and any bind failure.
    pub fn bind(port: u16, request: &ExposureRequest, budgets: Budgets) -> Result<Self, Error> {
        let policy = request.validate()?;
        let budgets = budgets.validate()?;
        if policy.mode() == ExposureMode::Off {
            return Err(Error::Off);
        }
        if policy.mode() == ExposureMode::TailnetOnly || policy.has_named_sources() {
            return Err(horizon_cloud_protocol::exposure::Error::IdentityUnavailable.into());
        }
        if !rustix::process::geteuid().is_root() {
            return Err(Error::Root);
        }
        if !(RESERVED_INGRESS_START..=RESERVED_INGRESS_END).contains(&port) {
            return Err(Error::IngressPort);
        }
        let listeners = vec![
            bind((Ipv4Addr::UNSPECIFIED, port).into())?,
            bind((Ipv6Addr::UNSPECIFIED, port).into())?,
        ];
        Self::owned(listeners, policy, budgets)
    }

    fn owned(
        listeners: Vec<Arc<TcpListener>>,
        policy: horizon_cloud_protocol::exposure::ExposurePolicy,
        budgets: Budgets,
    ) -> Result<Self, Error> {
        let until = Instant::now().checked_add(budgets.gate_lifetime).ok_or(Error::Clock)?;
        let shared = Arc::new(Mutex::new(State::new(listeners.clone(), policy, budgets, until)));
        let (cancel, watching) = watch::channel(false);
        let running = Arc::clone(&shared);
        let task = tokio::spawn(run(running, listeners, watching, until));
        Ok(Self {
            shared,
            cancel,
            task: Some(task),
        })
    }

    /// Stop admission and physically shut existing sockets before waiting for any task.
    pub fn cancel(&self) {
        lock(&self.shared).stop();
        let _ = self.cancel.send(true);
    }

    /// Acknowledge only after the listeners and every tracked flow task have closed.
    ///
    /// # Errors
    /// A failed task or listener produces no clean-close acknowledgement.
    pub async fn close(mut self) -> Result<(), Error> {
        self.cancel();
        if let Some(task) = self.task.take() {
            task.await.map_err(|_| Error::Task)??;
        }
        Ok(())
    }

    #[must_use]
    pub fn counters(&self) -> Counters {
        lock(&self.shared).counters()
    }
}
impl Drop for Gate {
    fn drop(&mut self) {
        self.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn bind(address: SocketAddr) -> io::Result<Arc<TcpListener>> {
    let socket = Socket::new(Domain::for_address(address), Type::STREAM, Some(Protocol::TCP))?;
    bounded_socket(&socket)?;
    socket.set_reuse_address(false)?;
    if address.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    socket.bind(&address.into())?;
    socket.listen(128)?;
    socket.set_nonblocking(true)?;
    Ok(Arc::new(TcpListener::from_std(socket.into())?))
}

fn bounded_socket(socket: &Socket) -> io::Result<()> {
    socket.set_recv_buffer_size(SOCKET_BUFFER_BYTES)?;
    socket.set_send_buffer_size(SOCKET_BUFFER_BYTES)?;
    if socket.recv_buffer_size()? > MAX_SOCKET_BUFFER_BYTES || socket.send_buffer_size()? > MAX_SOCKET_BUFFER_BYTES {
        return Err(io::Error::other("TCP socket buffer bound is unavailable"));
    }
    Ok(())
}

async fn accept(listeners: &[Arc<TcpListener>]) -> io::Result<TcpStream> {
    let accepted = if listeners.len() == 1 {
        listeners[0].accept().await
    } else {
        tokio::select! { result = listeners[0].accept() => result, result = listeners[1].accept() => result }
    };
    accepted.map(|(stream, _)| stream)
}

async fn run(
    shared: Shared,
    listeners: Vec<Arc<TcpListener>>,
    mut cancel: watch::Receiver<bool>,
    until: Instant,
) -> io::Result<()> {
    let mut flows = JoinSet::new();
    let mut failure = None;
    loop {
        tokio::select! {
            biased;
            _ = cancel.changed() => break,
            () = tokio::time::sleep_until(until) => break,
            result = flows.join_next(), if !flows.is_empty() => {
                if result.is_some_and(|result| result.is_err()) {
                    failure = Some(io::Error::other("Gate flow task failed"));
                    break;
                }
            },
            result = accept(&listeners) => match result {
                Ok(client) => {
                    let client = Arc::new(client);
                    let id = lock(&shared).admit(Arc::clone(&client));
                    if let Some(id) = id { relay::spawn(&mut flows, Arc::clone(&shared), id, client, cancel.clone()); }
                },
                Err(error) => { failure = Some(error); break; },
            },
        }
        tokio::task::yield_now().await;
    }
    lock(&shared).stop();
    flows.abort_all();
    while flows.join_next().await.is_some() {}
    lock(&shared).listeners.clear();
    drop(listeners);
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests;
