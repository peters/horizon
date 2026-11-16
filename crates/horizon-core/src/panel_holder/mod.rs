//! The holder of a panel: a process of its own that owns one terminal and the
//! program that runs in it.
//!
//! A holder keeps a program alive when Horizon ends. It opens the terminal,
//! starts the program, and keeps the newest output in a ring. Horizon talks to it
//! over a private Unix socket of its home and attaches again after a restart. A
//! client that goes away detaches: the program keeps running and the ring keeps
//! filling.
//!
//! The launch document reaches the holder on its standard input, never in its
//! arguments, because the environment of an agent panel holds secrets and the
//! arguments of a process are visible to every process of the computer. The holder
//! starts its own session, so the end of Horizon does not send it a hangup.
//!
//! The bytes of the program, and any process of the same user, are untrusted
//! here. [`docs/review/trust-boundaries/panel-holder.md`] states the boundary, and
//! [`docs/architecture/panel-holder.md`] states the protocol and the life of a
//! holder. Nothing of this module is reachable for a person yet: no panel uses a
//! holder until the attach of a terminal lands.

mod launch;
mod serve;

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use horizon_pty_tap::frames;
use sha2::{Digest, Sha256};

use crate::util::spawn_stand_in;
use crate::{HorizonHome, HorizonHomeError};

pub use self::launch::Launch;
pub use horizon_pty_tap::frames::Hello;

/// The link to the program of the holder of one panel, whose hello and replay
/// this value took: a terminal attaches to `socket` and reads the output of the
/// program from there.
pub struct Linked {
    /// The socket of the holder, whose hello and replay this value took.
    pub socket: UnixStream,
    /// The pid and the start time of the program of the holder.
    pub hello: Hello,
    /// The newest output of the program from before this link.
    pub replay: Vec<u8>,
}

/// The argument that starts the mode of a holder.
pub const PANEL_HOLDER_ARG: &str = "--panel-holder";

/// The folder of the holders of a home, beside its sessions.
const HOLDER_DIR: &str = "panel-holders";
/// The mode of that folder: its owner alone reads it.
const DIR_MODE: u32 = 0o700;
/// The mode of one socket: its owner alone reaches it. Every process of that
/// owner can still connect, and the boundary page states that limit.
const SOCKET_MODE: u32 = 0o600;
/// The longest socket path that this build writes. macOS keeps 104 bytes with its
/// end, so this bound sits below the bound of every system.
const MAX_SOCKET_PATH: usize = 100;
/// The time that a start waits for the hello of its holder.
const START_BOUND: Duration = Duration::from_secs(5);
/// The time that a probe waits for the answer of a holder.
const READ_BOUND: Duration = Duration::from_secs(2);
/// The pause between two probes of a start.
const WAIT_PAUSE: Duration = Duration::from_millis(10);
/// The most sockets that one start reads, so that a home with many holders costs a
/// bounded time.
const MAX_SOCKETS: usize = 64;

/// Why a holder cannot serve, start or answer.
#[derive(Debug, thiserror::Error)]
pub enum HolderError {
    #[error("the holder of a panel: {0}")]
    Io(#[from] io::Error),
    #[error("the home of a holder: {0}")]
    Home(#[from] HorizonHomeError),
    #[error("the terminal of a holder: {0}")]
    Terminal(String),
    #[error("the launch document of a holder: {0}")]
    Document(#[from] serde_json::Error),
    #[error(
        "the launch document of a holder is above the bound of {} bytes",
        launch::MAX_DOCUMENT
    )]
    DocumentTooLong,
    #[error("the socket path {} is longer than {MAX_SOCKET_PATH} bytes", .0.display())]
    PathTooLong(PathBuf),
    #[error("the holder of this panel already serves {}", .0.display())]
    AlreadyServing(PathBuf),
    #[error("no holder answered at {} within {START_BOUND:?}", .0.display())]
    NoAnswer(PathBuf),
    #[error("the holder of this panel ended before it answered, with {0:?}")]
    StartExited(Option<i32>),
    #[error("the holder of this panel serves another client")]
    Busy,
    #[error(transparent)]
    Frame(#[from] horizon_pty_tap::frames::FrameError),
    #[error("the answer of a holder has the kind {0}, which is no hello")]
    NoHello(u8),
}

/// The private folder and the socket of the holder of one panel.
/// The file name of the socket of `local_id`: 32 hexadecimal characters of the
/// SHA-256 of the id. The escaped form of a real id is 73 characters, which
/// passes the bound of a socket path in every home.
pub(crate) fn socket_stem(local_id: &str) -> String {
    use std::fmt::Write as _;
    Sha256::digest(local_id.as_bytes())[..16]
        .iter()
        .fold(String::with_capacity(32), |mut stem, byte| {
            let _ = write!(stem, "{byte:02x}");
            stem
        })
}

#[derive(Clone, Debug)]
struct Paths {
    socket: PathBuf,
    folder: PathBuf,
}

impl Paths {
    /// The socket path of `local_id`, which holds no path part of the id.
    fn of(home: &HorizonHome, local_id: &str) -> Result<Self, HolderError> {
        let folder = home.root().join(HOLDER_DIR);
        let socket = folder.join(format!("{}.sock", socket_stem(local_id)));
        if socket.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(HolderError::PathTooLong(socket));
        }
        Ok(Self { socket, folder })
    }

    fn socket(&self) -> &Path {
        &self.socket
    }

    /// Makes the folder that holds the sockets of this home private.
    fn prepare(&self) -> Result<(), HolderError> {
        fs::create_dir_all(&self.folder)?;
        fs::set_permissions(&self.folder, fs::Permissions::from_mode(DIR_MODE))?;
        Ok(())
    }

    /// Binds the socket of this holder.
    ///
    /// A socket that answers belongs to a live holder, and this bind refuses
    /// instead of replacing it. A socket that no process listens on is what a
    /// holder that was killed left, so it is removed first.
    fn bind(&self) -> Result<UnixListener, HolderError> {
        if UnixStream::connect(&self.socket).is_ok() {
            return Err(HolderError::AlreadyServing(self.socket.clone()));
        }
        match fs::symlink_metadata(&self.socket) {
            Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(&self.socket)?,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let listener = UnixListener::bind(&self.socket)?;
        fs::set_permissions(&self.socket, fs::Permissions::from_mode(SOCKET_MODE))?;
        Ok(listener)
    }

    fn remove_socket(&self) -> Result<(), HolderError> {
        match fs::remove_file(&self.socket) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

/// Runs the mode of a holder: reads its launch document from the standard input,
/// makes the session of a holder, and serves the socket of its panel.
///
/// # Errors
///
/// Returns why the holder cannot read its launch or serve its panel.
pub fn serve() -> Result<(), HolderError> {
    let launch = Launch::read(&mut io::stdin().lock())?;
    // The holder leads its own session, so the end of the Horizon that started it
    // sends it no hangup of a terminal.
    if let Err(error) = rustix::process::setsid() {
        tracing::warn!(%error, "the holder of a panel could not start its own session");
    }
    let home = HorizonHome::prepare()?;
    serve::serve(&home, &launch, serve::RING_LIMIT)
}

/// Starts the holder of `launch` and answers its hello.
///
/// The holder leads its own session and outlives this Horizon, so a thread of
/// this Horizon waits for its end and the system reaps it. This starts a process
/// and waits for a socket, so only a worker calls it.
///
/// # Errors
///
/// Returns why the holder cannot start or does not answer.
pub fn start(home: &HorizonHome, launch: &Launch) -> Result<Hello, HolderError> {
    let exe = std::env::current_exe()?;
    start_with(&exe, home, launch)
}

/// Starts `exe` in the mode of a holder. A test names a stand-in program.
fn start_with(exe: &Path, home: &HorizonHome, launch: &Launch) -> Result<Hello, HolderError> {
    let mut child = spawn_stand_in(|| holder_command(exe).spawn())?;
    let answer = start_document(&mut child, home, launch);
    if answer.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return answer;
    }
    thread::Builder::new()
        .name("panel-holder-reap".to_owned())
        .spawn(move || {
            let _ = child.wait();
        })?;
    answer
}

fn start_document(child: &mut Child, home: &HorizonHome, launch: &Launch) -> Result<Hello, HolderError> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("the holder of a panel has no standard input"))?;
    stdin.write_all(&launch.document()?)?;
    drop(stdin);
    wait_for_holder(child, home, &launch.local_id)
}

/// The command that runs the holder of a panel: the same binary in the mode of a
/// holder, with no argument that carries the launch.
fn holder_command(exe: &Path) -> Command {
    let mut command = Command::new(exe);
    command
        .arg(PANEL_HOLDER_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Waits until the holder of `local_id` answers, and answers its hello.
fn wait_for_holder(child: &mut Child, home: &HorizonHome, local_id: &str) -> Result<Hello, HolderError> {
    let socket = Paths::of(home, local_id)?.socket().to_path_buf();
    let deadline = Instant::now() + START_BOUND;
    loop {
        match probe(home, local_id)? {
            Some(hello) => return Ok(hello),
            None if Instant::now() >= deadline => return Err(HolderError::NoAnswer(socket)),
            None => {}
        }
        if let Some(status) = child.try_wait()? {
            return Err(HolderError::StartExited(status.code()));
        }
        std::thread::sleep(WAIT_PAUSE);
    }
}

/// Answers the hello of the live holder of `local_id`, or `None` when no holder
/// listens on its socket.
///
/// A holder that serves another client answers a refusal, which is
/// [`HolderError::Busy`]. This connects to a socket and waits for its answer, so
/// only a worker calls it.
///
/// # Errors
///
/// Returns why the socket cannot be reached, or that the answer is no hello.
pub fn probe(home: &HorizonHome, local_id: &str) -> Result<Option<Hello>, HolderError> {
    let paths = Paths::of(home, local_id)?;
    let Ok(mut stream) = UnixStream::connect(paths.socket()) else {
        return Ok(None);
    };
    stream.set_read_timeout(Some(READ_BOUND))?;
    match frames::read_one(&mut stream)? {
        frames::Frame::Hello(hello) => Ok(Some(hello)),
        frames::Frame::Refusal => Err(HolderError::Busy),
        frame => Err(HolderError::NoHello(frame.kind())),
    }
}

/// Links to the holder of `local_id` and answers its hello and its replay, or
/// `None` when no holder listens on its socket.
///
/// A holder that still serves the client of a probe answers a refusal, so this
/// tries again until its read bound passes. This connects to a socket, so only a
/// worker calls it.
///
/// # Errors
///
/// Returns why the socket cannot be reached, or why the holder answers no hello.
pub fn linked(home: &HorizonHome, local_id: &str) -> Result<Option<Linked>, HolderError> {
    let deadline = Instant::now() + READ_BOUND;
    loop {
        let mut socket = match UnixStream::connect(Paths::of(home, local_id)?.socket()) {
            Ok(socket) => socket,
            Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        match crate::terminal::holder_hello(&mut socket) {
            Ok((hello, replay)) => return Ok(Some(Linked { socket, hello, replay })),
            // The client of a probe, or a holder that is going, holds the link for
            // one more turn of its loop.
            Err(_) if Instant::now() < deadline => thread::sleep(WAIT_PAUSE),
            Err(error) => return Err(HolderError::Terminal(error.to_string())),
        }
    }
}

/// Ends the program of every holder of `home` whose panel is not in `shown`, and
/// answers how many holders it ended. A socket that nobody listens on is removed,
/// and a home with more sockets than this build reads keeps the rest for the next
/// start.
///
/// This connects to a socket and waits for its answer, so only a worker calls it.
///
/// # Errors
///
/// Returns why the folder of the holders or a socket cannot be read.
pub fn close_unshown(home: &HorizonHome, shown: &BTreeSet<String>) -> Result<usize, HolderError> {
    let folder = home.root().join(HOLDER_DIR);
    let entries = match fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let mut ended = 0;
    for entry in entries.take(MAX_SOCKETS) {
        let path = entry?.path();
        let Some(stem) = path.file_stem().and_then(std::ffi::OsStr::to_str) else {
            continue;
        };
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("sock")
            || shown.iter().any(|local_id| socket_stem(local_id) == stem)
        {
            continue;
        }
        ended += usize::from(end_holder(&path)?);
    }
    Ok(ended)
}

/// Ends the holder of `path` and answers whether one ran there, which a socket that
/// nobody listens on is not.
fn end_holder(path: &Path) -> Result<bool, HolderError> {
    let deadline = Instant::now() + READ_BOUND;
    loop {
        let Ok(mut stream) = UnixStream::connect(path) else {
            let _ = fs::remove_file(path);
            return Ok(false);
        };
        stream.set_read_timeout(Some(READ_BOUND))?;
        match frames::read_one(&mut stream) {
            Ok(frames::Frame::Hello(_)) => {
                frames::write(&mut stream, &frames::Frame::Close)?;
                return Ok(true);
            }
            // The client of a probe holds the link for one more turn of the loop.
            Ok(_) if Instant::now() < deadline => thread::sleep(WAIT_PAUSE),
            Ok(_) => return Ok(false),
            Err(_) => {
                let _ = fs::remove_file(path);
                return Ok(false);
            }
        }
    }
}

/// Serves `launch` on a thread of a test, so that a test starts no process.
#[cfg(test)]
pub(crate) fn serve_on_a_thread(home: &HorizonHome, launch: &Launch) -> Result<Hello, HolderError> {
    let owned = home.clone();
    let served = launch.clone();
    thread::Builder::new()
        .name("panel-holder-of-a-test".to_owned())
        .spawn(move || {
            let _ = serve::serve(&owned, &served, serve::RING_LIMIT);
        })?;
    let deadline = Instant::now() + START_BOUND;
    loop {
        match probe(home, &launch.local_id)? {
            Some(hello) => return Ok(hello),
            None if Instant::now() >= deadline => {
                return Err(HolderError::NoAnswer(
                    Paths::of(home, &launch.local_id)?.socket().to_path_buf(),
                ));
            }
            None => thread::sleep(WAIT_PAUSE),
        }
    }
}

#[cfg(test)]
mod tests;
