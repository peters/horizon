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
//! browser and its page running, and the next link of that panel attaches to the
//! page that it had. That is the promise that a Chromium host gives with its
//! running browser, and this is how Firefox keeps it.
//!
//! One session serves every panel of its profile: each panel is its own page of
//! the one Firefox, so two panels of a profile are signed in together, and the
//! pages are apart from each other's text and scroll place. [`pages`] keeps them
//! and the bound of them.
//!
//! A session lives until the person closes the panel, until the person removes
//! the browser of that profile, or until the machine of the host restarts. An
//! idle session keeps the browser of that profile with its page: the Firefox
//! process of the profile, its content processes and its `geckodriver`, the page
//! with its typed text, its scroll place and its cookies, and the profile folder
//! on the disk. That is the memory of one Firefox window, about 400 MB of memory
//! of the host for one page. An entry of a host can stop that browser after a
//! bound with no link, `--idle-stop <minutes>` on the command of the entry, and a
//! session without that option never stops by itself.
//!
//! Everything of the session is on the host: the profile folder
//! `<home>/.horizon/browser-profiles/<profile>`, its downloads, its log and its
//! socket. This computer never reads or prepares one of them.

pub(crate) mod client;
pub use client::{HostDriverHost, LinkPage};
#[cfg(test)]
mod client_tests;
mod coordination;
mod latency;
mod options;
mod pages;
pub(crate) mod panel;
#[cfg(test)]
mod panel_tests;
mod paths;
mod session;
#[cfg(test)]
mod tests;

use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

pub use options::{HostDriverOptions, MAX_IDLE_STOP_MINUTES, MAX_UNLINKED_PRIVATE_PAGE_SECONDS};

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

/// The poll of the session socket that waits for a link.
const ACCEPT_POLL: Duration = Duration::from_millis(100);

/// The longest silence of a link that the session keeps. A link of a panel says
/// something four times a second (the signals of its manifest), so a link that
/// says nothing for this long is a link whose other end is gone: the session
/// closes it, and the process of that link ends with the socket. A link whose
/// other end vanished keeps its socket open, because the other side of `ssh`
/// holds it, so this silence is not a rare case.
pub(super) const LINK_SILENT: Duration = Duration::from_secs(60);

/// The silence of a link that a new link of the same profile takes the page
/// over from. A panel that lost its link attaches again after a few seconds, and
/// the link that it left behind answered nothing since, so the page comes back
/// instead of waiting for that link for a minute.
pub(super) const LINK_TAKE_OVER: Duration = Duration::from_secs(1);

/// The most links that one session of a browser host serves at one time.
///
/// A link is a thread of the session, and a session serves one page for each
/// panel of its profile, so the links that a session carries are the pages of
/// [`pages::MAX_PAGES`] plus the links of a panel that attaches again while its
/// own link is still there. A connection beyond this bound reads why it waits
/// instead of growing the session without end.
const MAX_LINK_THREADS: usize = pages::MAX_PAGES + 4;

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

/// The socket of the session of `profile` under `home`, for a test that serves
/// the session of a browser host where a host would.
///
/// The panel of a browser host is tested through a real child of this binary,
/// and that child reads the socket of its session from the home of its process.
#[doc(hidden)]
#[must_use]
pub fn session_socket_in(home: &Path, profile: &str, engine: HostDriverEngine) -> Option<std::path::PathBuf> {
    paths::socket_path_in(home, profile, engine.as_str()).ok()
}

/// Serve the session of a browser host over `listener` on a page of a test.
///
/// A test of the panel uses this beside the real child of its binary: the child
/// is the link of a host, this thread is the session and its page, and no
/// browser of this machine takes part.
#[doc(hidden)]
#[must_use]
pub fn serve_test_session(
    listener: std::os::unix::net::UnixListener,
    socket: std::path::PathBuf,
    profile: String,
    slot: std::sync::Arc<horizon_browser::FrameSlot>,
) -> std::thread::JoinHandle<i32> {
    std::thread::spawn(move || {
        let Ok(options) = HostDriverOptions::new(profile, HostDriverEngine::Firefox) else {
            return 2;
        };
        let _ = listener.set_nonblocking(true);
        let rules = LinkRules {
            take_over: LINK_TAKE_OVER,
            silent: LINK_SILENT,
            idle_stop: None,
            unlinked_private_page: pages::PRIVATE_PAGE_UNLINKED,
        };
        let start = pages::StartPage::new(move |_options, _group, _private| {
            Ok(session::HostSession::over_page(
                Box::new(session::IdlePage),
                std::sync::Arc::clone(&slot),
            ))
        });
        serve_links(&listener, &options, &socket, rules, start)
    })
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
    // two words of `sh` that a host of this mode always has. The session takes
    // the arguments of this link, so the bound of an idle session is the one
    // that the entry of the host names.
    let script = format!(
        "{HOST_SESSION_ENV}=1 nohup {} {} >> {} 2>&1 &",
        quote(&program.to_string_lossy()),
        options.session_arguments(),
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
pub(super) fn quote(value: &str) -> String {
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
    let rules = LinkRules {
        take_over: LINK_TAKE_OVER,
        silent: LINK_SILENT,
        idle_stop: options.idle_stop(),
        unlinked_private_page: options.unlinked_private_page(),
    };
    serve_links(&listener, options, &socket, rules, pages::StartPage::of_host())
}

/// The bounds of one session of a browser host: when a new link takes the page
/// over, when a link is closed for its silence, and when a browser with no link
/// is stopped.
#[derive(Clone, Copy, Debug)]
pub(super) struct LinkRules {
    /// The silence of a link that a new connection takes the page over from.
    pub(super) take_over: Duration,
    /// The silence after which the session closes a link.
    pub(super) silent: Duration,
    /// The bound that stops the browser of a session with no link, when the
    /// entry of the host names one.
    pub(super) idle_stop: Option<Duration>,
    /// The bound of a private page with no link, which the session closes.
    pub(super) unlinked_private_page: Duration,
}

/// Serve the links of one session, in the order that this computer asks for
/// them.
///
/// The session outlives every link: a link that ends leaves the browser and its
/// page running, and the next link of that panel attaches to the page that it
/// had. Each link has a thread of its own, because one link drives one page and
/// two panels of one profile are two pages of one browser. A link takes its page
/// over from a link that went silent, which is what a dropped link looks like
/// from here, and a link whose other end vanished is closed after
/// [`LINK_SILENT`], so its process ends instead of waiting on the socket.
fn serve_links(
    listener: &std::os::unix::net::UnixListener,
    options: &HostDriverOptions,
    socket: &Path,
    rules: LinkRules,
    start: pages::StartPage,
) -> i32 {
    let served = Arc::new(pages::SessionPages::new(options.clone(), rules, start));
    let links = Arc::new(AtomicUsize::new(0));
    let (ended, events) = mpsc::channel();
    // The bound of the idle stop counts from the moment that this session has no
    // link, which is the moment that the last page of it let go.
    let mut idle_since: Option<Instant> = None;
    loop {
        if let Some(stream) = waiting_link(listener) {
            if links.load(Ordering::Acquire) >= MAX_LINK_THREADS {
                refuse_extra_link(stream);
                continue;
            }
            links.fetch_add(1, Ordering::AcqRel);
            spawn_link(stream, Arc::clone(&served), Arc::clone(&links), ended.clone());
        }
        let last_page_closed = match events.recv_timeout(ACCEPT_POLL) {
            Ok(SessionEvent::PageClosed { last_page }) => last_page,
            // A link ended, no link said anything, or every thread is gone, so
            // nothing of this session can be reached.
            Ok(SessionEvent::LinkEnded)
            | Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => false,
        };
        if last_page_closed {
            tracing::info!(
                profile = options.profile(),
                "the last page of a browser host session ended, so the session stops its browser"
            );
            served.stop_all();
            cleanup(socket, options);
            return 0;
        }
        if served.close_unlinked_private_pages(Instant::now()) {
            tracing::info!(
                profile = options.profile(),
                "the session of a browser host closed the last page that it had, so it stops its browser"
            );
            served.stop_all();
            cleanup(socket, options);
            return 0;
        }
        if let Some(idle_stop) = rules.idle_stop {
            if links.load(Ordering::Acquire) > 0 {
                idle_since = None;
            } else if idle_since.get_or_insert_with(Instant::now).elapsed() >= idle_stop {
                tracing::info!(
                    profile = options.profile(),
                    "the session of a browser host kept its browser with no link for the idle bound, so it stops"
                );
                served.stop_all();
                cleanup(socket, options);
                return 0;
            }
        }
    }
}

/// What a link of a session reports to the loop that serves the session.
enum SessionEvent {
    /// A link ended, and its page waits for the next link.
    LinkEnded,
    /// A link closed its page, and `last_page` says whether that was the last
    /// page of the session.
    PageClosed { last_page: bool },
}

/// Serve one connection of the socket of a session, on a thread of its own.
fn spawn_link(
    stream: std::os::unix::net::UnixStream,
    served: Arc<pages::SessionPages>,
    links: Arc<AtomicUsize>,
    events: mpsc::Sender<SessionEvent>,
) {
    let counted = Arc::clone(&links);
    let spawned = std::thread::Builder::new()
        .name("browser-host-link".to_string())
        .spawn(move || {
            let mut link = Link::over_socket(&stream);
            let served_page = served.serve_link(&mut link);
            drop(link);
            let event = if served_page.last_page {
                SessionEvent::PageClosed { last_page: true }
            } else {
                SessionEvent::LinkEnded
            };
            let _ = events.send(event);
            links.fetch_sub(1, Ordering::AcqRel);
        });
    if spawned.is_err() {
        counted.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Tell a connection that this session has no link left for it, and close it.
///
/// The greeting of the connection is read first, in a thread of its own: a link
/// that this session closes with its greeting still unread answers the write of
/// the greeting with a reset, and this computer would then read no words at all.
fn refuse_extra_link(stream: std::os::unix::net::UnixStream) {
    refuse_link(
        stream,
        "link_limit",
        &format!(
            "the session of this browser host serves {MAX_LINK_THREADS} links, and all of them are in use: close a panel of this profile, then use Retry"
        ),
    );
}

fn refuse_link(stream: std::os::unix::net::UnixStream, code: &'static str, message: &str) {
    let reply = HostDriverReply::Refusal {
        code: code.to_string(),
        message: message.to_string(),
        fatal: true,
    };
    std::thread::Builder::new()
        .name("browser-host-refusal".to_string())
        .spawn(move || {
            let _ = stream.set_read_timeout(Some(session::GREET_WAIT));
            if let Ok(reader) = stream.try_clone() {
                let mut reader = BufReader::new(reader);
                let _ = read_wire_line(&mut reader, MAX_HOST_DRIVER_LINE_BYTES);
            }
            if let Ok(line) = horizon_browser::host_driver::encode_line(&reply) {
                let mut writer = &stream;
                let _ = writer.write_all(&line);
                let _ = writer.flush();
            }
            let _ = stream.shutdown(std::net::Shutdown::Both);
        })
        .ok();
}

/// The connection that waits on the socket of the session, when one arrived, and
/// otherwise nothing.
fn waiting_link(listener: &std::os::unix::net::UnixListener) -> Option<std::os::unix::net::UnixStream> {
    match listener.accept() {
        // A socket that `accept` gives takes the mode of its listener on this
        // system, and the listener polls, so the link is put back into the
        // blocking mode that its two ends read and write with.
        Ok((stream, _)) => {
            let _ = stream.set_nonblocking(false);
            Some(stream)
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => None,
        Err(error) => {
            tracing::warn!(%error, "the socket of a browser host session could not accept a link");
            None
        }
    }
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
    /// A new link of the same profile takes the page over from this one.
    TakeOver,
    /// This link said nothing for longer than the session keeps it.
    Silent,
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

/// How one link of a browser host ended.
pub(super) struct LinkEnd {
    pub(super) outcome: LinkOutcome,
}

impl LinkEnd {
    pub(super) const fn of(outcome: LinkOutcome) -> Self {
        Self { outcome }
    }
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
///
/// This link lives as long as its two ends copy. It ends when the session closes
/// its socket, which is what a session does when the person closes the panel,
/// when this link went silent, or when a new link took the page over. It does not
/// wait for its own input to end first: the other side of `ssh` keeps the input
/// of this process open after the person's computer is gone, so an input that
/// never ends is not the end of a link.
fn relay(stream: std::os::unix::net::UnixStream) -> i32 {
    relay_streams(std::io::stdin(), std::io::stdout(), stream)
}

/// [`relay`] over the two streams of a link, so a test drives a link whose
/// input never ends.
fn relay_streams(
    input: impl Read + Send + 'static,
    output: impl Write + Send + 'static,
    session: std::os::unix::net::UnixStream,
) -> i32 {
    let reader = match session.try_clone() {
        Ok(reader) => reader,
        Err(error) => {
            eprintln!("error: this link of a browser host cannot read its session: {error}");
            return HOST_DRIVER_REFUSED_EXIT;
        }
    };
    let (ended, ended_rx) = mpsc::channel();
    let to_caller = std::thread::Builder::new()
        .name("browser-host-relay-out".to_string())
        .spawn(move || {
            let mut reader = BufReader::new(reader);
            let mut output = output;
            let _ = std::io::copy(&mut reader, &mut output);
            let _ = output.flush();
            let _ = ended.send(());
        });
    if to_caller.is_err() {
        return HOST_DRIVER_REFUSED_EXIT;
    }
    let mut writer = session;
    let to_session = std::thread::Builder::new()
        .name("browser-host-relay-in".to_string())
        .spawn(move || {
            let mut input = input;
            let _ = std::io::copy(&mut input, &mut writer);
            let _ = writer.shutdown(std::net::Shutdown::Write);
        });
    if to_session.is_err() {
        return HOST_DRIVER_REFUSED_EXIT;
    }
    // The session ends this link, and its end is the end of this process. The
    // thread that copies the input of `ssh` ends with this process, so a link
    // that waits on an input that never closes leaves nothing behind.
    match ended_rx.recv() {
        Ok(()) => 0,
        Err(_) => HOST_DRIVER_REFUSED_EXIT,
    }
}
