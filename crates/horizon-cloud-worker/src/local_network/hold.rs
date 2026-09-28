//! The helper at the worker end of one bridge session. It lives exactly as long as the owner's
//! SSH session keeps writing heartbeats, and takes its sockets and forwards with it.
use super::{Answer, Forward, MAX_MESSAGE, Paths, Request, Status, forward};
use horizon_cloud_protocol::local_network::{HEARTBEAT_INTERVAL, HEARTBEAT_TIMEOUT, Nonce, Ready, Subnet};
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
    heartbeat: Heartbeat,
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
            note: NOTE.into(),
        }
    }

    fn forwards(&self) -> std::sync::MutexGuard<'_, Vec<Pinned>> {
        self.forwards.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn answer(&self, request: Request) -> Answer {
        match request {
            Request::Status => Answer::Status(self.status()),
            Request::Retire => {
                if self.heartbeat.silent_for() < RETIRE_AFTER {
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
        // Otherwise the helper there retired, is absent or does not answer: the socket is free.
        // A retiring helper removes its socket only under this lock, after checking it is its own.
        if let Ok(Answer::Error(error)) = super::exchange(paths, &Request::Retire) {
            return Err(io::Error::other(error));
        }
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
    mut output: impl Write,
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
        heartbeat: Heartbeat::watch(input)?,
        retired: AtomicBool::new(false),
    });
    let control = Control::bind(paths)?;
    serde_json::to_writer(&mut output, &Ready { proxy: helper.proxy })?;
    output.write_all(b"\n")?;
    output.flush()?;
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

/// When the owner's session last wrote to standard input, and whether it closed it.
struct Heartbeat {
    last: Arc<Mutex<Instant>>,
    ended: Arc<AtomicBool>,
}

impl Heartbeat {
    fn watch(mut input: impl Read + Send + 'static) -> io::Result<Self> {
        let last = Arc::new(Mutex::new(Instant::now()));
        let ended = Arc::new(AtomicBool::new(false));
        {
            let (last, ended) = (Arc::clone(&last), Arc::clone(&ended));
            thread::Builder::new()
                .name("local-network-heartbeat".into())
                .spawn(move || {
                    let mut buffer = [0; 64];
                    loop {
                        match input.read(&mut buffer) {
                            Ok(0) => break,
                            Ok(_) => *last.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now(),
                            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                            Err(_) => break,
                        }
                    }
                    ended.store(true, Ordering::Release);
                })?;
        }
        Ok(Self { last, ended })
    }

    fn silent_for(&self) -> Duration {
        self.last.lock().unwrap_or_else(PoisonError::into_inner).elapsed()
    }

    fn alive(&self) -> bool {
        !self.ended.load(Ordering::Acquire) && self.silent_for() < HEARTBEAT_TIMEOUT
    }
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
    while helper.heartbeat.alive() && !helper.retired.load(Ordering::Acquire) {
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
        let last = Instant::now().checked_sub(silent).unwrap_or_else(Instant::now);
        Helper {
            bridge: PathBuf::from("/nonexistent"),
            subnet: "192.168.1.0/24".parse().unwrap(),
            proxy: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1),
            relays: Arc::new(AtomicUsize::new(0)),
            forwards: Mutex::new(Vec::new()),
            heartbeat: Heartbeat {
                last: Arc::new(Mutex::new(last)),
                ended: Arc::new(AtomicBool::new(false)),
            },
            retired: AtomicBool::new(false),
        }
    }

    #[test]
    fn only_a_helper_whose_owner_went_silent_makes_way() {
        let live = helper(Duration::ZERO);
        assert!(matches!(live.answer(Request::Retire), Answer::Error(error) if error.contains("Another Horizon")));
        assert!(!live.retired.load(Ordering::Acquire));
        let lost = helper(RETIRE_AFTER + Duration::from_secs(1));
        if lost.heartbeat.silent_for() >= RETIRE_AFTER {
            assert_eq!(lost.answer(Request::Retire), Answer::Retired);
            assert!(lost.retired.load(Ordering::Acquire));
        }
    }
}
