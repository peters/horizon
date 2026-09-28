//! A port probe of one named host, only when an agent asks: TCP connects to a few ports, a few
//! probes a minute, one at a time, and only to an address the bridge's scope admits. Nothing
//! is sent to the device beyond the connection attempt, and nothing sweeps the subnet.
use super::super::{Destination, Scope};
use horizon_cloud_protocol::local_network::discovery::{Answer, Probe, Request};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    sync::{Mutex, PoisonError},
    thread,
    time::{Duration, Instant},
};

pub(super) const PER_MINUTE: usize = 6;
const MINUTE: Duration = Duration::from_secs(60);
/// Connection attempts in flight at once.
const AT_ONCE: usize = 4;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
/// Addresses whose open ports are remembered for discovery answers.
const MAX_REMEMBERED: usize = 256;

/// Opens one TCP connection and closes it at once.
pub(super) type Connect = Box<dyn Fn(SocketAddr, Duration) -> io::Result<()> + Send + Sync>;

pub(super) fn connect(address: SocketAddr, timeout: Duration) -> io::Result<()> {
    TcpStream::connect_timeout(&address, timeout).map(drop)
}

pub(super) struct Prober {
    connect: Connect,
    /// When recent probes started, oldest first. It stays locked for a whole probe, so probes
    /// run one at a time.
    started: Mutex<VecDeque<Instant>>,
    /// The ports found open, per address.
    open: Mutex<BTreeMap<Ipv4Addr, BTreeSet<u16>>>,
}

impl Prober {
    pub(super) fn new(connect: Connect) -> Self {
        Self {
            connect,
            started: Mutex::new(VecDeque::with_capacity(PER_MINUTE)),
            open: Mutex::new(BTreeMap::new()),
        }
    }

    /// Tries `ports`, or the defaults, on `host`; the caller has validated both.
    pub(super) fn probe(&self, scope: &Scope, host: &str, ports: &[u16]) -> Answer {
        let mut started = self.started.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        while started.front().is_some_and(|at| now.duration_since(*at) >= MINUTE) {
            started.pop_front();
        }
        if let Some(oldest) = started.front()
            && started.len() >= PER_MINUTE
        {
            let wait = MINUTE.saturating_sub(now.duration_since(*oldest)).as_secs() + 1;
            return Answer::Refused(format!(
                "At most {PER_MINUTE} probes a minute; try again in {wait} seconds"
            ));
        }
        // The scope resolves a name on this computer and admits only a device on the bridged
        // network; the probe connects to exactly the address it admitted.
        let destination = match host.parse::<Ipv4Addr>() {
            Ok(address) => Destination::Address(SocketAddr::new(address.into(), 1)),
            Err(_) => Destination::Name(host.to_owned(), 1),
        };
        let address = match scope.admit(&destination) {
            Ok(admitted) => admitted.iter().find_map(|candidate| match candidate.ip() {
                IpAddr::V4(address) => Some(address),
                IpAddr::V6(_) => None,
            }),
            Err(reply) => return Answer::Refused(reply.message().into()),
        };
        let Some(address) = address else {
            return Answer::Refused(
                horizon_cloud_protocol::local_network::Reply::NotAllowed
                    .message()
                    .into(),
            );
        };
        started.push_back(now);
        let mut probe = Probe {
            // Validation bounds the host, so it is echoed as the agent named it.
            host: host.to_owned(),
            address,
            open: Vec::new(),
            closed: Vec::new(),
            silent: Vec::new(),
        };
        for batch in Request::probe_ports(ports).chunks(AT_ONCE) {
            // A bridge whose computer left the network stops probing at once.
            if let Err(reply) = scope.on_network() {
                return Answer::Refused(reply.message().into());
            }
            for (port, result) in self.attempt(address, batch) {
                match result {
                    Ok(()) => probe.open.push(port),
                    Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => probe.closed.push(port),
                    Err(_) => probe.silent.push(port),
                }
            }
        }
        self.remember(address, &probe.open);
        Answer::Probe(probe)
    }

    /// One batch of connection attempts side by side.
    fn attempt(&self, address: Ipv4Addr, ports: &[u16]) -> Vec<(u16, io::Result<()>)> {
        thread::scope(|scope| {
            let attempts: Vec<_> = ports
                .iter()
                .map(|&port| {
                    let target = SocketAddr::new(address.into(), port);
                    let attempt = thread::Builder::new()
                        .name("local-network-probe".into())
                        .spawn_scoped(scope, move || (self.connect)(target, CONNECT_TIMEOUT));
                    (port, attempt)
                })
                .collect();
            attempts
                .into_iter()
                .map(|(port, attempt)| {
                    let result = attempt
                        .map_err(|error| io::Error::other(error.to_string()))
                        .and_then(|attempt| {
                            attempt
                                .join()
                                .unwrap_or_else(|_| Err(io::Error::other("the attempt stopped")))
                        });
                    (port, result)
                })
                .collect()
        })
    }

    fn remember(&self, address: Ipv4Addr, open: &[u16]) {
        let mut remembered = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if open.is_empty() || (remembered.len() >= MAX_REMEMBERED && !remembered.contains_key(&address)) {
            return;
        }
        remembered.entry(address).or_default().extend(open);
    }

    /// The ports probes found open so far, per address.
    pub(super) fn open(&self) -> BTreeMap<Ipv4Addr, BTreeSet<u16>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}
