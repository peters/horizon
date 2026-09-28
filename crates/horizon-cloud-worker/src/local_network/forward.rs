//! Worker loopback listeners whose connections travel over the bridge socket, either as a
//! SOCKS5 CONNECT to one pinned destination or as the raw SOCKS5 endpoint for tools.
use horizon_cloud_protocol::local_network::Reply;
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, Shutdown, TcpListener, TcpStream},
    os::unix::net::UnixStream,
    path::Path,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// Longer than the owner's proxy spends resolving and dialling before it answers.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(40);
const POLL: Duration = Duration::from_millis(25);
const BUFFER_BYTES: usize = 16 * 1024;
/// Relayed connections per bridge session on this worker, as many as the owner's proxy allows.
pub(super) const MAX_RELAYS: usize = 64;

/// Opens `host:port` through the owner's proxy, which admits or refuses it.
///
/// # Errors
/// Returns the refusal agents see, or that the bridge is off.
pub(super) fn connect(bridge: &Path, host: &str, port: u16) -> io::Result<UnixStream> {
    let request = request(host, port)?;
    let mut stream = UnixStream::connect(bridge).map_err(|_| io::Error::other(super::OFF))?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let closed =
        |_| io::Error::other("The bridge closed the connection; it may be at its connection limit or stopping");
    let unexpected = || io::Error::other("The bridge answered with an unexpected protocol");
    stream.write_all(&[5, 1, 0]).map_err(closed)?;
    let mut choice = [0; 2];
    stream.read_exact(&mut choice).map_err(closed)?;
    if choice != [5, 0] {
        return Err(unexpected());
    }
    stream.write_all(&request).map_err(closed)?;
    let mut head = [0; 4];
    stream.read_exact(&mut head).map_err(closed)?;
    if head[0] != 5 {
        return Err(unexpected());
    }
    let bound = match head[3] {
        1 => 4,
        4 => 16,
        _ => return Err(unexpected()),
    };
    let mut rest = vec![0; bound + 2];
    stream.read_exact(&mut rest).map_err(closed)?;
    let reply = Reply::from_code(head[1]);
    if reply != Reply::Succeeded {
        return Err(io::Error::other(reply.message()));
    }
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    Ok(stream)
}

/// A SOCKS5 CONNECT for an address literal or a name the owner's computer resolves.
fn request(host: &str, port: u16) -> io::Result<Vec<u8>> {
    if port == 0 {
        return Err(io::Error::other("Invalid port"));
    }
    let mut request = vec![5, 1, 0];
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => {
            request.push(1);
            request.extend(address.octets());
        }
        Ok(IpAddr::V6(address)) => {
            request.push(4);
            request.extend(address.octets());
        }
        Err(_) => {
            let length = u8::try_from(host.len())
                .ok()
                .filter(|length| *length > 0)
                .filter(|_| host.bytes().all(|byte| byte.is_ascii_graphic()))
                .ok_or_else(|| io::Error::other("Invalid device address"))?;
            request.push(3);
            request.push(length);
            request.extend(host.as_bytes());
        }
    }
    request.extend(port.to_be_bytes());
    Ok(request)
}

/// Every open local connection of one listener, so closing it ends them.
#[derive(Default)]
struct Streams {
    stopped: bool,
    next: u64,
    open: HashMap<u64, Vec<Box<dyn Half + Send>>>,
}

impl Streams {
    /// Keeps `stream` until its connection ends, or refuses once the listener is closed.
    fn register(&mut self, id: u64, stream: Box<dyn Half + Send>) -> bool {
        if self.stopped {
            stream.close(Shutdown::Both);
            return false;
        }
        self.open.entry(id).or_default().push(stream);
        true
    }
}

/// One relayed connection's share of [`MAX_RELAYS`], returned on every exit path.
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(relays: &Arc<AtomicUsize>) -> Option<Self> {
        relays
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_RELAYS).then_some(count + 1)
            })
            .ok()
            .map(|_| Self(Arc::clone(relays)))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

type Dial = Arc<dyn Fn() -> io::Result<UnixStream> + Send + Sync>;

/// A `127.0.0.1` listener on this worker; dropping it closes the listener and its connections.
pub(super) struct Listener {
    port: u16,
    streams: Arc<Mutex<Streams>>,
    accept: Option<JoinHandle<()>>,
}

impl Listener {
    pub(super) fn start(relays: Arc<AtomicUsize>, dial: Dial) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let streams = Arc::new(Mutex::new(Streams::default()));
        let accept = {
            let streams = Arc::clone(&streams);
            thread::Builder::new()
                .name("local-network-listener".into())
                .spawn(move || accept(&listener, &streams, &relays, &dial))?
        };
        Ok(Self {
            port,
            streams,
            accept: Some(accept),
        })
    }

    pub(super) const fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        {
            let mut streams = self.streams.lock().unwrap_or_else(PoisonError::into_inner);
            streams.stopped = true;
            for stream in streams.open.drain().flat_map(|(_, streams)| streams) {
                stream.close(Shutdown::Both);
            }
        }
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

fn accept(listener: &TcpListener, streams: &Arc<Mutex<Streams>>, relays: &Arc<AtomicUsize>, dial: &Dial) {
    while !streams.lock().unwrap_or_else(PoisonError::into_inner).stopped {
        match listener.accept() {
            Ok((local, _)) => {
                let Some(slot) = Slot::take(relays) else {
                    continue;
                };
                let (streams, dial) = (Arc::clone(streams), Arc::clone(dial));
                let _ = thread::Builder::new()
                    .name("local-network-relay".into())
                    .spawn(move || serve(local, &streams, &dial, slot));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            // A peer that went away before it was accepted, or a brief lack of descriptors,
            // must not close the listener.
            Err(_) => thread::sleep(POLL),
        }
    }
}

fn serve(local: TcpStream, streams: &Mutex<Streams>, dial: &Dial, _slot: Slot) {
    if local.set_nonblocking(false).is_err() {
        return;
    }
    let Ok(clone) = local.try_clone() else { return };
    let id = {
        let mut streams = streams.lock().unwrap_or_else(PoisonError::into_inner);
        let id = streams.next;
        streams.next += 1;
        if !streams.register(id, Box::new(clone)) {
            return;
        }
        id
    };
    if let Ok(remote) = dial()
        && let Ok(clone) = remote.try_clone()
        && streams
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .register(id, Box::new(clone))
    {
        relay(local, remote);
    }
    streams.lock().unwrap_or_else(PoisonError::into_inner).open.remove(&id);
}

fn relay(local: TcpStream, remote: UnixStream) {
    let (Ok(local_writer), Ok(remote_reader)) = (local.try_clone(), remote.try_clone()) else {
        return;
    };
    let Ok(downstream) = thread::Builder::new()
        .name("local-network-relay".into())
        .spawn(move || copy(remote_reader, local_writer))
    else {
        return;
    };
    copy(local, remote);
    let _ = downstream.join();
}

trait Half: Read + Write {
    fn close(&self, how: Shutdown);
}

impl Half for TcpStream {
    fn close(&self, how: Shutdown) {
        let _ = self.shutdown(how);
    }
}

impl Half for UnixStream {
    fn close(&self, how: Shutdown) {
        let _ = self.shutdown(how);
    }
}

/// Copies until end of input and half-closes the other side; on failure closes both.
fn copy(mut from: impl Half, mut to: impl Half) {
    let mut buffer = vec![0; BUFFER_BYTES];
    loop {
        match from.read(&mut buffer) {
            Ok(0) => return to.close(Shutdown::Write),
            Ok(count) => {
                if to.write_all(&buffer[..count]).is_err() {
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    from.close(Shutdown::Both);
    to.close(Shutdown::Both);
}
