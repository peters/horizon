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
pub use client::HostDriverHost;
#[cfg(test)]
mod client_tests;
mod coordination;
mod latency;
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
        };
        let start = move |_options: &HostDriverOptions| {
            Ok(session::HostSession::over_page(
                Box::new(session::IdlePage),
                std::sync::Arc::clone(&slot),
            ))
        };
        serve_links(&listener, &options, &socket, rules, start)
    })
}

/// The settings of one driver of a browser host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostDriverOptions {
    profile: String,
    engine: HostDriverEngine,
    idle_stop: Option<Duration>,
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
        Ok(Self {
            profile,
            engine,
            idle_stop: None,
        })
    }

    /// Stop the browser of this host after `minutes` with no link attached.
    ///
    /// The session keeps its browser and its page until the person closes the
    /// panel, until the person removes the browser or until the machine of the
    /// host restarts, so this bound is off unless the entry of the host asks for
    /// it.
    ///
    /// # Errors
    /// The bound is not 1 to [`MAX_IDLE_STOP_MINUTES`] minutes.
    pub fn with_idle_stop_minutes(mut self, minutes: u32) -> Result<Self> {
        if minutes == 0 || minutes > MAX_IDLE_STOP_MINUTES {
            return Err(Error::Config(format!(
                "`{minutes}` is not a bound for an idle browser host: `--idle-stop` takes 1 to {MAX_IDLE_STOP_MINUTES} minutes, and a session without the option keeps its browser"
            )));
        }
        self.idle_stop = Some(Duration::from_secs(u64::from(minutes) * 60));
        Ok(self)
    }

    /// Read the arguments of `--browser-host-driver`: `--profile <name>`,
    /// `--engine <engine>` and `--idle-stop <minutes>`, each at most once.
    ///
    /// # Errors
    /// An argument that this mode does not serve, a repeated flag, or a value
    /// that the rule of the profile, the engine and the idle bound refuses.
    pub fn parse(arguments: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Self> {
        let mut profile: Option<String> = None;
        let mut engine: Option<HostDriverEngine> = None;
        let mut idle_stop: Option<u32> = None;
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
                "--idle-stop" => {
                    let value = value_of(&mut arguments, flag)?;
                    let minutes = value.trim().parse::<u32>().map_err(|_| {
                        Error::Config(format!(
                            "`{value}` is not a number of minutes for the idle bound of a browser host"
                        ))
                    })?;
                    if idle_stop.replace(minutes).is_some() {
                        return Err(Error::Config("`--idle-stop` is given once".into()));
                    }
                }
                other => {
                    return Err(Error::Config(format!(
                        "`{other}` is not part of the browser host driver mode"
                    )));
                }
            }
        }
        let options = Self::new(
            profile.unwrap_or_else(|| DEFAULT_HOST_DRIVER_PROFILE.to_string()),
            engine.unwrap_or(HostDriverEngine::Firefox),
        )?;
        match idle_stop {
            Some(minutes) => options.with_idle_stop_minutes(minutes),
            None => Ok(options),
        }
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

    /// The bound that stops the browser of this host with no link attached, and
    /// `None` when the entry of the host asks for none.
    #[must_use]
    pub fn idle_stop(&self) -> Option<Duration> {
        self.idle_stop
    }

    /// The arguments that start this session again, so a person reads one
    /// command of a host in a log.
    fn session_arguments(&self) -> String {
        let profile = quote(self.profile());
        let engine = quote(self.engine().as_str());
        match self.idle_stop {
            Some(idle_stop) => format!(
                "--browser-host-driver --profile {profile} --engine {engine} --idle-stop {}",
                idle_stop.as_secs() / 60
            ),
            None => format!("--browser-host-driver --profile {profile} --engine {engine}"),
        }
    }
}

/// The longest idle bound that a host entry can name, in minutes.
pub const MAX_IDLE_STOP_MINUTES: u32 = 10_080;

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
    let start = |options: &HostDriverOptions| session::HostSession::start(options);
    let rules = LinkRules {
        take_over: LINK_TAKE_OVER,
        silent: LINK_SILENT,
        idle_stop: options.idle_stop(),
    };
    serve_links(&listener, options, &socket, rules, start)
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
}

/// Serve the links of one session, in the order that this computer asks for
/// them.
///
/// The session outlives every link: a link that ends leaves the browser and the
/// page running, and the next link attaches to the same page. A new link takes
/// the page over from a link that went silent, which is what a dropped link
/// looks like from here, and a link whose other end vanished is closed after
/// [`LINK_SILENT`], so its process ends instead of waiting on the socket.
fn serve_links<F>(
    listener: &std::os::unix::net::UnixListener,
    options: &HostDriverOptions,
    socket: &Path,
    rules: LinkRules,
    start: F,
) -> i32
where
    F: Fn(&HostDriverOptions) -> std::result::Result<session::HostSession, String>,
{
    let mut session: Option<session::HostSession> = None;
    let mut next_link: Option<std::os::unix::net::UnixStream> = None;
    let mut idle_since: Option<Instant> = None;
    loop {
        let Some(stream) = next_link.take().or_else(|| waiting_link(listener)) else {
            if let Some(idle_stop) = rules.idle_stop
                && idle_since.is_some_and(|since| since.elapsed() >= idle_stop)
            {
                tracing::info!(
                    profile = options.profile(),
                    "the session of a browser host kept its browser with no link for the idle bound, so it stops"
                );
                stop_and_cleanup(&mut session, socket, options);
                return 0;
            }
            std::thread::sleep(ACCEPT_POLL);
            continue;
        };
        idle_since = None;
        let mut link = Link::over_socket(&stream);
        let end = serve_one(&mut session, options, &mut link, listener, rules, &start);
        drop(link);
        if let Some(next) = end.next {
            next_link = Some(next);
            continue;
        }
        if end.outcome == LinkOutcome::Close {
            stop_and_cleanup(&mut session, socket, options);
            return 0;
        }
        idle_since = Some(Instant::now());
    }
}

/// The connection that waits on the socket of the session, when one arrived, and
/// otherwise nothing.
fn waiting_link(listener: &std::os::unix::net::UnixListener) -> Option<std::os::unix::net::UnixStream> {
    match listener.accept() {
        Ok((stream, _)) => Some(stream),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => None,
        Err(error) => {
            tracing::warn!(%error, "the socket of a browser host session could not accept a link");
            None
        }
    }
}

/// Stop the browser of this host and remove the files of its session.
fn stop_and_cleanup(session: &mut Option<session::HostSession>, socket: &Path, options: &HostDriverOptions) {
    if let Some(session) = session.as_mut() {
        session.stop();
    }
    cleanup(socket, options);
}

fn serve_one<F>(
    session: &mut Option<session::HostSession>,
    options: &HostDriverOptions,
    link: &mut Link,
    listener: &std::os::unix::net::UnixListener,
    rules: LinkRules,
    start: &F,
) -> LinkEnd
where
    F: Fn(&HostDriverOptions) -> std::result::Result<session::HostSession, String>,
{
    if session.is_none() {
        match start(options) {
            Ok(started) => *session = Some(started),
            Err(words) => {
                let _ = link.send(&HostDriverReply::Refusal {
                    code: "browser_start".to_string(),
                    message: words,
                    fatal: true,
                });
                let _ = link.flush();
                return LinkEnd::of(LinkOutcome::Closed);
            }
        }
    }
    let Some(started) = session.as_mut() else {
        return LinkEnd::of(LinkOutcome::Closed);
    };
    session::serve_link(
        started,
        link,
        options.profile(),
        options.engine(),
        Some(&session::NewLinks {
            listener,
            take_over: rules.take_over,
            silent: rules.silent,
            profile: options.profile(),
        }),
    )
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
    /// The connection that takes the page over, when one arrived while this link
    /// served it.
    pub(super) next: Option<std::os::unix::net::UnixStream>,
}

impl LinkEnd {
    pub(super) fn of(outcome: LinkOutcome) -> Self {
        Self { outcome, next: None }
    }

    pub(super) fn taken_over(stream: std::os::unix::net::UnixStream) -> Self {
        Self {
            outcome: LinkOutcome::TakeOver,
            next: Some(stream),
        }
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
