//! A SOCKS5 CONNECT subset on this computer's loopback: the only way bridged worker
//! connections reach the local network. Every destination passes the [`Gate`] first.
use super::Destination;
use horizon_cloud_protocol::local_network::Reply;
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// All connection attempts for one request together.
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a refused client may take to finish sending before its socket closes, so the
/// close does not reset the connection and discard the reply.
const LINGER: Duration = Duration::from_millis(250);
const ACCEPT_POLL: Duration = Duration::from_millis(25);
const BUFFER_BYTES: usize = 16 * 1024;
pub const MAX_CONNECTIONS: usize = 64;
/// Relayed bytes in both directions per bridge; the owner restarts the bridge to reset it.
pub const BYTE_BUDGET: u64 = 64 * 1024 * 1024 * 1024;
const VERSION: u8 = 5;
const NO_AUTHENTICATION: u8 = 0;
const NO_ACCEPTABLE_METHOD: u8 = 0xff;
const CONNECT: u8 = 1;

pub(super) trait Gate: Send + Sync {
    /// The checked addresses to try in order, or why the destination is refused. The proxy
    /// connects only to returned addresses, so a name cannot resolve again to something else.
    fn admit(&self, destination: &Destination) -> Result<Vec<SocketAddr>, Reply>;

    /// Whether a relay to an address admitted earlier may stay open under the current rules.
    fn keeps(&self, _address: SocketAddr) -> bool {
        true
    }
}

impl Gate for super::Scope {
    fn admit(&self, destination: &Destination) -> Result<Vec<SocketAddr>, Reply> {
        super::Scope::admit(self, destination)
    }

    fn keeps(&self, address: SocketAddr) -> bool {
        super::Scope::keeps(self, address)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Connections open through the proxy now, including ones still negotiating.
    pub connections: usize,
    /// Bytes relayed in both directions since the proxy started.
    pub bytes: u64,
    /// Connections the proxy refused or could not open.
    pub refused: u64,
}

struct Shared {
    gate: Arc<dyn Gate>,
    active: AtomicUsize,
    bytes: AtomicU64,
    refused: AtomicU64,
    /// Every open socket, so stopping the proxy ends relays that are blocked in reads.
    sockets: Mutex<Sockets>,
    next: AtomicU64,
    relays: relays::Relays,
}

#[derive(Default)]
struct Sockets {
    stopped: bool,
    open: HashMap<u64, TcpStream>,
}

impl Shared {
    fn register(&self, socket: &TcpStream) -> io::Result<u64> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let clone = socket.try_clone()?;
        let mut sockets = self.sockets.lock().unwrap_or_else(PoisonError::into_inner);
        if sockets.stopped {
            return Err(io::Error::other("Local network proxy stopped"));
        }
        sockets.open.insert(id, clone);
        Ok(id)
    }

    fn release(&self, id: u64) {
        self.sockets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .open
            .remove(&id);
    }

    fn stop(&self) {
        let mut sockets = self.sockets.lock().unwrap_or_else(PoisonError::into_inner);
        sockets.stopped = true;
        for (_, socket) in sockets.open.drain() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }

    /// Ends every open relay once the byte budget is spent; new ones are refused before dialling.
    fn close_all(&self) {
        let sockets = self.sockets.lock().unwrap_or_else(PoisonError::into_inner);
        for socket in sockets.open.values() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }

    /// Takes up to `count` bytes from the budget: how many may be relayed, and whether that
    /// spends the budget.
    fn reserve(&self, count: u64) -> (u64, bool) {
        let mut allowed = 0;
        let used = self
            .bytes
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                allowed = count.min(BYTE_BUDGET.saturating_sub(used));
                Some(used + allowed)
            })
            .unwrap_or(BYTE_BUDGET);
        (allowed, used + allowed >= BYTE_BUDGET)
    }

    fn stopped(&self) -> bool {
        self.sockets.lock().unwrap_or_else(PoisonError::into_inner).stopped
    }

    fn over_budget(&self) -> bool {
        self.bytes.load(Ordering::Acquire) >= BYTE_BUDGET
    }

    fn refuse(&self) {
        self.refused.fetch_add(1, Ordering::Relaxed);
    }
}

/// One accepted connection's slot, registered sockets and relay record, released on every
/// exit path.
struct Slot {
    shared: Arc<Shared>,
    ids: Vec<u64>,
    relay: Option<u64>,
}

impl Slot {
    fn register(&mut self, socket: &TcpStream) -> io::Result<()> {
        self.ids.push(self.shared.register(socket)?);
        Ok(())
    }

    /// Lists the connection as relaying `requested` to `address` until the slot is released.
    fn relaying(&mut self, requested: Destination, address: SocketAddr) -> Arc<Entry> {
        let id = self.shared.next.fetch_add(1, Ordering::Relaxed);
        self.relay = Some(id);
        self.shared.relays.insert(id, requested, address, self.ids.clone())
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            self.shared.release(id);
        }
        if let Some(id) = self.relay.take() {
            self.shared.relays.remove(id);
        }
        self.shared.active.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) struct Proxy {
    port: u16,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

impl Proxy {
    /// # Errors
    /// Fails when the loopback listener cannot bind.
    pub(super) fn start(gate: Arc<dyn Gate>) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let shared = Arc::new(Shared {
            gate,
            active: AtomicUsize::new(0),
            bytes: AtomicU64::new(0),
            refused: AtomicU64::new(0),
            sockets: Mutex::default(),
            next: AtomicU64::new(0),
            relays: relays::Relays::default(),
        });
        let accept = {
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name("local-network-proxy".into())
                .spawn(move || accept(&listener, &shared))?
        };
        Ok(Self {
            port,
            shared,
            accept: Some(accept),
        })
    }

    pub(super) const fn port(&self) -> u16 {
        self.port
    }

    pub(super) fn counters(&self) -> Counters {
        Counters {
            connections: self.shared.active.load(Ordering::Acquire),
            bytes: self.shared.bytes.load(Ordering::Acquire),
            refused: self.shared.refused.load(Ordering::Acquire),
        }
    }

    /// The connections relaying now, oldest first.
    pub(super) fn relays(&self) -> Vec<Relay> {
        self.shared.relays.snapshot()
    }

    /// Closes every open relay whose dialled address `keep` no longer allows.
    pub(super) fn close_unless(&self, keep: impl Fn(SocketAddr) -> bool) {
        let rejected = self.shared.relays.rejected(keep);
        let sockets = self.shared.sockets.lock().unwrap_or_else(PoisonError::into_inner);
        for id in rejected {
            if let Some(socket) = sockets.open.get(&id) {
                let _ = socket.shutdown(Shutdown::Both);
            }
        }
    }

    /// Refuses everything from now on and closes every open connection, without waiting.
    pub(super) fn close(&self) {
        self.shared.stop();
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        // Connections accepted after this cannot register, so none outlives the proxy.
        self.shared.stop();
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

fn accept(listener: &TcpListener, shared: &Arc<Shared>) {
    while !shared.stopped() {
        match listener.accept() {
            Ok((socket, _)) => {
                if shared
                    .active
                    .try_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                        (active < MAX_CONNECTIONS).then_some(active + 1)
                    })
                    .is_err()
                {
                    shared.refuse();
                    continue;
                }
                let slot = Slot {
                    shared: Arc::clone(shared),
                    ids: Vec::with_capacity(2),
                    relay: None,
                };
                // A failed spawn drops the closure, and with it the slot and the socket.
                if thread::Builder::new()
                    .name("local-network-connection".into())
                    .spawn(move || serve(socket, slot))
                    .is_err()
                {
                    shared.refuse();
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(ACCEPT_POLL),
            // A client that reset before it was accepted, or a brief lack of descriptors,
            // must not end the proxy while it is on.
            Err(error) => {
                tracing::debug!(%error, "local network proxy accept failed");
                thread::sleep(ACCEPT_POLL);
            }
        }
    }
}

enum Refusal {
    /// Not SOCKS5, no acceptable method, a stalled or closed handshake: close without a reply.
    Silent,
    Reply(Reply),
}

fn serve(mut client: TcpStream, mut slot: Slot) {
    // Accepted sockets inherit the listener's nonblocking mode on Windows.
    if client.set_nonblocking(false).is_err() || slot.register(&client).is_err() {
        slot.shared.refuse();
        return;
    }
    let _ = client.set_nodelay(true);
    let shared = Arc::clone(&slot.shared);
    let opened = negotiate(&mut client, Instant::now() + HANDSHAKE_TIMEOUT).and_then(|destination| {
        let upstream = adopt(&mut slot, open(&shared, &destination)?)?;
        let address = upstream
            .peer_addr()
            .map_err(|_| Refusal::Reply(Reply::GeneralFailure))?;
        Ok((destination, address, upstream))
    });
    let (requested, address, upstream) = match opened {
        Ok(opened) => opened,
        Err(refusal) => {
            shared.refuse();
            if let Refusal::Reply(reply) = refusal {
                let _ = client.write_all(&reply_bytes(reply));
                linger(&mut client);
            }
            return;
        }
    };
    if client.set_read_timeout(None).is_err() || client.write_all(&reply_bytes(Reply::Succeeded)).is_err() {
        return;
    }
    let _ = upstream.set_nodelay(true);
    let entry = slot.relaying(requested, address);
    // Rules that changed after admission but before the relay was listed are applied here: a
    // change made after it was listed closes it through `close_unless`.
    if !shared.gate.keeps(address) {
        return;
    }
    relay(&shared, &entry, client, upstream);
}

/// Registers a new upstream connection so stopping or spending the budget closes it, and
/// refuses it if either already happened while it was being opened. Registering first means
/// a later stop or budget close sees it, and checking afterwards covers one that came before.
fn adopt(slot: &mut Slot, upstream: TcpStream) -> Result<TcpStream, Refusal> {
    if slot.register(&upstream).is_err() || slot.shared.over_budget() {
        return Err(Refusal::Reply(Reply::GeneralFailure));
    }
    Ok(upstream)
}

fn open(shared: &Shared, destination: &Destination) -> Result<TcpStream, Refusal> {
    if shared.over_budget() || shared.stopped() {
        return Err(Refusal::Reply(Reply::GeneralFailure));
    }
    let candidates = shared.gate.admit(destination).map_err(Refusal::Reply)?;
    let deadline = Instant::now() + OPEN_TIMEOUT;
    let mut failure = Reply::HostUnreachable;
    for address in candidates {
        // Admission can take seconds; a bridge stopped or spent meanwhile dials nothing more.
        if shared.over_budget() || shared.stopped() {
            return Err(Refusal::Reply(Reply::GeneralFailure));
        }
        let Some(left) = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
        else {
            break;
        };
        match TcpStream::connect_timeout(&address, left.min(CONNECT_TIMEOUT)) {
            Ok(stream) => return Ok(stream),
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => failure = Reply::ConnectionRefused,
            Err(_) => {}
        }
    }
    Err(Refusal::Reply(failure))
}

/// Ends the client's side first and reads what it still sends, briefly, before the close.
fn linger(client: &mut TcpStream) {
    let _ = client.shutdown(Shutdown::Write);
    let _ = client.set_read_timeout(Some(LINGER));
    let deadline = Instant::now() + LINGER;
    let mut sink = [0; 512];
    while Instant::now() < deadline && matches!(client.read(&mut sink), Ok(count) if count > 0) {}
}

fn reply_bytes(reply: Reply) -> [u8; 10] {
    [VERSION, reply.code(), 0, 1, 0, 0, 0, 0, 0, 0]
}

/// Reads exactly `buffer.len()` bytes before `deadline`, however slowly they trickle in.
fn read_by(socket: &mut TcpStream, buffer: &mut [u8], deadline: Instant) -> Result<(), Refusal> {
    let mut filled = 0;
    while filled < buffer.len() {
        let left = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or(Refusal::Silent)?;
        socket.set_read_timeout(Some(left)).map_err(|_| Refusal::Silent)?;
        match socket.read(&mut buffer[filled..]) {
            Ok(0) => return Err(Refusal::Silent),
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(Refusal::Silent),
        }
    }
    Ok(())
}

fn negotiate(socket: &mut TcpStream, deadline: Instant) -> Result<Destination, Refusal> {
    let mut greeting = [0; 2];
    read_by(socket, &mut greeting, deadline)?;
    if greeting[0] != VERSION {
        return Err(Refusal::Silent);
    }
    let mut methods = vec![0; usize::from(greeting[1])];
    read_by(socket, &mut methods, deadline)?;
    if !methods.contains(&NO_AUTHENTICATION) {
        if socket.write_all(&[VERSION, NO_ACCEPTABLE_METHOD]).is_ok() {
            linger(socket);
        }
        return Err(Refusal::Silent);
    }
    socket
        .write_all(&[VERSION, NO_AUTHENTICATION])
        .map_err(|_| Refusal::Silent)?;
    let mut request = [0; 4];
    read_by(socket, &mut request, deadline)?;
    if request[0] != VERSION || request[2] != 0 {
        return Err(Refusal::Reply(Reply::GeneralFailure));
    }
    // The whole request is read before it is judged, so a refusal is not followed by unread input.
    let destination = match request[3] {
        1 => {
            let mut address = [0; 4];
            read_by(socket, &mut address, deadline)?;
            Some(Destination::Address(SocketAddr::new(Ipv4Addr::from(address).into(), 0)))
        }
        3 => {
            let mut length = [0; 1];
            read_by(socket, &mut length, deadline)?;
            let mut name = vec![0; usize::from(length[0])];
            read_by(socket, &mut name, deadline)?;
            name_destination(&name)
        }
        4 => {
            let mut address = [0; 16];
            read_by(socket, &mut address, deadline)?;
            Some(Destination::Address(SocketAddr::new(Ipv6Addr::from(address).into(), 0)))
        }
        _ => return Err(Refusal::Reply(Reply::AddressTypeNotSupported)),
    };
    let mut port = [0; 2];
    read_by(socket, &mut port, deadline)?;
    let port = u16::from_be_bytes(port);
    if request[1] != CONNECT {
        return Err(Refusal::Reply(Reply::CommandNotSupported));
    }
    let destination = destination.ok_or(Refusal::Reply(Reply::AddressTypeNotSupported))?;
    if port == 0 {
        return Err(Refusal::Reply(Reply::NotAllowed));
    }
    Ok(match destination {
        Destination::Address(address) => Destination::Address(SocketAddr::new(address.ip(), port)),
        Destination::Name(name, _) => Destination::Name(name, port),
    })
}

/// A DNS name as the gate will resolve it, or a literal address sent in name form.
fn name_destination(name: &[u8]) -> Option<Destination> {
    let name = std::str::from_utf8(name).ok()?;
    if let Ok(address) = name.parse::<IpAddr>() {
        return Some(Destination::Address(SocketAddr::new(address, 0)));
    }
    let labels = name.strip_suffix('.').unwrap_or(name);
    let valid = !labels.is_empty()
        && labels.len() <= 253
        && labels.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        });
    valid.then(|| Destination::Name(name.to_owned(), 0))
}

fn relay(shared: &Arc<Shared>, entry: &Arc<Entry>, client: TcpStream, upstream: TcpStream) {
    let (Ok(client_reader), Ok(upstream_writer)) = (client.try_clone(), upstream.try_clone()) else {
        return;
    };
    let downstream = {
        let (shared, entry) = (Arc::clone(shared), Arc::clone(entry));
        thread::Builder::new()
            .name("local-network-relay".into())
            .spawn(move || copy(&shared, &entry, upstream, client))
    };
    let Ok(downstream) = downstream else {
        let _ = client_reader.shutdown(Shutdown::Both);
        let _ = upstream_writer.shutdown(Shutdown::Both);
        return;
    };
    copy(shared, entry, client_reader, upstream_writer);
    let _ = downstream.join();
}

/// Writes as much of `bytes` as the peer takes and returns how much that was.
fn write_counted(to: &mut TcpStream, bytes: &[u8]) -> usize {
    let mut written = 0;
    while written < bytes.len() {
        match to.write(&bytes[written..]) {
            Ok(0) => break,
            Ok(count) => written += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    written
}

/// Copies until end of input, then half-closes the other side so each direction ends on its own.
fn copy(shared: &Shared, entry: &Entry, mut from: TcpStream, mut to: TcpStream) {
    let mut buffer = vec![0; BUFFER_BYTES];
    loop {
        match from.read(&mut buffer) {
            Ok(0) => {
                let _ = to.shutdown(Shutdown::Write);
                return;
            }
            Ok(count) => {
                let (allowed, spent) = shared.reserve(count as u64);
                let allowed = usize::try_from(allowed).unwrap_or(count);
                let written = write_counted(&mut to, &buffer[..allowed]);
                entry.bytes.fetch_add(written as u64, Ordering::AcqRel);
                if written < allowed {
                    // Undelivered bytes go back to the budget and out of the count.
                    shared.bytes.fetch_sub((allowed - written) as u64, Ordering::AcqRel);
                    break;
                }
                if spent {
                    shared.close_all();
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let _ = from.shutdown(Shutdown::Both);
    let _ = to.shutdown(Shutdown::Both);
}

mod relays;
use relays::Entry;
pub use relays::Relay;

#[cfg(test)]
mod tests;
