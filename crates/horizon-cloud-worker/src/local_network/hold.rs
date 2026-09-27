//! The helper at the worker end of one bridge session. It lives exactly as long as the owner's
//! SSH session keeps writing heartbeats, and takes its sockets and forwards with it.
use super::{Answer, Forward, MAX_MESSAGE, Paths, Request, Status, forward};
use horizon_cloud_protocol::local_network::{HEARTBEAT_TIMEOUT, Nonce, Ready, Subnet};
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddrV4},
    os::unix::{
        fs::MetadataExt,
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
const NOTE: &str = "TCP only. Names are resolved on the owner's computer. Every process on this worker can use the proxy and forwards while the bridge is on. Forwards end when the bridge stops or reconnects; check the status and forward again.";

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
    /// A newer session takes the control socket over; the older helper keeps serving its own
    /// forwards until its heartbeat stops.
    fn bind(paths: &Paths) -> io::Result<Self> {
        let path = paths.control();
        let lock = paths.lock();
        let _guard = Locked::new(&lock)?;
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
        let file = File::options().create(true).truncate(false).write(true).open(path)?;
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
    super::prepare(paths)?;
    let bridge = Bridge(paths.bridge(&nonce));
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
    });
    let control = Control::bind(paths)?;
    let heartbeat = Heartbeat::watch(input)?;
    serde_json::to_writer(&mut output, &Ready { proxy: helper.proxy })?;
    output.write_all(b"\n")?;
    output.flush()?;
    serve(&helper, &control, &heartbeat);
    helper.forwards().clear();
    drop(endpoint);
    Ok(())
}

fn wait_for_socket(path: &Path) -> io::Result<()> {
    let deadline = Instant::now() + SOCKET_WAIT;
    while !path.exists() {
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

    fn alive(&self) -> bool {
        !self.ended.load(Ordering::Acquire)
            && self.last.lock().unwrap_or_else(PoisonError::into_inner).elapsed() < HEARTBEAT_TIMEOUT
    }
}

fn serve(helper: &Arc<Helper>, control: &Control, heartbeat: &Heartbeat) {
    let requests = Arc::new(AtomicUsize::new(0));
    while heartbeat.alive() {
        match control.listener.accept() {
            Ok((stream, _)) => {
                if requests
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                        (count < MAX_REQUESTS).then_some(count + 1)
                    })
                    .is_err()
                {
                    continue;
                }
                let spawned = {
                    let (helper, requests) = (Arc::clone(helper), Arc::clone(&requests));
                    thread::Builder::new()
                        .name("local-network-control".into())
                        .spawn(move || {
                            let _ = handle(&helper, stream);
                            requests.fetch_sub(1, Ordering::AcqRel);
                        })
                };
                if spawned.is_err() {
                    requests.fetch_sub(1, Ordering::AcqRel);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
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
