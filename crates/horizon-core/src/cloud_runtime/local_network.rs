//! Local Network Bridge: lets one cloud's worker open TCP connections to devices on the
//! network this computer is on, through a scope-checking proxy that runs here.
//!
//! [`Scope`] is the whole policy and the [`Proxy`] applies it: the worker only ever reaches
//! the proxy's loopback port. Only the owner starts a [`Bridge`], from the cloud card, and
//! nothing persists it; dropping it stops its SSH session and closes every relayed connection.
mod discovery;
mod rules;
mod scope;
mod session;
mod socks;

use super::{Cancellation, ssh::Connection};
pub use discovery::Discoverer;
pub use horizon_cloud_protocol::local_network::Subnet;
use horizon_cloud_protocol::local_network::{
    Reply,
    discovery::{Answer, Hello, Request},
};
pub use rules::{Device, MAX_DEVICES, MAX_PORTS, Rules, RulesError};
pub use scope::ScopeError;
pub use socks::{BYTE_BUDGET, Counters, MAX_CONNECTIONS, Relay};
use std::{
    collections::BTreeSet,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs},
    sync::{
        Arc, Mutex, PoisonError, RwLock,
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
    #[error("The saved scope does not fit the current network: {0}")]
    Rules(#[from] RulesError),
    /// The network this computer is on is not the one the start was approved for; `None` when
    /// it is on no shareable network at all.
    #[error("This computer is no longer on the network sharing was approved for")]
    Moved(Option<Network>),
    #[error("Local Network Bridge could not start: {0}")]
    Io(#[from] io::Error),
}

/// Which network this computer is on: the subnet that carries its default route, and its own
/// address and interface there. A bridge shares only the network it started on, so comparing
/// this with [`Bridge::network`] tells a move to another network, such as a new Wi-Fi.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network(scope::Network);

impl Network {
    /// The network that carries this computer's default route now.
    ///
    /// # Errors
    /// Fails when there is no shareable network or the interfaces cannot be read.
    pub fn current() -> Result<Self, StartError> {
        Ok(Self(scope::Host::read()?.current_network()?))
    }

    /// A network described by its parts, for code that compares networks without reading
    /// this computer's interfaces.
    #[must_use]
    pub fn new(subnet: Subnet, address: Ipv4Addr, interface: impl Into<String>) -> Self {
        Self(scope::Network {
            subnet,
            address,
            interface: interface.into(),
        })
    }

    #[must_use]
    pub const fn subnet(&self) -> Subnet {
        self.0.subnet
    }
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
    /// The owner's scope; absent only in session tests, which use a fixture gate.
    scope: Option<Arc<Scope>>,
}

impl Bridge {
    /// Shares the current network with the worker behind `connection`, and answers its
    /// agents' discovery requests.
    ///
    /// # Errors
    /// Fails when there is no shareable network or the local proxy cannot start.
    pub fn start(connection: &Connection) -> Result<Self, StartError> {
        Self::start_with(connection, Rules::default(), None)
    }

    /// As [`Self::start`], narrowed by `rules` before the worker can reach the proxy: a bridge
    /// that resumes never starts wider than the owner left it.
    ///
    /// # Errors
    /// Also fails when `rules` do not fit the current network, for example after a move to
    /// another Wi-Fi, and with [`StartError::Moved`] when `expected` names a network other than
    /// the one this computer is on now, or when there is none it can share now: a start approved
    /// for one network never shares another.
    pub fn start_with(connection: &Connection, rules: Rules, expected: Option<&Network>) -> Result<Self, StartError> {
        let scope = Arc::new(approved(Scope::current(), expected)?);
        // Switching the bridge off also stops a probe or browse an agent asked for.
        let cancel = Cancellation::default();
        let answers = Arc::new(Discoverer::new(Arc::clone(&scope), cancel.clone()));
        let proxy = Proxy::with_gate(scope.subnet(), Arc::clone(&scope) as Arc<dyn socks::Gate>)?;
        // Only the SSH session started below lets the worker reach the proxy.
        scope.set_rules(rules, proxy.port())?;
        let mut bridge = Self::with_answers(proxy, session::Ssh(connection.clone()), answers, cancel)?;
        bridge.scope = Some(scope);
        Ok(bridge)
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
            scope: None,
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
    /// The network this bridge shares; absent only for the session tests' fixture gate.
    #[must_use]
    pub fn network(&self) -> Option<Network> {
        self.scope.as_ref().map(|scope| Network(scope.network.clone()))
    }

    /// The owner's current narrowing of the scope.
    #[must_use]
    pub fn rules(&self) -> Rules {
        self.scope.as_ref().map(|scope| scope.rules()).unwrap_or_default()
    }

    /// Applies new rules at once: later connections, discovery and probes follow them, and
    /// every open connection they no longer allow is closed.
    ///
    /// # Errors
    /// Refuses rules that name something the bridge cannot reach, leaving the old ones.
    pub fn set_rules(&self, rules: Rules) -> Result<(), RulesError> {
        let Some(scope) = &self.scope else {
            return Ok(());
        };
        scope.set_rules(rules, self.proxy.port())?;
        self.proxy.inner.close_unless(|address| scope.keeps(address));
        Ok(())
    }

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

/// The scope a start may use: `current`, unless the start was approved for another network.
/// No shareable network then is a move away from the approved one, not a refusal; failing to
/// read the interfaces stays an error.
fn approved(current: Result<Scope, StartError>, expected: Option<&Network>) -> Result<Scope, StartError> {
    let scope = match current {
        Err(StartError::Scope(ScopeError::NoNetwork | ScopeError::PointToPoint)) if expected.is_some() => {
            return Err(StartError::Moved(None));
        }
        current => current?,
    };
    match expected {
        Some(expected) if expected.0 != scope.network => Err(StartError::Moved(Some(Network(scope.network.clone())))),
        _ => Ok(scope),
    }
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
    rules: RwLock<Rules>,
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
            rules: RwLock::default(),
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
        // This computer's own loopback services, only on the ports the owner opened.
        let local = match destination {
            Destination::Address(address) => {
                rules::LocalHost::of_address(address.ip()).map(|host| (host, address.port()))
            }
            Destination::Name(name, port) => rules::LocalHost::of_name(name).map(|host| (host, *port)),
        };
        if let Some((host, port)) = local {
            return self.rules().local(host, port).ok_or(Reply::NotAllowed);
        }
        self.admit_device(destination, Rules::permits_device)
    }

    /// As [`Self::admit`] for a device on the bridged network whatever the port, never this
    /// computer: what a probe dials, port by port, after [`Self::keeps`].
    pub(super) fn admit_host(&self, destination: &Destination) -> Result<Vec<SocketAddr>, Reply> {
        self.on_network()?;
        self.admit_device(destination, |rules, address, _| rules.permits_host(address))
    }

    fn admit_device(
        &self,
        destination: &Destination,
        permits: impl Fn(&Rules, Ipv4Addr, u16) -> bool,
    ) -> Result<Vec<SocketAddr>, Reply> {
        let candidates = match destination {
            Destination::Address(address) => vec![*address],
            Destination::Name(name, port) => (self.resolve)(name, *port)?,
        };
        // A lookup takes time; decide on this computer, and on the rules, as they are after it.
        let host = self.on_network()?;
        let rules = self.rules();
        let allowed: Vec<_> = candidates
            .into_iter()
            .map(|candidate| SocketAddr::new(candidate.ip().to_canonical(), candidate.port()))
            .filter(|target| {
                matches!(target.ip(), IpAddr::V4(address) if permits(&rules, address, target.port()))
                    && scope::admits(&self.network, &host, target.ip(), || (self.source)(*target))
            })
            .take(MAX_ATTEMPTS)
            .collect();
        if allowed.is_empty() {
            return Err(Reply::NotAllowed);
        }
        Ok(allowed)
    }

    /// The owner's current rules.
    #[must_use]
    pub fn rules(&self) -> Rules {
        self.rules.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Whether a relay to `address`, admitted earlier, may stay open under the current rules.
    pub(super) fn keeps(&self, address: SocketAddr) -> bool {
        self.rules.read().unwrap_or_else(PoisonError::into_inner).keeps(address)
    }

    /// Replaces the rules once they are known to apply to the bridged network.
    fn set_rules(&self, rules: Rules, bridge_port: u16) -> Result<(), RulesError> {
        let host = self.on_network().ok();
        rules.validate(
            |address| {
                host.as_ref().is_some_and(|host| {
                    scope::admits(&self.network, host, IpAddr::V4(address), || {
                        (self.source)(SocketAddr::new(IpAddr::V4(address), 9))
                    })
                })
            },
            bridge_port,
        )?;
        *self.rules.write().unwrap_or_else(PoisonError::into_inner) = rules;
        Ok(())
    }

    /// The addresses among `addresses` that a bridge may reach, judged against this computer
    /// as it is now, read once for all of them.
    ///
    /// # Errors
    /// Refuses everything once this computer has left the bridged network.
    fn reachable(&self, addresses: &BTreeSet<Ipv4Addr>) -> Result<BTreeSet<Ipv4Addr>, Reply> {
        let host = self.on_network()?;
        let rules = self.rules();
        Ok(addresses
            .iter()
            .copied()
            .filter(|address| {
                rules.permits_host(*address)
                    && scope::admits(&self.network, &host, IpAddr::V4(*address), || {
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
