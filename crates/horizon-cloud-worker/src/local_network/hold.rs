//! The helper at the worker end of one bridge session. It lives exactly as long as the owner's
//! SSH session keeps writing heartbeats, and takes its sockets and forwards with it.
use super::{Answer, Availability, Forward, MAX_MESSAGE, Paths, Request, Status, forward, owner::Owner};
use horizon_cloud_protocol::local_network::{
    HEARTBEAT_INTERVAL, Nonce, Ready, Subnet,
    discovery::{self, Source},
};
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddrV4},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const SOCKET_WAIT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(50);
const MAX_FORWARDS: usize = 16;
const MAX_REQUESTS: usize = 8;
/// A helper whose owner has written nothing for this long has lost its session and makes way
/// for a newer one; a helper heard from more recently refuses to.
const RETIRE_AFTER: Duration = Duration::from_secs(2 * HEARTBEAT_INTERVAL.as_secs());
const BUSY: &str = "The bridge is busy; try again";
const NO_PROBE: &str = "The owner's Horizon does not support probes yet; ask the owner to update Horizon";
const UNEXPECTED: &str = "The owner's Horizon answered with something else";
const RETIRE_TIMEOUT: Duration = Duration::from_secs(10);
/// The whole of one request must arrive within this, however slowly it trickles in.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const PRIVATE: &str = "Only a bridge helper can ask for that";
const NOTE: &str = "TCP only. Names are resolved on the owner's computer. Every process on this worker, including web pages open in its browsers, can use the proxy and forwards while the bridge is on. Forwards end when the bridge stops or reconnects; check the status and forward again.";

struct Pinned {
    forward: Forward,
    _listener: forward::Listener,
}

struct Helper {
    bridge: PathBuf,
    subnet: Subnet,
    proxy: SocketAddrV4,
    relays: Arc<AtomicUsize>,
    forwards: Mutex<Vec<Pinned>>,
    owner: Owner,
    /// Set when a newer session took over; the helper then ends.
    retired: AtomicBool,
}

impl Helper {
    fn status(&self) -> Status {
        Status {
            active: true,
            subnet: Some(self.subnet.to_string()),
            proxy: Some(self.proxy.to_string()),
            forwards: self.forwards().iter().map(|pinned| pinned.forward.clone()).collect(),
            discovery: Some(self.availability()),
            note: NOTE.into(),
        }
    }

    fn availability(&self) -> Availability {
        match self.owner.hello().filter(|hello| hello.discovery > 0) {
            Some(hello) => Availability {
                available: true,
                probe: hello.sources.contains(&Source::Probe),
                sources: hello
                    .sources
                    .iter()
                    .filter(|source| !matches!(source, Source::Other | Source::Probe))
                    .filter_map(|source| serde_json::to_value(source).ok()?.as_str().map(str::to_owned))
                    .collect(),
                note: hello.note.map(|note| discovery::text(&note)),
            },
            None => Availability {
                available: false,
                probe: false,
                sources: Vec::new(),
                note: Some(super::owner::NO_DISCOVERY.into()),
            },
        }
    }

    /// Checked here as well as on the owner's computer, so a bad request fails at once.
    fn probe(&self, host: String, ports: Vec<u16>) -> Answer {
        let request = discovery::Request::Probe { host, ports };
        if let Err(refusal) = request.validate() {
            return Answer::Error(refusal);
        }
        if self
            .owner
            .hello()
            .is_some_and(|hello| !hello.sources.contains(&Source::Probe))
        {
            return Answer::Error(NO_PROBE.into());
        }
        match self.owner.ask(request) {
            Ok(discovery::Answer::Probe(probe)) => Answer::Probe(probe),
            Ok(discovery::Answer::Refused(refusal)) => Answer::Error(discovery::text(&refusal)),
            Ok(discovery::Answer::Discovery(_)) => Answer::Error(UNEXPECTED.into()),
            Err(error) => Answer::Error(error.to_string()),
        }
    }

    fn forwards(&self) -> std::sync::MutexGuard<'_, Vec<Pinned>> {
        self.forwards.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn answer(&self, request: Request) -> Answer {
        match request {
            Request::Status => Answer::Status(self.status()),
            Request::Retire => {
                if self.owner.silent_for() < RETIRE_AFTER {
                    return Answer::Error(
                        "Another Horizon is already sharing its local network with this worker".into(),
                    );
                }
                self.retired.store(true, Ordering::Release);
                Answer::Retired
            }
            Request::Forward { host, port } => match self.forward(host, port) {
                Ok(forward) => Answer::Forward(forward),
                Err(error) => Answer::Error(error.to_string()),
            },
            Request::Discover => match self.owner.ask(discovery::Request::Discover) {
                Ok(discovery::Answer::Discovery(found)) => Answer::Discovery(found),
                Ok(discovery::Answer::Refused(refusal)) => Answer::Error(discovery::text(&refusal)),
                Ok(discovery::Answer::Probe(_)) => Answer::Error(UNEXPECTED.into()),
                Err(error) => Answer::Error(error.to_string()),
            },
            Request::Probe { host, ports } => self.probe(host, ports),
            Request::Unforward { worker_port } => {
                let removed = {
                    let mut forwards = self.forwards();
                    let before = forwards.len();
                    forwards.retain(|pinned| pinned.forward.worker_port != worker_port);
                    before != forwards.len()
                };
                if removed {
                    Answer::Status(self.status())
                } else {
                    Answer::Error(format!("No forward listens on worker port {worker_port}"))
                }
            }
        }
    }

    fn forward(&self, host: String, port: u16) -> io::Result<Forward> {
        if let Some(pinned) = self
            .forwards()
            .iter()
            .find(|pinned| pinned.forward.host == host && pinned.forward.port == port)
        {
            return Ok(pinned.forward.clone());
        }
        if self.forwards().len() >= MAX_FORWARDS {
            return Err(io::Error::other(format!(
                "At most {MAX_FORWARDS} forwards; remove one first"
            )));
        }
        // The owner's proxy decides before anything listens here.
        drop(forward::connect(&self.bridge, &host, port)?);
        let listener = {
            let (bridge, host) = (self.bridge.clone(), host.clone());
            forward::Listener::start(
                Arc::clone(&self.relays),
                Arc::new(move || forward::connect(&bridge, &host, port)),
            )?
        };
        let pinned = Pinned {
            forward: Forward {
                worker_port: listener.port(),
                host,
                port,
            },
            _listener: listener,
        };
        let forward = pinned.forward.clone();
        let mut forwards = self.forwards();
        // A concurrent identical request may have finished first.
        if let Some(existing) = forwards
            .iter()
            .find(|existing| existing.forward.host == forward.host && existing.forward.port == port)
        {
            return Ok(existing.forward.clone());
        }
        if forwards.len() >= MAX_FORWARDS {
            return Err(io::Error::other(format!(
                "At most {MAX_FORWARDS} forwards; remove one first"
            )));
        }
        forwards.push(pinned);
        Ok(forward)
    }
}

/// Who reached the helper, by the socket they used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Caller {
    /// The private control socket: root, and a newer session's helper.
    Private,
    /// The agent socket, open to every local user.
    Agent,
}

/// One listening socket, removed on drop only while it is still this helper's.
struct Socket {
    listener: UnixListener,
    path: PathBuf,
    identity: (u64, u64),
}

impl Socket {
    /// Binds at `staging` in the private directory, sets `mode`, then moves the socket over
    /// whatever is at `path` in one step. Callers hold the lock.
    fn bind(staging: &Path, path: PathBuf, mode: u32) -> io::Result<Self> {
        match std::fs::remove_file(staging) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(staging)?;
        let placed = (|| {
            listener.set_nonblocking(true)?;
            std::fs::set_permissions(staging, std::fs::Permissions::from_mode(mode))?;
            std::fs::rename(staging, &path)
        })();
        if let Err(error) = placed {
            let _ = std::fs::remove_file(staging);
            return Err(error);
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        Ok(Self {
            listener,
            path,
            identity: (metadata.dev(), metadata.ino()),
        })
    }

    /// Removes the socket file if it is still this one. Callers hold the lock.
    fn remove(&self) {
        if std::fs::symlink_metadata(&self.path).is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == self.identity)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// The helper's sockets: the private control socket, and the agent socket that answers only
/// what agents may ask.
struct Control {
    private: Socket,
    agent: Socket,
    lock: PathBuf,
}

impl Control {
    /// A newer session takes the sockets over from a helper whose session was lost, which then
    /// ends; it refuses while that helper's owner is still heard from. Asking and replacing
    /// happen under one lock, so two new sessions cannot both take over.
    fn bind(paths: &Paths) -> io::Result<Self> {
        let lock = paths.lock();
        let _guard = Locked::new(&lock)?;
        // A retiring helper removes its sockets only under this lock, after checking they are its own.
        retire_running(&paths.control())?;
        let private = Socket::bind(&paths.staging("control"), paths.control(), 0o600)?;
        let agent = match Socket::bind(&paths.staging("agent"), paths.agent.clone(), 0o666) {
            Ok(agent) => agent,
            Err(error) => {
                private.remove();
                return Err(error);
            }
        };
        Ok(Self { private, agent, lock })
    }

    /// Accepts one connection from either socket, if one is waiting.
    fn accept(&self) -> io::Result<(UnixStream, Caller)> {
        match self.private.listener.accept() {
            Ok((stream, _)) => Ok((stream, Caller::Private)),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                self.agent.listener.accept().map(|(stream, _)| (stream, Caller::Agent))
            }
            Err(error) => Err(error),
        }
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        let Ok(_guard) = Locked::new(&self.lock) else {
            return;
        };
        self.agent.remove();
        self.private.remove();
    }
}

/// Succeeds only when no helper serves `path`, or the one there agreed to retire; an
/// inconclusive answer keeps the running bridge.
fn retire_running(path: &Path) -> io::Result<()> {
    let mut stream = match UnixStream::connect(path) {
        Ok(stream) => stream,
        Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let answer = (|| -> io::Result<Option<String>> {
        stream.set_read_timeout(Some(RETIRE_TIMEOUT))?;
        stream.set_write_timeout(Some(RETIRE_TIMEOUT))?;
        serde_json::to_writer(&mut stream, &Request::Retire)?;
        stream.write_all(b"\n")?;
        super::read_line(&mut BufReader::new(&stream))
    })()
    .ok()
    .flatten()
    .and_then(|line| serde_json::from_str(&line).ok());
    match answer {
        Some(Answer::Retired) => Ok(()),
        Some(Answer::Error(error)) => Err(io::Error::other(error)),
        _ => Err(io::Error::other(
            "The bridge helper already on this worker did not answer; try again",
        )),
    }
}

/// An exclusive lock on the directory's lock file while replacing or removing the sockets.
struct Locked(File);

impl Locked {
    fn new(path: &Path) -> io::Result<Self> {
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(path)?;
        file.lock()?;
        Ok(Self(file))
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Removes this session's bridge socket, which sshd leaves behind.
struct Bridge(PathBuf);

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub(super) fn run(
    paths: &Paths,
    nonce: &str,
    subnet: &str,
    input: impl Read + Send + 'static,
    output: impl Write + Send + 'static,
) -> io::Result<()> {
    let nonce = Nonce::parse(nonce).ok_or_else(|| io::Error::other("Invalid bridge session"))?;
    let subnet: Subnet = subnet.parse().map_err(io::Error::other)?;
    let bridge = Bridge(paths.bridge(&nonce));
    super::prepare(paths)?;
    wait_for_socket(&bridge.0)?;
    let relays = Arc::new(AtomicUsize::new(0));
    let endpoint = {
        let bridge = bridge.0.clone();
        forward::Listener::start(Arc::clone(&relays), Arc::new(move || UnixStream::connect(&bridge)))?
    };
    let helper = Arc::new(Helper {
        bridge: bridge.0.clone(),
        subnet,
        proxy: SocketAddrV4::new(Ipv4Addr::LOCALHOST, endpoint.port()),
        relays,
        forwards: Mutex::new(Vec::new()),
        owner: Owner::watch(input, output)?,
        retired: AtomicBool::new(false),
    });
    let control = Control::bind(paths)?;
    helper.owner.write(&Ready { proxy: helper.proxy })?;
    serve(&helper, &control);
    helper.forwards().clear();
    drop(endpoint);
    Ok(())
}

fn wait_for_socket(path: &Path) -> io::Result<()> {
    let deadline = Instant::now() + SOCKET_WAIT;
    while !std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_socket()) {
        if Instant::now() >= deadline {
            return Err(io::Error::other("The bridge socket did not appear"));
        }
        thread::sleep(POLL);
    }
    Ok(())
}

/// One request's share of its socket's [`MAX_REQUESTS`], returned however its handler ends.
struct Pending(Arc<AtomicUsize>);

impl Drop for Pending {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn serve(helper: &Arc<Helper>, control: &Control) {
    // Each socket has its own budget, so agents cannot crowd out a newer session's helper.
    let (private, agents) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    while helper.owner.alive() && !helper.retired.load(Ordering::Acquire) {
        match control.accept() {
            Ok((mut stream, caller)) => {
                let requests = match caller {
                    Caller::Private => &private,
                    Caller::Agent => &agents,
                };
                if requests
                    .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                        (count < MAX_REQUESTS).then_some(count + 1)
                    })
                    .is_err()
                {
                    // Accepted sockets may inherit nonblocking mode; a busy answer never waits.
                    let _ = serde_json::to_writer(&mut stream, &Answer::Error(BUSY.into()));
                    let _ = stream.write_all(b"\n");
                    continue;
                }
                let share = Pending(Arc::clone(requests));
                let helper = Arc::clone(helper);
                let _ = thread::Builder::new()
                    .name("local-network-control".into())
                    .spawn(move || {
                        let _share = share;
                        let _ = handle(&helper, &stream, caller);
                    });
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            // A peer that went away before it was accepted, or a brief lack of descriptors,
            // must not end the session.
            Err(_) => thread::sleep(POLL),
        }
    }
}

/// Reads a request, or writes its answer, against a single deadline, so a client that sends
/// or reads a byte at a time cannot hold a share of the request budget for long.
struct Deadline<'a> {
    stream: &'a UnixStream,
    until: Instant,
}

impl Deadline<'_> {
    fn left(&self) -> io::Result<Duration> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "The client was too slow"));
        }
        Ok(left)
    }
}

impl Read for Deadline<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.left()?))?;
        let mut stream = self.stream;
        stream.read(buffer)
    }
}

impl Write for Deadline<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.left()?))?;
        let mut stream = self.stream;
        stream.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn handle(helper: &Helper, stream: &UnixStream, caller: Caller) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    let mut line = String::new();
    let request = Deadline {
        stream,
        until: Instant::now() + REQUEST_TIMEOUT,
    };
    let count = BufReader::new(request).take(MAX_MESSAGE + 1).read_line(&mut line)?;
    let answer = if count as u64 > MAX_MESSAGE {
        Answer::Error("Request too large".into())
    } else {
        match serde_json::from_str::<Request>(&line) {
            Err(_) => Answer::Error("Invalid request".into()),
            Ok(request) if caller == Caller::Agent && !request.for_agents() => Answer::Error(PRIVATE.into()),
            Ok(request) => helper.answer(request),
        }
    };
    let mut reply = serde_json::to_vec(&answer)?;
    reply.push(b'\n');
    send(stream, &reply, REQUEST_TIMEOUT)
}

/// Writes the whole answer within `timeout`, so a client that never reads it frees its share.
fn send(stream: &UnixStream, reply: &[u8], timeout: Duration) -> io::Result<()> {
    Deadline {
        stream,
        until: Instant::now() + timeout,
    }
    .write_all(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn helper(silent: Duration) -> Helper {
        Helper {
            bridge: PathBuf::from("/nonexistent"),
            subnet: "192.168.1.0/24".parse().unwrap(),
            proxy: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1),
            relays: Arc::new(AtomicUsize::new(0)),
            forwards: Mutex::new(Vec::new()),
            owner: Owner::silent(silent),
            retired: AtomicBool::new(false),
        }
    }

    #[test]
    fn an_answer_that_nobody_reads_gives_up_at_the_deadline() {
        let (stream, _unread) = UnixStream::pair().unwrap();
        let started = Instant::now();
        // Far more than a Unix socket buffers, as a large discovery answer can be.
        assert!(send(&stream, &vec![b'x'; 16 * 1024 * 1024], Duration::from_millis(300)).is_err());
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[test]
    fn only_a_helper_whose_owner_went_silent_makes_way() {
        let live = helper(Duration::ZERO);
        assert!(matches!(live.answer(Request::Retire), Answer::Error(error) if error.contains("Another Horizon")));
        assert!(!live.retired.load(Ordering::Acquire));
        let lost = helper(RETIRE_AFTER + Duration::from_secs(1));
        if lost.owner.silent_for() >= RETIRE_AFTER {
            assert_eq!(lost.answer(Request::Retire), Answer::Retired);
            assert!(lost.retired.load(Ordering::Acquire));
        }
    }
}
