//! Local Network Bridge: lets one cloud's worker open TCP connections to devices on the
//! network this computer is on, through a scope-checking proxy that runs here.
//!
//! [`Scope`] is the whole policy and the [`Proxy`] applies it: the worker only ever reaches
//! the proxy's loopback port. Only the owner starts a [`Bridge`], from the cloud card, and
//! nothing persists it; dropping it stops its SSH session and closes every relayed connection.
mod discovery;
mod scope;
mod session;
mod socks;

use super::{Cancellation, ssh::Connection};
pub use discovery::Discoverer;
use horizon_cloud_protocol::local_network::{
    Reply, Subnet,
    discovery::{Answer, Hello, Request},
};
pub use scope::ScopeError;
pub use socks::{BYTE_BUDGET, Counters, MAX_CONNECTIONS, Relay};
use std::{
    collections::BTreeSet,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// Resolved addresses tried for one hostname.
const MAX_ATTEMPTS: usize = 4;
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);
/// Name lookups running at once, including ones abandoned at their deadline.
const MAX_LOOKUPS: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error(transparent)]
    Scope(#[from] ScopeError),
    #[error("Local Network Bridge could not start: {0}")]
    Io(#[from] io::Error),
}

/// A requested destination, before any policy decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Address(SocketAddr),
    /// Resolved by [`Scope`] on this computer, never by the worker.
    Name(String, u16),
}

/// A SOCKS5 proxy on this computer's loopback that reaches only what its [`Scope`] admits.
pub struct Proxy {
    subnet: Subnet,
    inner: socks::Proxy,
}

impl Proxy {
    /// Serves the subnet of the network that carries this computer's default route, for as
    /// long as that address and interface still carry it.
    ///
    /// # Errors
    /// Fails when there is no shareable network or the loopback listener cannot bind.
    pub fn start() -> Result<Self, StartError> {
        let scope = Scope::current()?;
        Ok(Self::with_gate(scope.subnet(), Arc::new(scope))?)
    }

    fn with_gate(subnet: Subnet, gate: Arc<dyn socks::Gate>) -> io::Result<Self> {
        Ok(Self {
            subnet,
            inner: socks::Proxy::start(gate)?,
        })
    }

    #[must_use]
    pub const fn subnet(&self) -> Subnet {
        self.subnet
    }

    /// The loopback port that the bridge's reverse forward targets.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.inner.port()
    }

    #[must_use]
    pub fn counters(&self) -> Counters {
        self.inner.counters()
    }

    /// The connections relaying now, oldest first.
    #[must_use]
    pub fn relays(&self) -> Vec<Relay> {
        self.inner.relays()
    }

    fn close(&self) {
        self.inner.close();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Starting,
    /// The worker helper confirmed the bridge and serves `proxy` on the worker's loopback.
    Active {
        proxy: SocketAddrV4,
    },
    /// The session ended; it is retried with backoff while the bridge is on.
    Reconnecting {
        error: String,
    },
    /// Retrying cannot help, for example on a worker image without the helper.
    Failed {
        error: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub subnet: Subnet,
    pub state: State,
    pub counters: Counters,
    /// Open relays, oldest first; the counters also include connections still negotiating.
    pub relays: Vec<Relay>,
}

struct Shared {
    state: Mutex<State>,
}

impl Shared {
    fn set(&self, state: State) {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner) = state;
    }
}

/// One cloud's bridge: the local [`Proxy`] and the supervised SSH session that forwards the
/// worker's bridge socket to it.
pub struct Bridge {
    shared: Arc<Shared>,
    cancel: Cancellation,
    supervisor: Option<JoinHandle<()>>,
    proxy: Proxy,
}

impl Bridge {
    /// Shares the current network with the worker behind `connection`, and answers its
    /// agents' discovery requests.
    ///
    /// # Errors
    /// Fails when there is no shareable network or the local proxy cannot start.
    pub fn start(connection: &Connection) -> Result<Self, StartError> {
        let scope = Arc::new(Scope::current()?);
        // Switching the bridge off also stops a probe or browse an agent asked for.
        let cancel = Cancellation::default();
        let answers = Arc::new(Discoverer::new(Arc::clone(&scope), cancel.clone()));
        let proxy = Proxy::with_gate(scope.subnet(), scope)?;
        Ok(Self::with_answers(
            proxy,
            session::Ssh(connection.clone()),
            answers,
            cancel,
        )?)
    }

    /// A bridge whose helper hears that nothing is answered here, for the Unix-only session tests.
    #[cfg(all(test, unix))]
    fn with_parts(proxy: Proxy, transport: impl session::Transport) -> io::Result<Self> {
        Self::with_answers(proxy, transport, Arc::new(tests::Unanswered), Cancellation::default())
    }

    /// `cancel` is shared with `answers`, so revoking the bridge stops their work too.
    fn with_answers(
        proxy: Proxy,
        transport: impl session::Transport,
        answers: Arc<dyn Answers>,
        cancel: Cancellation,
    ) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::Starting),
        });
        let supervisor = {
            let shared = Arc::clone(&shared);
            let cancel = cancel.clone();
            let (subnet, port) = (proxy.subnet(), proxy.port());
            thread::Builder::new()
                .name("local-network-bridge".into())
                .spawn(move || session::supervise(&transport, subnet, port, &answers, &shared, &cancel))?
        };
        Ok(Self {
            shared,
            cancel,
            supervisor: Some(supervisor),
            proxy,
        })
    }

    #[must_use]
    pub fn status(&self) -> Status {
        Status {
            subnet: self.proxy.subnet(),
            state: self.shared.state.lock().unwrap_or_else(PoisonError::into_inner).clone(),
            counters: self.proxy.counters(),
            relays: self.proxy.relays(),
        }
    }
}

impl Bridge {
    /// Revokes the bridge at once without waiting: the proxy refuses and closes every
    /// connection, a probe or browse in progress stops, and the SSH session is told to end. Dropping it afterwards only waits for
    /// that session to finish.
    pub fn revoke(&self) {
        self.cancel.cancel();
        self.proxy.close();
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }
        // The proxy then stops accepting and closes every relayed connection.
    }
}

/// Answers what the worker helper asks on behalf of its agents.
pub(super) trait Answers: Send + Sync + 'static {
    /// What this computer answers, told to the helper once per session.
    fn hello(&self) -> Hello;
    fn answer(&self, request: Request) -> Answer;
}

type Resolve = Box<dyn Fn(&str, u16) -> Result<Vec<SocketAddr>, Reply> + Send + Sync>;
type ReadHost = Box<dyn Fn() -> io::Result<scope::Host> + Send + Sync>;
type Source = Box<dyn Fn(SocketAddr) -> Option<Ipv4Addr> + Send + Sync>;

/// Which destinations a bridge may reach: hosts on the network this computer was on when the
/// bridge started, reached from this computer's address there, never this computer itself.
/// Names are resolved here, and only the checked addresses are returned for dialling, so a name
/// cannot resolve again to something else.
pub struct Scope {
    network: scope::Network,
    resolve: Resolve,
    host: ReadHost,
    source: Source,
}

impl Scope {
    /// The scope of the network that carries this computer's default route now.
    ///
    /// # Errors
    /// Fails when there is no shareable network or the interfaces cannot be read.
    pub fn current() -> Result<Self, StartError> {
        let lookups = Arc::new(AtomicUsize::new(0));
        Ok(Self {
            network: scope::Host::read()?.current_network()?,
            resolve: Box::new(move |name, port| resolve(&lookups, name, port)),
            host: Box::new(scope::Host::read),
            source: Box::new(scope::source_for),
        })
    }

    #[must_use]
    pub const fn subnet(&self) -> Subnet {
        self.network.subnet
    }

    /// The addresses to try, in order, for `destination`, or the refusal to report.
    ///
    /// # Errors
    /// Refuses destinations outside the scope, names that do not resolve, and every
    /// destination once this computer has left the bridged network.
    pub fn admit(&self, destination: &Destination) -> Result<Vec<SocketAddr>, Reply> {
        self.on_network()?;
        let candidates = match destination {
            Destination::Address(address) => vec![*address],
            Destination::Name(name, port) => (self.resolve)(name, *port)?,
        };
        // A lookup takes time; decide on this computer as it is after it.
        let host = self.on_network()?;
        let allowed: Vec<_> = candidates
            .into_iter()
            .map(|candidate| SocketAddr::new(candidate.ip().to_canonical(), candidate.port()))
            .filter(|target| scope::admits(&self.network, &host, target.ip(), || (self.source)(*target)))
            .take(MAX_ATTEMPTS)
            .collect();
        if allowed.is_empty() {
            return Err(Reply::NotAllowed);
        }
        Ok(allowed)
    }

    /// The addresses among `addresses` that a bridge may reach, judged against this computer
    /// as it is now, read once for all of them.
    ///
    /// # Errors
    /// Refuses everything once this computer has left the bridged network.
    fn reachable(&self, addresses: &BTreeSet<Ipv4Addr>) -> Result<BTreeSet<Ipv4Addr>, Reply> {
        let host = self.on_network()?;
        Ok(addresses
            .iter()
            .copied()
            .filter(|address| {
                scope::admits(&self.network, &host, IpAddr::V4(*address), || {
                    // Any port routes the same; the discard port stands for all of them.
                    (self.source)(SocketAddr::new(IpAddr::V4(*address), 9))
                })
            })
            .collect())
    }

    /// This computer now, while it is still on the bridged network.
    fn on_network(&self) -> Result<scope::Host, Reply> {
        let host = (self.host)().map_err(|_| Reply::GeneralFailure)?;
        if host.current_network().ok().as_ref() != Some(&self.network) {
            return Err(Reply::NetworkUnreachable);
        }
        Ok(host)
    }
}

/// One running lookup's share of [`MAX_LOOKUPS`], held by its thread until the lookup returns.
struct Lookup(Arc<AtomicUsize>);

impl Drop for Lookup {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The system resolver under a deadline. A lookup that outlives it finishes on its own thread
/// and is discarded; it keeps its share of the lookup limit until then, so stalled lookups
/// cannot pile up. A name that does not resolve in time is unreachable; the bridge's own
/// limit is a general failure.
fn resolve(lookups: &Arc<AtomicUsize>, name: &str, port: u16) -> Result<Vec<SocketAddr>, Reply> {
    lookups
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |running| {
            (running < MAX_LOOKUPS).then_some(running + 1)
        })
        .map_err(|_| Reply::GeneralFailure)?;
    let lookup = Lookup(Arc::clone(lookups));
    let (sender, receiver) = mpsc::channel();
    let name = name.to_owned();
    thread::Builder::new()
        .name("local-network-resolve".into())
        .spawn(move || {
            let _lookup = lookup;
            let _ = sender.send((name.as_str(), port).to_socket_addrs().map(Iterator::collect));
        })
        .map_err(|_| Reply::GeneralFailure)?;
    match receiver.recv_timeout(RESOLVE_TIMEOUT) {
        Ok(Ok(addresses)) => Ok(addresses),
        Ok(Err(_)) | Err(_) => Err(Reply::HostUnreachable),
    }
}

#[cfg(test)]
mod tests;
