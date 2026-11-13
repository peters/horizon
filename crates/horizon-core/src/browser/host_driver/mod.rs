//! The driver that runs beside the browser on a browser host.
//!
//! A browser panel can use the browser of another machine. A Chromium host is
//! reached through its `DevTools` port and one SSH forward, because CDP is a
//! socket protocol that a forward carries. Firefox is not: its driver is
//! `WebDriver` requests against a loopback port that refuses a foreign `Host`
//! port, and each picture is a screenshot, which costs one round trip. On a far
//! link that is 2 frames per second at best.
//!
//! So Firefox on a browser host runs a small driver of Horizon **on that
//! machine**, beside the browser, and that driver sends a compact stream: the
//! picture as a JPEG when the page changed, the state of the page, and the
//! results of the operations that an agent asked for. The protocol is
//! [`horizon_browser::host_driver`].
//!
//! The mode is one argument of this binary:
//!
//! ```text
//! ssh <target> <path>/horizon --browser-host-driver --profile <name> --engine firefox
//! ```
//!
//! The `ssh` child speaks the protocol on its standard input and output. It does
//! not own the session: the first link starts a small session process of the
//! same binary on the host (`HORIZON_BROWSER_HOST_SESSION=1`, started with
//! `nohup` so that it outlives the link), and every link relays its standard
//! input and output to the socket of that session. A link that ends leaves the
//! browser and its page running, and the next link attaches to the same page.
//! That is the promise that a Chromium host gives with its running browser, and
//! this is how Firefox keeps it.
//!
//! Everything of the session is on the host: the profile folder
//! `<home>/.horizon/browser-profiles/<profile>`, its downloads, its log and its
//! socket. This computer never reads or prepares one of them.

mod coordination;
mod paths;
mod session;
#[cfg(test)]
mod tests;

use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use horizon_browser::BackendKind;
use horizon_browser::host_driver::{
    HOST_DRIVER_REFUSED_EXIT, HostDriverLineError, HostDriverReply, MAX_HOST_DRIVER_LINE_BYTES,
    read_line as read_wire_line,
};

use crate::error::{Error, Result};

/// The environment of the session process of a browser host. The command line of
/// the mode is the same for the link and for the session, so that a person reads
/// one command in a log and the same one in this module.
pub const HOST_SESSION_ENV: &str = "HORIZON_BROWSER_HOST_SESSION";

/// How long a link waits for the session process that it started to accept it.
/// A cold Firefox of a browser host takes about 24 seconds to start, and the
/// session binds its socket before it starts the browser, so this bound covers
/// the start of the process and not the browser.
const SOCKET_READY: Duration = Duration::from_secs(20);

/// The poll of a link that waits for the session socket.
const SOCKET_POLL: Duration = Duration::from_millis(50);

/// The poll of the session socket that waits for a link, so the idle rule can
/// stop a browser that nobody attached to.
const ACCEPT_POLL: Duration = Duration::from_millis(100);

/// The engine that a driver of a browser host serves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostDriverEngine {
    Firefox,
}

impl HostDriverEngine {
    /// The name of the engine on a command line and on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Firefox => "firefox",
        }
    }

    /// The engine of this session, which the local driver of the host starts.
    #[must_use]
    pub const fn backend(self) -> BackendKind {
        match self {
            Self::Firefox => BackendKind::FirefoxBidi,
        }
    }

    /// The engine of a name, when this driver serves it. A Chromium host does
    /// not need this mode, so its name is refused with the reason.
    ///
    /// # Errors
    /// The name is not an engine of a browser host driver.
    pub fn parse(name: &str) -> Result<Self> {
        match name.trim() {
            "firefox" => Ok(Self::Firefox),
            "chromium" | "chromium-cdp" => Err(Error::Config(
                "a Chromium browser host needs no driver: this computer reaches its DevTools port through one SSH forward, so name no engine for it".into(),
            )),
            other => Err(Error::Config(format!(
                "`{other}` is not an engine of a browser host driver: this driver runs firefox"
            ))),
        }
    }
}

/// The settings of one driver of a browser host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostDriverOptions {
    profile: String,
    engine: HostDriverEngine,
}

/// The name of the profile that a browser host driver uses when no link names
/// one, as the browser host entry does.
pub const DEFAULT_HOST_DRIVER_PROFILE: &str = super::DEFAULT_HOST_PROFILE;

impl HostDriverOptions {
    /// # Errors
    /// The profile is not one profile name of a browser host.
    pub fn new(profile: impl Into<String>, engine: HostDriverEngine) -> Result<Self> {
        let profile = profile.into();
        if !horizon_browser::valid_profile_name(&profile) {
            return Err(Error::Config(format!(
                "`{profile}` is not one profile name of a browser host: 1 to 40 of a-z, 0-9 and '-', with a letter or a digit at each end"
            )));
        }
        Ok(Self { profile, engine })
    }

    /// Read the arguments of `--browser-host-driver`: `--profile <name>` and
    /// `--engine <engine>`, each at most once.
    ///
    /// # Errors
    /// An argument that this mode does not serve, a repeated flag, or a value
    /// that the rule of the profile and the engine refuses.
    pub fn parse(arguments: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Self> {
        let mut profile: Option<String> = None;
        let mut engine: Option<HostDriverEngine> = None;
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            let flag = argument.to_str().ok_or_else(|| {
                Error::Config(format!(
                    "`{}` is not part of the browser host driver mode",
                    argument.to_string_lossy()
                ))
            })?;
            match flag {
                "--profile" => {
                    let value = value_of(&mut arguments, flag)?;
                    if profile.replace(value).is_some() {
                        return Err(Error::Config("`--profile` is given once".into()));
                    }
                }
                "--engine" => {
                    let value = value_of(&mut arguments, flag)?;
                    if engine.replace(HostDriverEngine::parse(&value)?).is_some() {
                        return Err(Error::Config("`--engine` is given once".into()));
                    }
                }
                other => {
                    return Err(Error::Config(format!(
                        "`{other}` is not part of the browser host driver mode"
                    )));
                }
            }
        }
        Self::new(
            profile.unwrap_or_else(|| DEFAULT_HOST_DRIVER_PROFILE.to_string()),
            engine.unwrap_or(HostDriverEngine::Firefox),
        )
    }

    /// The profile of the browser on the host.
    #[must_use]
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// The engine that the driver runs on the host.
    #[must_use]
    pub const fn engine(&self) -> HostDriverEngine {
        self.engine
    }
}

fn value_of(arguments: &mut impl Iterator<Item = std::ffi::OsString>, flag: &str) -> Result<String> {
    let value = arguments
        .next()
        .ok_or_else(|| Error::Config(format!("`{flag}` needs one value")))?;
    value
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| Error::Config(format!("the value of `{flag}` is not text")))
}

/// Serve the mode: relay this link to the session of the browser host, starting
/// that session when this is the first link.
///
/// The answer is the exit code of the mode. Words of a failure go to the caller
/// on its standard error, which the local side reads as the reason of a failed
/// link.
///
/// The same program is the session of the host: `nohup` starts it with
/// [`HOST_SESSION_ENV`] set, and then it serves the socket of the profile
/// instead of the standard input of one `ssh` child.
#[must_use]
pub fn run(options: &HostDriverOptions) -> i32 {
    if std::env::var_os(HOST_SESSION_ENV).is_some() {
        return serve_session(options);
    }
    match attach_or_start(options) {
        Ok(stream) => relay(stream),
        Err(error) => {
            eprintln!("error: {error}");
            HOST_DRIVER_REFUSED_EXIT
        }
    }
}

/// Connect to the session of this profile, and start that session when no one
/// answers its socket.
fn attach_or_start(options: &HostDriverOptions) -> Result<std::os::unix::net::UnixStream> {
    let socket = paths::socket_path(options.profile(), options.engine().as_str())?;
    if let Some(stream) = connect(&socket) {
        return Ok(stream);
    }
    let started = start_session_process(options)?;
    let deadline = Instant::now() + SOCKET_READY;
    loop {
        if let Some(stream) = connect(&socket) {
            return Ok(stream);
        }
        if Instant::now() >= deadline {
            return Err(Error::Config(format!(
                "the browser host driver of this machine did not start within {} seconds: read {}",
                SOCKET_READY.as_secs(),
                started.display()
            )));
        }
        std::thread::sleep(SOCKET_POLL);
    }
}

fn connect(socket: &Path) -> Option<std::os::unix::net::UnixStream> {
    std::os::unix::net::UnixStream::connect(socket).ok()
}

/// Start the session process of the host, detached from this link with
/// `nohup`, and answer the file that keeps its words.
fn start_session_process(options: &HostDriverOptions) -> Result<std::path::PathBuf> {
    let profile_root = paths::profile_root(options.profile())?;
    paths::ensure_profile_root(&profile_root)?;
    let log = paths::log_path(options.profile(), options.engine().as_str())?;
    let program = std::env::current_exe()
        .map_err(|error| Error::Config(format!("this binary has no path to start again: {error}")))?;
    // The session keeps the browser of this machine after this link ends, so it
    // must not die with the `ssh` child that started it: `nohup` and `&` are the
    // two words of `sh` that a host of this mode always has.
    let script = format!(
        "{HOST_SESSION_ENV}=1 nohup {} --browser-host-driver --profile {} --engine {} >> {} 2>&1 &",
        quote(&program.to_string_lossy()),
        quote(options.profile()),
        quote(options.engine().as_str()),
        quote(&log.to_string_lossy()),
    );
    Command::new("sh")
        .args(["-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| {
            Error::Config(format!(
                "this machine could not start a session of its browser: {error}"
            ))
        })?;
    Ok(log)
}

/// One `sh` word that keeps the value of the person literal.
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Serve the session of this host: the browser, the page and the socket that
/// links attach to.
fn serve_session(options: &HostDriverOptions) -> i32 {
    let socket = match paths::socket_path(options.profile(), options.engine().as_str()) {
        Ok(socket) => socket,
        Err(error) => {
            eprintln!("error: {error}");
            return 2;
        }
    };
    if let Some(parent) = socket.parent()
        && let Err(error) = paths::ensure_profile_root(parent)
    {
        eprintln!("error: {error}");
        return 2;
    }
    if let Err(error) = remove_stale_socket(&socket) {
        eprintln!("error: {error}");
        return 2;
    }
    let listener = match std::os::unix::net::UnixListener::bind(&socket) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("error: could not bind {}: {error}", socket.display());
            return 2;
        }
    };
    if let Err(error) = listener.set_nonblocking(true) {
        eprintln!("error: the socket of this browser host cannot poll: {error}");
        return 2;
    }
    if let Err(error) = write_state(options) {
        eprintln!("error: {error}");
    }
    let mut session: Option<session::HostSession> = None;
    let mut link_ended = Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let mut link = Link::over_socket(&stream);
                let outcome = serve_one(&mut session, options, &mut link);
                drop(link);
                link_ended = Instant::now();
                if outcome == LinkOutcome::Close {
                    stop_and_cleanup(&mut session, &socket, options);
                    return 0;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => {
                eprintln!("error: the socket of this browser host failed: {error}");
                return 1;
            }
        }
        // A session that no link attached to for the idle bound stops its
        // browser: the person who closed the panel does not leave a Firefox of
        // this host running.
        if session.is_some() && link_ended.elapsed() >= session::LINK_IDLE {
            stop_and_cleanup(&mut session, &socket, options);
            return 0;
        }
        std::thread::sleep(ACCEPT_POLL);
    }
}

/// Stop the browser of this host and remove the files of its session.
fn stop_and_cleanup(session: &mut Option<session::HostSession>, socket: &Path, options: &HostDriverOptions) {
    if let Some(session) = session.as_mut() {
        session.stop();
    }
    cleanup(socket, options);
}

fn serve_one(session: &mut Option<session::HostSession>, options: &HostDriverOptions, link: &mut Link) -> LinkOutcome {
    if session.is_none() {
        match session::HostSession::start(options) {
            Ok(started) => *session = Some(started),
            Err(words) => {
                let _ = link.send(&HostDriverReply::Refusal {
                    code: "browser_start".to_string(),
                    message: words,
                    fatal: true,
                });
                let _ = link.flush();
                return LinkOutcome::Closed;
            }
        }
    }
    let Some(started) = session.as_mut() else {
        return LinkOutcome::Closed;
    };
    session::serve_link(started, link, options.profile(), options.engine())
}

fn remove_stale_socket(socket: &Path) -> Result<()> {
    match std::fs::remove_file(socket) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::Config(format!(
            "could not remove the socket of a previous browser host driver at {}: {error}",
            socket.display()
        ))),
    }
}

fn write_state(options: &HostDriverOptions) -> Result<()> {
    let state = paths::state_path(options.profile(), options.engine().as_str())?;
    let text = serde_json::json!({
        "pid": std::process::id(),
        "horizon_version": env!("CARGO_PKG_VERSION"),
        "profile": options.profile(),
        "engine": options.engine().as_str(),
        "system": horizon_browser::host_driver::HostSystem::here(),
        "socket": paths::socket_path(options.profile(), options.engine().as_str())?.to_string_lossy(),
    });
    std::fs::write(&state, text.to_string())
        .map_err(|error| Error::Config(format!("could not write {}: {error}", state.display())))
}

fn cleanup(socket: &Path, options: &HostDriverOptions) {
    let _ = std::fs::remove_file(socket);
    if let Ok(state) = paths::state_path(options.profile(), options.engine().as_str()) {
        let _ = std::fs::remove_file(state);
    }
}

/// What the link of a browser host did.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum LinkOutcome {
    /// The link goes on.
    Continue,
    /// The other end asked for the end of the session.
    Close,
    /// The other end is gone.
    Closed,
    /// The link ends with this answer.
    Answer(HostDriverReply),
    /// The link ends with these words.
    Refuse { code: String, message: String },
    /// The link answers these words and goes on: the page can accept the next
    /// request, so a person keeps the panel.
    Note { code: String, message: String },
}

impl LinkOutcome {
    /// The answer of a link whose line this wire cannot use.
    pub(super) fn refusal(error: HostDriverLineError) -> Self {
        match error {
            HostDriverLineError::TooLong { size, limit } => {
                Self::Answer(horizon_browser::host_driver::line_refusal(size, limit))
            }
            HostDriverLineError::Message { version, reason } => Self::Refuse {
                code: "protocol_message".to_string(),
                message: format!(
                    "the other end wrote a message that browser host protocol version {version} does not describe: {reason}"
                ),
            },
            HostDriverLineError::Ended => Self::Closed,
            HostDriverLineError::Read(error) | HostDriverLineError::Write(error) => Self::Refuse {
                code: "protocol_link".to_string(),
                message: format!("the link of this browser host failed: {error}"),
            },
        }
    }
}

/// What a pass of a link found on its input.
#[derive(Debug)]
pub(super) enum Next {
    Line(Vec<u8>),
    TooLong { size: usize, limit: usize },
    Ended,
    Nothing,
}

/// One step of a link: a request to apply, a silent pass, the end of the link,
/// or the answer that ends it.
#[derive(Debug)]
pub(super) enum Step {
    Request(horizon_browser::host_driver::HostDriverRequest),
    Silent,
    Ended,
    Refuse(LinkOutcome),
}

/// One link: the messages of this computer, and the answers of the driver.
pub(super) struct Link {
    incoming: mpsc::Receiver<Incoming>,
    writer: Box<dyn Write + Send>,
    close_reader: Option<Box<dyn Fn() + Send>>,
}

pub(super) enum Incoming {
    Line(Vec<u8>),
    TooLong { size: usize, limit: usize },
    Ended,
}

impl Link {
    /// A link over one socket of this host: a thread reads its lines so that a
    /// pass of the loop never waits on a silent link.
    pub(super) fn over_socket(stream: &std::os::unix::net::UnixStream) -> Self {
        let reader = match stream.try_clone() {
            Ok(reader) => reader,
            Err(error) => {
                tracing::warn!(%error, "a link of a browser host has no reader");
                let (sender, incoming) = mpsc::channel();
                let _ = sender.send(Incoming::Ended);
                return Self {
                    incoming,
                    writer: Box::new(std::io::sink()),
                    close_reader: None,
                };
            }
        };
        let (sender, incoming) = mpsc::channel();
        std::thread::Builder::new()
            .name("browser-host-link".to_string())
            .spawn(move || read_into(reader, &sender))
            .ok();
        let writer: Box<dyn Write + Send> = if let Ok(writer) = stream.try_clone() {
            Box::new(writer)
        } else {
            tracing::warn!("a link of a browser host has no writer");
            Box::new(std::io::sink())
        };
        let shutdown = stream.try_clone();
        Self {
            incoming,
            writer,
            close_reader: shutdown.ok().map(|stream| {
                Box::new(move || {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }) as Box<dyn Fn() + Send>
            }),
        }
    }

    /// One link of a test: the driver's end, and the peer that writes what this
    /// computer would write and reads what the driver answers.
    #[cfg(test)]
    pub(super) fn for_tests() -> (Self, tests::Peer) {
        let (requests, incoming) = mpsc::channel();
        let (writer, replies) = tests::CollectingWriter::new();
        (
            Self {
                incoming,
                writer: Box::new(writer),
                close_reader: None,
            },
            tests::Peer { requests, replies },
        )
    }

    /// The next message of this computer, or [`Next::Nothing`] when the link was
    /// silent for `wait`.
    pub(super) fn next(&mut self, wait: Duration) -> Next {
        match self.incoming.recv_timeout(wait) {
            Ok(Incoming::Line(line)) => Next::Line(line),
            Ok(Incoming::TooLong { size, limit }) => Next::TooLong { size, limit },
            Ok(Incoming::Ended) | Err(mpsc::RecvTimeoutError::Disconnected) => Next::Ended,
            Err(mpsc::RecvTimeoutError::Timeout) => Next::Nothing,
        }
    }

    /// Write one answer of the driver.
    pub(super) fn send(&mut self, reply: &HostDriverReply) -> std::io::Result<()> {
        match horizon_browser::host_driver::encode_line(reply) {
            Ok(line) => self.writer.write_all(&line),
            Err(error) => Err(std::io::Error::other(error.to_string())),
        }
    }

    pub(super) fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        if let Some(close) = self.close_reader.take() {
            close();
        }
    }
}

fn read_into(mut reader: impl Read, sender: &mpsc::Sender<Incoming>) {
    let mut reader = BufReader::new(&mut reader);
    loop {
        let next = match read_wire_line(&mut reader, MAX_HOST_DRIVER_LINE_BYTES) {
            Ok(Some(line)) => Incoming::Line(line),
            Ok(None) | Err(HostDriverLineError::Ended) => Incoming::Ended,
            Err(HostDriverLineError::TooLong { size, limit }) => Incoming::TooLong { size, limit },
            Err(HostDriverLineError::Message { .. } | HostDriverLineError::Read(_) | HostDriverLineError::Write(_)) => {
                Incoming::Ended
            }
        };
        let ended = matches!(next, Incoming::Ended | Incoming::TooLong { .. });
        if sender.send(next).is_err() || ended {
            return;
        }
    }
}

/// Relay the standard input and output of this link to the session of the host.
///
/// The relay does not read the protocol: it copies bytes. The words of a refusal
/// of the session are therefore the words that this computer reads.
fn relay(stream: std::os::unix::net::UnixStream) -> i32 {
    let reader = match stream.try_clone() {
        Ok(reader) => reader,
        Err(error) => {
            eprintln!("error: this link of a browser host cannot read its session: {error}");
            return HOST_DRIVER_REFUSED_EXIT;
        }
    };
    let to_stdout = std::thread::Builder::new()
        .name("browser-host-relay-out".to_string())
        .spawn(move || {
            let mut reader = BufReader::new(reader);
            let mut stdout = std::io::stdout().lock();
            let _ = std::io::copy(&mut reader, &mut stdout);
            let _ = stdout.flush();
        });
    let mut writer = stream;
    let mut stdin = std::io::stdin().lock();
    let copied = std::io::copy(&mut stdin, &mut writer);
    let _ = writer.shutdown(std::net::Shutdown::Write);
    match copied {
        Ok(_) => {
            if let Ok(handle) = to_stdout {
                let _ = handle.join();
            }
            0
        }
        Err(error) => {
            eprintln!("error: this link of a browser host could not read its input: {error}");
            HOST_DRIVER_REFUSED_EXIT
        }
    }
}
