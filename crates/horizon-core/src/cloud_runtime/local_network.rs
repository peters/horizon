//! Local Network Bridge: lets one cloud's worker open TCP connections to devices on the
//! network this computer is on, through a proxy that runs here. [`Scope`] is the whole policy
//! that proxy applies.
mod scope;

use horizon_cloud_protocol::local_network::{Reply, Subnet};
pub use scope::ScopeError;
use std::{
    io,
    net::{Ipv4Addr, SocketAddr, ToSocketAddrs},
    sync::mpsc,
    thread,
    time::Duration,
};

/// Resolved addresses tried for one hostname.
const MAX_ATTEMPTS: usize = 4;
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

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

type Resolve = Box<dyn Fn(&str, u16) -> io::Result<Vec<SocketAddr>> + Send + Sync>;
type ReadHost = Box<dyn Fn() -> io::Result<scope::Host> + Send + Sync>;
type Source = Box<dyn Fn(SocketAddr) -> Option<Ipv4Addr> + Send + Sync>;

/// Which destinations a bridge may reach: hosts on the network this computer was on when the
/// bridge started, reached through the same interface, never this computer itself. Names are
/// resolved here, and only the checked addresses are returned for dialling, so a name cannot
/// resolve again to something else.
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
        Ok(Self {
            network: scope::Host::read()?.current_network()?,
            resolve: Box::new(resolve),
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
        let candidates = match destination {
            Destination::Address(address) => vec![*address],
            Destination::Name(name, port) => (self.resolve)(name, *port).map_err(|_| Reply::HostUnreachable)?,
        };
        let host = (self.host)().map_err(|_| Reply::GeneralFailure)?;
        let mut changed = false;
        let mut allowed = Vec::new();
        for candidate in candidates {
            if allowed.len() == MAX_ATTEMPTS {
                break;
            }
            let target = SocketAddr::new(candidate.ip().to_canonical(), candidate.port());
            match scope::decide(&self.network, &host, candidate.ip(), || (self.source)(target)) {
                scope::Decision::Allowed => allowed.push(target),
                scope::Decision::OutsideScope => {}
                scope::Decision::NetworkChanged => changed = true,
            }
        }
        if allowed.is_empty() {
            return Err(if changed {
                Reply::NetworkUnreachable
            } else {
                Reply::NotAllowed
            });
        }
        Ok(allowed)
    }
}

/// The system resolver under a deadline. A lookup that outlives it finishes on its own thread
/// and is discarded.
fn resolve(name: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    let (sender, receiver) = mpsc::channel();
    let name = name.to_owned();
    thread::Builder::new()
        .name("local-network-resolve".into())
        .spawn(move || {
            let _ = sender.send((name.as_str(), port).to_socket_addrs().map(Iterator::collect));
        })?;
    receiver
        .recv_timeout(RESOLVE_TIMEOUT)
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
}

#[cfg(test)]
mod tests;
