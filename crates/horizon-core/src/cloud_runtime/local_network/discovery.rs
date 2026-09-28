//! Finds the devices on the bridged network for an agent that asks: one short mDNS/DNS-SD
//! browse, one SSDP search and this computer's neighbor table, all from this computer and
//! only on request. Every address passes the bridge's [`Scope`] before an agent sees it.
mod mdns;
mod neighbors;
mod probe;
mod ssdp;

use super::{Answers, Scope};
use horizon_cloud_protocol::local_network::{
    Reply, Subnet,
    discovery::{Answer, Device, Discovery, Hello, MAX_NAMES, MAX_SERVICES, Request, Service, Source, VERSION},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    sync::{Arc, Mutex, PoisonError},
    thread,
    time::{Duration, Instant},
};

/// A browse's results answer requests for this long, so an agent asking again soon does not
/// send anything on the network.
const REUSE_FOR: Duration = Duration::from_secs(15);
/// Addresses one browse keeps track of before the scope decides.
const MAX_FOUND: usize = 1024;

/// One thing a source learned about an address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Finding {
    Seen(Ipv4Addr, Source),
    Name(Ipv4Addr, String),
    Service(Ipv4Addr, Service),
}

impl Finding {
    const fn address(&self) -> Ipv4Addr {
        match self {
            Self::Seen(address, _) | Self::Name(address, _) | Self::Service(address, _) => *address,
        }
    }
}

/// Browses from this computer's bridged address: what was found, and notes on what failed.
type Browse = Box<dyn Fn(Ipv4Addr, Subnet) -> (Vec<Finding>, Vec<String>) + Send + Sync>;

/// Answers one bridge's discovery requests: at most one browse at a time, and none while
/// recent results can answer instead.
pub struct Discoverer {
    scope: Arc<Scope>,
    browse: Browse,
    /// The last results and when their browse ended. It stays locked for a whole browse, so
    /// requests that arrive meanwhile wait and then share its results.
    last: Mutex<Option<(Instant, Discovery)>>,
    prober: probe::Prober,
}

impl Discoverer {
    #[must_use]
    pub fn new(scope: Arc<Scope>) -> Self {
        Self::with_browse(scope, Box::new(browse))
    }

    fn with_browse(scope: Arc<Scope>, browse: Browse) -> Self {
        Self::with_parts(scope, browse, Box::new(probe::connect))
    }

    fn with_parts(scope: Arc<Scope>, browse: Browse, connect: probe::Connect) -> Self {
        Self {
            scope,
            browse,
            last: Mutex::new(None),
            prober: probe::Prober::new(connect),
        }
    }

    /// The devices on the bridged network, from recent results or from a browse now.
    #[must_use]
    pub fn discover(&self) -> Answer {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((at, discovery)) = last.as_ref()
            && at.elapsed() < REUSE_FOR
        {
            return Answer::Discovery(self.with_probes(Discovery {
                age_seconds: at.elapsed().as_secs(),
                ..discovery.clone()
            }));
        }
        // Nothing is sent on a network other than the bridged one.
        if let Err(reply) = self.scope.on_network() {
            return Answer::Refused(reply.message().into());
        }
        let (findings, notes) = (self.browse)(self.scope.network.address, self.scope.subnet());
        let devices = match merge(findings, |addresses| self.scope.reachable(addresses)) {
            Ok(devices) => devices,
            Err(reply) => return Answer::Refused(reply.message().into()),
        };
        let discovery = Discovery {
            age_seconds: 0,
            devices,
            truncated: false,
            notes,
        }
        .bounded();
        *last = Some((Instant::now(), discovery.clone()));
        Answer::Discovery(self.with_probes(discovery))
    }

    /// Adds the ports that probes found open, so later answers list them as known ports.
    fn with_probes(&self, mut discovery: Discovery) -> Discovery {
        let open = self.prober.open();
        if open.is_empty() {
            return discovery;
        }
        for (address, ports) in open {
            let index = if let Some(index) = discovery.devices.iter().position(|device| device.address == address) {
                index
            } else {
                discovery.devices.push(Device {
                    address,
                    names: Vec::new(),
                    services: Vec::new(),
                    ports: Vec::new(),
                    sources: Vec::new(),
                });
                discovery.devices.len() - 1
            };
            let device = &mut discovery.devices[index];
            // A port the device already advertises is not news, and must not read as truncation.
            let fresh: Vec<_> = ports.into_iter().filter(|port| !device.ports.contains(port)).collect();
            device.ports.extend(fresh);
            if !device.sources.contains(&Source::Probe) {
                device.sources.push(Source::Probe);
                device.sources.sort_unstable();
            }
        }
        discovery.bounded()
    }
}

impl Answers for Discoverer {
    fn hello(&self) -> Hello {
        let mut sources = vec![Source::Mdns, Source::Ssdp];
        if cfg!(any(target_os = "linux", target_os = "macos", windows)) {
            sources.push(Source::Neighbors);
        }
        sources.push(Source::Probe);
        Hello {
            discovery: VERSION,
            sources,
            note: cfg!(windows).then(|| {
                "Discovery from a Windows computer is not fully tested yet; its firewall can hide devices that answer mDNS or SSDP".into()
            }),
        }
    }

    /// Every request is checked here first: the helper that sent it is not trusted.
    fn answer(&self, request: Request) -> Answer {
        if let Err(refusal) = request.validate() {
            return Answer::Refused(refusal);
        }
        match request {
            Request::Discover => self.discover(),
            Request::Probe { host, ports } => self.prober.probe(&self.scope, &host, &ports),
        }
    }
}

/// Combines what the sources found per address, keeping only the addresses `reachable`
/// admits; the limits of one answer are applied later.
fn merge(
    findings: Vec<Finding>,
    reachable: impl FnOnce(&BTreeSet<Ipv4Addr>) -> Result<BTreeSet<Ipv4Addr>, Reply>,
) -> Result<Vec<Device>, Reply> {
    let mut found: BTreeMap<Ipv4Addr, Device> = BTreeMap::new();
    for finding in findings {
        let address = finding.address();
        if found.len() >= MAX_FOUND && !found.contains_key(&address) {
            continue;
        }
        let device = found.entry(address).or_insert_with(|| Device {
            address,
            names: Vec::new(),
            services: Vec::new(),
            ports: Vec::new(),
            sources: Vec::new(),
        });
        match finding {
            Finding::Seen(_, source) => {
                if !device.sources.contains(&source) {
                    device.sources.push(source);
                }
            }
            Finding::Name(_, name) => {
                // One over the limit, so the answer reports that names were dropped.
                if !name.is_empty() && !device.names.contains(&name) && device.names.len() <= MAX_NAMES {
                    device.names.push(name);
                }
            }
            Finding::Service(_, service) => {
                if !device.sources.contains(&service.source) {
                    device.sources.push(service.source);
                }
                // Ports lists TCP ports; a DNS-SD `_udp` service's port is not one.
                if let Some(port) = service.port
                    && !service.kind.ends_with("._udp")
                    && !device.ports.contains(&port)
                {
                    device.ports.push(port);
                }
                if !device.services.contains(&service) && device.services.len() <= MAX_SERVICES {
                    device.services.push(service);
                }
            }
        }
    }
    let admitted = reachable(&found.keys().copied().collect())?;
    Ok(found
        .into_values()
        .filter(|device| admitted.contains(&device.address))
        .map(|mut device| {
            device.sources.sort_unstable();
            device.names.sort_unstable();
            device
        })
        .collect())
}

/// A datagram socket on this computer's bridged address that sends multicast out of the
/// bridged interface only, and receives the answers sent back to it.
fn multicast_socket(local: Ipv4Addr, ttl: u32) -> io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_multicast_if_v4(&local)?;
    socket.set_multicast_ttl_v4(ttl)?;
    socket.set_multicast_loop_v4(false)?;
    socket.bind(&SocketAddr::from((local, 0)).into())?;
    Ok(socket.into())
}

/// The neighbor table first, so mDNS can ask those neighbors their names, then mDNS and SSDP
/// side by side for about three seconds.
fn browse(local: Ipv4Addr, subnet: Subnet) -> (Vec<Finding>, Vec<String>) {
    let mut notes = Vec::new();
    let mut findings = Vec::new();
    let neighbors = neighbors::read().unwrap_or_else(|error| {
        notes.push(format!(
            "The neighbor table could not be read on the owner's computer: {error}"
        ));
        Vec::new()
    });
    let neighbors: Vec<_> = neighbors
        .into_iter()
        .filter(|address| subnet.contains_host(*address) && *address != local)
        .collect();
    findings.extend(
        neighbors
            .iter()
            .map(|address| Finding::Seen(*address, Source::Neighbors)),
    );
    let upnp = thread::Builder::new()
        .name("local-network-ssdp".into())
        .spawn(move || ssdp::browse(local));
    let reverse: Vec<_> = neighbors.into_iter().take(mdns::MAX_REVERSE).collect();
    match mdns::browse(local, &reverse) {
        Ok(found) => findings.extend(found),
        Err(error) => notes.push(format!("mDNS browsing failed on the owner's computer: {error}")),
    }
    match upnp.map_err(|error| error.to_string()).and_then(|upnp| {
        upnp.join()
            .map_err(|_| "the search stopped unexpectedly".to_owned())?
            .map_err(|error| error.to_string())
    }) {
        Ok(found) => findings.extend(found),
        Err(error) => notes.push(format!("SSDP search failed on the owner's computer: {error}")),
    }
    (findings, notes)
}

#[cfg(test)]
mod tests;
