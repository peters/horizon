//! Local Network Bridge: lets one cloud's worker open TCP connections to devices on the
//! network this computer is on, through a proxy that runs here. [`Scope`] is the whole policy
//! that proxy applies.
mod scope;

use horizon_cloud_protocol::local_network::{Reply, Subnet};
pub use scope::ScopeError;
use std::{
    io,
    net::{Ipv4Addr, SocketAddr, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
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
