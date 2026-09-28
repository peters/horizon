//! A port probe of one named host, only when an agent asks: TCP connects to a few ports, a few
//! probes a minute, one at a time, and only to an address the bridge's scope admits. Nothing
//! is sent to the device beyond the connection attempt, and nothing sweeps the subnet.
use super::{
    super::{Cancellation, Destination, Scope},
    STOPPED,
};
use horizon_cloud_protocol::local_network::{
    Reply,
    discovery::{Answer, Probe, Request},
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    sync::{Arc, Mutex, PoisonError, TryLockError, mpsc},
    thread,
    time::{Duration, Instant},
};

pub(super) const PER_MINUTE: usize = 6;
pub(super) const RUNNING: &str = "A probe is already running; try again in a few seconds";
pub(super) const NARROWED: &str = "The owner changed the bridge's scope during the probe; probe again";
const MINUTE: Duration = Duration::from_secs(60);
/// Connection attempts in flight at once.
const AT_ONCE: usize = 4;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
/// How often a probe waiting for a connection attempt checks whether the bridge stopped.
const CONNECT_POLL: Duration = Duration::from_millis(20);
/// How often a probe waiting for its host's lookup checks whether the bridge stopped.
const ADMIT_POLL: Duration = Duration::from_millis(100);
/// Addresses whose open ports are remembered for discovery answers.
const MAX_REMEMBERED: usize = 256;

/// Opens one TCP connection and closes it at once, giving up when the bridge stops.
pub(super) type Connect = Box<dyn Fn(SocketAddr, Duration, &Cancellation) -> io::Result<()> + Send + Sync>;

/// A plain blocking connect with its own time limit, on a short-lived thread the caller stops
/// waiting for once the bridge stops. No new attempt starts after that; one already in flight
/// may still complete within its limit, and its connection is closed at once without use.
///
/// # Errors
/// Returns the connection's own error, `TimedOut`, or `Interrupted` once `cancel` fires.
pub(super) fn connect(address: SocketAddr, timeout: Duration, cancel: &Cancellation) -> io::Result<()> {
    if cancel.is_cancelled() {
        return Err(io::Error::new(io::ErrorKind::Interrupted, STOPPED));
    }
    let (sender, outcome) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("local-network-dial".into())
        .spawn(move || {
            let _ = sender.send(TcpStream::connect_timeout(&address, timeout).map(drop));
        })?;
    loop {
        match outcome.recv_timeout(CONNECT_POLL) {
            Ok(outcome) => return outcome,
            Err(mpsc::RecvTimeoutError::Timeout) if cancel.is_cancelled() => {
                return Err(io::Error::new(io::ErrorKind::Interrupted, STOPPED));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(io::Error::other("the attempt stopped")),
        }
    }
}

/// Why a probe of `ports` on `host` is refused although the device itself is in scope.
fn outside_ports(host: &str, ports: &[u16]) -> String {
    let noun = if ports.len() == 1 { "Port" } else { "Ports" };
    let ports: Vec<_> = ports.iter().map(u16::to_string).collect();
    format!(
        "{noun} {} on {host} {} outside the bridge's scope; the owner chooses what it reaches on the cloud card",
        ports.join(", "),
        if noun == "Port" { "is" } else { "are" }
    )
}

/// The scope's decision on `destination`, abandoned as soon as the bridge stops: a name lookup
/// can take seconds, so it runs on its own thread and its late result is dropped.
fn admit(scope: &Arc<Scope>, destination: Destination, cancel: &Cancellation) -> Result<Vec<SocketAddr>, String> {
    let (sender, decision) = mpsc::sync_channel(1);
    let scope = Arc::clone(scope);
    thread::Builder::new()
        .name("local-network-probe-admit".into())
        .spawn(move || {
            let _ = sender.send(scope.admit_host(&destination));
        })
        .map_err(|error| error.to_string())?;
    loop {
        if cancel.is_cancelled() {
            return Err(STOPPED.into());
        }
        match decision.recv_timeout(ADMIT_POLL) {
            Ok(decision) => return decision.map_err(|reply| reply.message().into()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(Reply::GeneralFailure.message().into()),
        }
    }
}

pub(super) struct Prober {
    connect: Connect,
    /// When recent probes started to connect, oldest first; refused probes are not counted.
    /// It stays locked for a whole probe, so probes run one at a time.
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
    pub(super) fn probe(&self, scope: &Arc<Scope>, host: &str, ports: &[u16], cancel: &Cancellation) -> Answer {
        // Never queued: a probe waiting behind another could start after the helper stopped
        // waiting for its answer.
        let mut started = match self.started.try_lock() {
            Ok(started) => started,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return Answer::Refused(RUNNING.into()),
        };
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
        let candidates: Vec<Ipv4Addr> = match admit(scope, destination, cancel) {
            Ok(admitted) => admitted
                .iter()
                .filter_map(|candidate| match candidate.ip() {
                    IpAddr::V4(address) => Some(address),
                    IpAddr::V6(_) => None,
                })
                .collect(),
            Err(refusal) => return Answer::Refused(refusal),
        };
        // The owner may have narrowed a device to some ports. A name with several addresses
        // probes the first that allows every named port, or, for the defaults, the first that
        // allows any; the defaults are then probed only where the scope reaches.
        let requested = Request::probe_ports(ports);
        let allowed = |address: Ipv4Addr| -> Vec<u16> {
            requested
                .iter()
                .copied()
                .filter(|port| scope.keeps(SocketAddr::new(address.into(), *port)))
                .collect()
        };
        let picked = candidates
            .iter()
            .map(|address| (*address, allowed(*address)))
            .find(|(_, allowed)| {
                if ports.is_empty() {
                    !allowed.is_empty()
                } else {
                    allowed.len() == requested.len()
                }
            });
        let Some((address, chosen)) = picked else {
            let Some(first) = candidates.first() else {
                return Answer::Refused(Reply::NotAllowed.message().into());
            };
            let open = allowed(*first);
            let outside: Vec<_> = requested.iter().copied().filter(|port| !open.contains(port)).collect();
            return Answer::Refused(outside_ports(host, &outside));
        };
        let mut probe = Probe {
            // Validation bounds the host, so it is echoed as the agent named it.
            host: host.to_owned(),
            address,
            open: Vec::new(),
            closed: Vec::new(),
            silent: Vec::new(),
        };
        for (index, batch) in chosen.chunks(AT_ONCE).enumerate() {
            // A bridge switched off, or whose computer left the network, stops probing at once.
            if cancel.is_cancelled() {
                return Answer::Refused(STOPPED.into());
            }
            if let Err(reply) = scope.on_network() {
                return Answer::Refused(reply.message().into());
            }
            // The owner may narrow the scope while a probe runs: no port it no longer allows is
            // dialled, so the probe ends instead.
            if batch
                .iter()
                .any(|port| !scope.keeps(SocketAddr::new(address.into(), *port)))
            {
                return Answer::Refused(NARROWED.into());
            }
            // Only a probe about to dial takes a slot of the rate limit, stamped now: a slow
            // lookup before it must not backdate the slot.
            if index == 0 {
                started.push_back(Instant::now());
            }
            for (port, result) in self.attempt(address, batch, cancel) {
                match result {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                        return Answer::Refused(STOPPED.into());
                    }
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
    fn attempt(&self, address: Ipv4Addr, ports: &[u16], cancel: &Cancellation) -> Vec<(u16, io::Result<()>)> {
        thread::scope(|scope| {
            let attempts: Vec<_> = ports
                .iter()
                .map(|&port| {
                    let target = SocketAddr::new(address.into(), port);
                    let attempt = thread::Builder::new()
                        .name("local-network-probe".into())
                        .spawn_scoped(scope, move || (self.connect)(target, CONNECT_TIMEOUT, cancel));
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

    /// When the latest probe took its slot.
    #[cfg(test)]
    pub(super) fn last_start(&self) -> Option<Instant> {
        self.started
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .back()
            .copied()
    }

    /// The ports probes found open so far, per address.
    pub(super) fn open(&self) -> BTreeMap<Ipv4Addr, BTreeSet<u16>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}
