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

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use horizon_pty_tap::frames;

use crate::horizon_home::safe_local_id;
use crate::util::spawn_stand_in;
use crate::{HorizonHome, HorizonHomeError};

pub use self::launch::Launch;
pub use horizon_pty_tap::frames::Hello;

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
#[derive(Clone, Debug)]
struct Paths {
    socket: PathBuf,
    folder: PathBuf,
}

impl Paths {
    /// The socket path of `local_id`. The id is encoded, so it can hold no path
    /// part of its own.
    fn of(home: &HorizonHome, local_id: &str) -> Result<Self, HolderError> {
        let folder = home.root().join(HOLDER_DIR);
        let socket = folder.join(format!("{}.sock", safe_local_id(local_id)));
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

#[cfg(test)]
mod tests;
