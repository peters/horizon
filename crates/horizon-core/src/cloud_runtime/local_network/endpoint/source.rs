//! Captured endpoint bindings. Only existing host route/client observations produce them.
use super::super::{Destination, Scope, resolve, scope};
use super::{Endpoint, Error, SourceKind, tailnet::Client};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex, TryLockError, atomic::AtomicUsize},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Binding {
    Lan {
        network: scope::Network,
        address: SocketAddr,
    },
    Tailnet {
        desktop: String,
        peer: String,
        name: String,
        address: SocketAddr,
    },
}

impl Binding {
    pub(super) const fn address(&self) -> SocketAddr {
        match self {
            Self::Lan { address, .. } | Self::Tailnet { address, .. } => *address,
        }
    }

    pub(super) const fn kind(&self) -> SourceKind {
        match self {
            Self::Lan { .. } => SourceKind::Lan,
            Self::Tailnet { .. } => SourceKind::Tailnet,
        }
    }
}

#[cfg(test)]
type Observer = dyn Fn(&Endpoint) -> Result<Binding, Error> + Send + Sync;

enum Backend {
    Lan(Arc<Scope>),
    Tailnet(Client),
    #[cfg(test)]
    Fixture(Arc<Observer>),
}

pub(super) struct Source {
    backend: Backend,
    checking: Mutex<()>,
    lookups: Arc<AtomicUsize>,
}

impl Source {
    pub(super) fn lan() -> Result<Self, Error> {
        let scope = Scope::current().map_err(|_| Error::UnsupportedSource)?;
        Ok(Self::new(Backend::Lan(Arc::new(scope))))
    }

    pub(super) fn tailnet() -> Result<Self, Error> {
        Ok(Self::new(Backend::Tailnet(Client::existing()?)))
    }

    fn new(backend: Backend) -> Self {
        Self {
            backend,
            checking: Mutex::new(()),
            lookups: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[cfg(test)]
    pub(super) fn fixture(check: impl Fn(&Endpoint) -> Result<Binding, Error> + Send + Sync + 'static) -> Self {
        Self::new(Backend::Fixture(Arc::new(check)))
    }

    #[cfg(test)]
    pub(super) fn lan_fixture(scope: Scope) -> Self {
        Self::new(Backend::Lan(Arc::new(scope)))
    }

    pub(super) fn cancel(&self) {
        if let Backend::Tailnet(client) = &self.backend {
            client.cancel();
        }
    }

    pub(super) fn observe(&self, endpoint: &Endpoint) -> Result<Binding, Error> {
        let _checking = self.checking.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => Error::Busy,
            TryLockError::Poisoned(_) => Error::SourceUnavailable,
        })?;
        match &self.backend {
            Backend::Lan(scope) => {
                scope.on_network().map_err(|_| Error::SourceUnavailable)?;
                let addresses = match endpoint.destination() {
                    Destination::Address(address) => vec![address],
                    Destination::Name(name, port) => {
                        (scope.resolve)(&name, port).map_err(|_| Error::SourceUnavailable)?
                    }
                };
                // Check every IPv4 answer before LAN policy can filter or cap it.
                let address = single_ipv4(
                    addresses
                        .into_iter()
                        .map(|address| SocketAddr::new(address.ip().to_canonical(), address.port())),
                )?;
                let admitted = scope
                    .admit(&Destination::Address(address))
                    .map_err(|_| Error::SourceUnavailable)?;
                let [address] = admitted.as_slice() else {
                    return Err(Error::InvalidEndpoint);
                };
                Ok(Binding::Lan {
                    network: scope.network.clone(),
                    address: *address,
                })
            }
            Backend::Tailnet(client) => {
                let desktop = client.desktop()?;
                let addresses = match endpoint.destination() {
                    Destination::Address(address) => vec![address],
                    Destination::Name(name, port) => {
                        resolve(&self.lookups, &name, port).map_err(|_| Error::SourceUnavailable)?
                    }
                };
                let address = single_ipv4(addresses)?;
                let IpAddr::V4(ip) = address.ip() else {
                    return Err(Error::InvalidEndpoint);
                };
                if !tailnet_ip(ip)
                    || desktop.addresses.contains(&ip)
                    || !scope::source_for(address).is_some_and(|source| desktop.addresses.contains(&source))
                {
                    return Err(Error::UnsupportedSource);
                }
                let peer = client.peer(ip)?;
                if peer.id == desktop.id || !peer.addresses.contains(&ip) {
                    return Err(Error::InvalidEndpoint);
                }
                if endpoint.host.parse::<IpAddr>().is_err() && endpoint.host != peer.name {
                    return Err(Error::BindingChanged);
                }
                // Re-read after DNS/WhoIs: a changed desktop identity must not inherit approval.
                if client.desktop()? != desktop {
                    return Err(Error::BindingChanged);
                }
                Ok(Binding::Tailnet {
                    desktop: desktop.id,
                    peer: peer.id,
                    name: peer.name,
                    address,
                })
            }
            #[cfg(test)]
            Backend::Fixture(check) => check(endpoint),
        }
    }
}

fn single_ipv4(addresses: impl IntoIterator<Item = SocketAddr>) -> Result<SocketAddr, Error> {
    let mut addresses: Vec<_> = addresses.into_iter().filter(SocketAddr::is_ipv4).collect();
    addresses.sort_unstable();
    addresses.dedup();
    match addresses.as_slice() {
        [address] => Ok(*address),
        _ => Err(Error::InvalidEndpoint),
    }
}

fn tailnet_ip(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}
