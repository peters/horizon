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
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
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

/// The control socket, removed on drop only while it is still this helper's.
struct Control {
    listener: UnixListener,
    path: PathBuf,
    lock: PathBuf,
    identity: (u64, u64),
}

impl Control {
    /// A newer session takes the control socket over from a helper whose session was lost,
    /// which then ends; it refuses while that helper's owner is still heard from. Asking and
    /// replacing happen under one lock, so two new sessions cannot both take over.
    fn bind(paths: &Paths) -> io::Result<Self> {
        let path = paths.control();
        let lock = paths.lock();
        let _guard = Locked::new(&lock)?;
        // A retiring helper removes its socket only under this lock, after checking it is its own.
        retire_running(&path)?;
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        let metadata = std::fs::symlink_metadata(&path)?;
        Ok(Self {
            listener,
            path,
            lock,
            identity: (metadata.dev(), metadata.ino()),
        })
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        let Ok(_guard) = Locked::new(&self.lock) else {
            return;
        };
        if std::fs::symlink_metadata(&self.path).is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == self.identity)
        {
            let _ = std::fs::remove_file(&self.path);
        }
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

/// An exclusive lock on the directory's lock file while replacing or removing the control socket.
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

/// One control request's share of [`MAX_REQUESTS`], returned however its handler ends.
struct Pending(Arc<AtomicUsize>);

impl Drop for Pending {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn serve(helper: &Arc<Helper>, control: &Control) {
    let requests = Arc::new(AtomicUsize::new(0));
    while helper.owner.alive() && !helper.retired.load(Ordering::Acquire) {
        match control.listener.accept() {
            Ok((mut stream, _)) => {
                if requests
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                        (count < MAX_REQUESTS).then_some(count + 1)
                    })
                    .is_err()
                {
                    // Accepted sockets may inherit nonblocking mode; a busy answer never waits.
                    let _ = serde_json::to_writer(&mut stream, &Answer::Error(BUSY.into()));
                    let _ = stream.write_all(b"\n");
                    continue;
                }
                let share = Pending(Arc::clone(&requests));
                let helper = Arc::clone(helper);
                let _ = thread::Builder::new()
                    .name("local-network-control".into())
                    .spawn(move || {
                        let _share = share;
                        let _ = handle(&helper, stream);
                    });
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            // A peer that went away before it was accepted, or a brief lack of descriptors,
            // must not end the session.
            Err(_) => thread::sleep(POLL),
        }
    }
}

fn handle(helper: &Helper, mut stream: UnixStream) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut line = String::new();
    let count = BufReader::new(&stream).take(MAX_MESSAGE + 1).read_line(&mut line)?;
    let answer = if count as u64 > MAX_MESSAGE {
        Answer::Error("Request too large".into())
    } else {
        serde_json::from_str(&line).map_or_else(
            |_| Answer::Error("Invalid request".into()),
            |request| helper.answer(request),
        )
    };
    serde_json::to_writer(&mut stream, &answer)?;
    stream.write_all(b"\n")
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
