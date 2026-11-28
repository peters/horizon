//! The supervisor of the link of one computer: one transport, the protocol of the
//! store over it, and the state that the sidebar reads.
//!
//! The supervisor runs on a worker thread and never on the UI thread. It starts
//! `ssh <target> <command>` with the bounded flags of the browser forward, or it
//! opens one socket to the TCP listener of the server when the settings name an
//! address and this computer holds the token of a join. It speaks the frames of
//! [`crate::client`] over that transport, pushes the queue of the computer, pulls
//! the changes into its mirror, and then waits for the next change. When the link
//! ends it starts it again on the delays of
//! [`horizon_core::place::reconnect_delay`].
//!
//! A link that the server ends with `removed` or `not_logged_in` is the one end
//! that does not retry: this computer forgets its token and stops until a person
//! joins it again.
//!
//! The mirror is the machine list that the server last gave, kept in one private
//! file so a restart while the server is away shows the same list. The records
//! that this computer wrote are marked in the same file: a record that arrived
//! from the server alone is one that this computer has no key binding for, and a
//! click on it must not try the keys of the person.
//!
//! The secrets of this computer follow the server in the same link: a machine key
//! and a credential value are written where the code of this computer reads them,
//! and their bodies come into a push here and nowhere else. [`pull`] applies one
//! answer of the server, [`secrets`] fills a push and keeps a value that lost, and
//! [`view`] answers the machine list and its key bindings.
//!
//! Nothing here reads a clock of the computer for the store, and no frame of the
//! server makes this computer run a command: the only process is the `ssh` of the
//! entry, and the only value that reaches it is its validated target and command.

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use horizon_browser_control::ssh_forward::{ssh_command, ssh_command_with_options};
use horizon_core::machines::{LoadedMachines, MachineBindings, MachineStore};
use horizon_core::secret_store::{self, SecretStore};
use horizon_core::{Machine, MachineError};
use horizon_core::{SecretTarget, SshConnection};

use crate::error::SyncError;
use crate::frame::{read_frame, write_frame};
use crate::protocol::Frame;
use crate::refusal::{LinkRefusal, RefusalCell, of_exit_code};
use connect::{Halt, Opened};
use files::{MIRROR_FILE, MIRROR_FORMAT, QUEUE_FILE, SETTINGS_MIRROR_FILE, device_name};
use link::{End, LinkParts, serve_link};
use pull::{accept_hello, now_millis, view_bindings};
use view::records_of;

mod connect;
mod files;
mod link;
mod live;
mod live_mirror;
mod machine_login;
mod pull;
mod questions;
mod reset;
mod secrets;
mod settings;
mod state;
mod store_writes;
mod view;

pub use live::{LiveHandle, LiveSupervisor};
pub use machine_login::machine_login;
pub use questions::{ServerReply, ServerRequest};
pub use settings::SyncedSettings;
pub(crate) use state::SyncState;
pub use state::{LinkSnapshot, SettingsSyncView, SyncHandle};

/// How long one turn of the supervisor waits for a frame or for local work.
const LINK_SLICE: Duration = Duration::from_millis(100);
/// How long the hello of a start may take before the child is ended. It holds the
/// SSH connect of a reachable server and one round trip.
const HELLO_BOUND: Duration = Duration::from_secs(20);
/// How long a push or a pull of this computer may wait for its answer. The server
/// answers both at once: its store answers a push after the commit, and its pull
/// is one read of the file. A frame that no answer reached after this time is a
/// link that is lost, and not a slow server, so the child is ended and started
/// again.
pub const REQUEST_BOUND: Duration = Duration::from_secs(20);
/// The margin that a wait may take over the bound of the server itself. A wait of
/// the server stays open for [`crate::server::WAIT_BOUND`] (fifteen seconds) and
/// then answers with the changes that it has, so its answer is not due at any
/// shorter time. The margin holds one round trip of the slowest link that this
/// product targets and the schedule of a busy computer, and it keeps the answer of
/// a wait due after the answer of a push and a pull.
pub const WAIT_ANSWER_MARGIN: Duration = Duration::from_secs(6);
/// How long a wait of this computer may stay open before the link is lost. It is
/// the bound of the server plus [`WAIT_ANSWER_MARGIN`], and a test keeps the two
/// in step. An idle link therefore proves that it is alive within about twenty
/// seconds, and a server program that stopped answering is read as away then.
pub const WAIT_ANSWER_BOUND: Duration = Duration::from_secs(21);
/// How long a wait of a server of an older version may stay open. Such a server
/// holds an idle wait for fifty seconds, and this computer cannot hurry it, so its
/// answer is due at that bound plus the margin that this version used.
pub const LEGACY_WAIT_ANSWER_BOUND: Duration = Duration::from_secs(65);
/// The longest line of the standard error of `ssh` that is logged.
pub(super) const MAX_STDERR_LINE: usize = 400;

/// The bounds of one link of one computer: the longest time that its requests may
/// wait for an answer before the supervisor treats the link as lost.
///
/// They are one value so a test needs no minute of patience to see a link that
/// went silent: a test that gives shorter bounds runs the same code path as the
/// bounds of a real link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkBounds {
    /// The bound of a push and of a pull, and of the wait of a start. The server
    /// answers each of them at once, so a longer wait is a dead link.
    pub request: Duration,
    /// The bound of a wait. The server keeps a wait open for its own bound and
    /// then answers it, so this bound is longer than [`Self::request`].
    pub wait: Duration,
}

impl LinkBounds {
    /// The bounds of a real link: [`REQUEST_BOUND`] and [`WAIT_ANSWER_BOUND`].
    pub const STANDARD: Self = Self {
        request: REQUEST_BOUND,
        wait: WAIT_ANSWER_BOUND,
    };

    /// The bounds of one link whose server answered `server_protocol` in its
    /// greeting.
    ///
    /// A server of [`crate::protocol::SHORT_WAIT_PROTOCOL_VERSION`] or later answers
    /// an idle wait at its own short bound, so this computer may treat a silent answer
    /// as a lost link then. A server of an older version holds an idle wait for
    /// fifty seconds, and a shorter bound here would end a healthy link for ever:
    /// such a server keeps [`LEGACY_WAIT_ANSWER_BOUND`].
    #[must_use]
    pub fn of_server(self, server_protocol: u32) -> Self {
        Self {
            request: self.request,
            wait: if server_protocol >= crate::protocol::SHORT_WAIT_PROTOCOL_VERSION {
                self.wait
            } else {
                LEGACY_WAIT_ANSWER_BOUND
            },
        }
    }
}

impl Default for LinkBounds {
    fn default() -> Self {
        Self::STANDARD
    }
}

/// A name that a computer without a login proposes for itself: the prefix of
/// every name that Horizon makes, and eight characters of random.
#[must_use]
pub fn proposed_device_name() -> String {
    files::proposed_device_name()
}

/// The device name that this computer wrote, or `None` when its machine directory
/// holds no name that passes the rule. This reads one file and writes nothing.
///
/// # Errors
/// Returns why the file cannot be read.
pub fn stored_device_name(directory: &Path) -> Result<Option<String>, SyncError> {
    files::stored_device_name(directory)
}

/// Writes `name` as the device name of this computer.
///
/// A join writes this file before it takes the token of its server, so the name of
/// the join is the name that every later link of this computer presents.
///
/// # Errors
/// Refuses a text that is not one device name, and returns why the file cannot be
/// written.
pub fn write_device_name(directory: &Path, name: &str) -> Result<(), SyncError> {
    files::write_device_name(directory, name)
}

/// The settings of one supervisor: where the server is, how to reach it and which
/// files of this computer hold the queue and the mirror.
#[derive(Clone, Debug)]
pub struct SupervisorConfig {
    /// SSH target of the server, as the settings entry names it.
    pub target: String,
    /// The login of the server when the entry names a machine of the machine list:
    /// its address, its user, its port and the machine key of Horizon with its own
    /// known-hosts file. `None` starts `ssh <target>` with the SSH configuration of
    /// the person, as a computer of the person needs.
    pub route: Option<SshConnection>,
    /// Command that the child runs on the server.
    pub command: String,
    /// The program of the child: `ssh`, or a stand-in of a test.
    pub program: String,
    /// The address of the TCP listener of the server, `host:port`, when the
    /// settings name one. A link uses it with the token of a join, and falls back
    /// to the `ssh` child without one.
    pub address: Option<String>,
    /// The secrets file of this computer, which holds the token of a join. The
    /// supervisor reads it on its worker thread before a TCP link, and writes it
    /// when the server removes this computer.
    pub token_store: Option<Arc<SecretStore>>,
    /// The name of this computer on the server.
    pub device: String,
    /// The queue file of this computer.
    pub queue_file: PathBuf,
    /// The mirror file of this computer.
    pub mirror_file: PathBuf,
    /// The directory of the machine list that this computer kept before it
    /// followed a server, whose records seed the first link.
    pub machines_dir: PathBuf,
    /// The settings file of this computer, whose store holds the shared sections
    /// of the settings. `None` keeps every setting on this computer.
    pub settings_path: Option<PathBuf>,
    /// The mirror of the settings that the server gave, in the machine directory
    /// beside the mirror of the machine list.
    pub settings_mirror_file: PathBuf,
    /// The places where this computer keeps its secrets: its machine keys and its
    /// credential values. The link writes every value that the server decided
    /// there, reads the value of a local change from there, and never sends a
    /// value that is not a secret.
    pub secrets: Arc<dyn SecretTarget>,
    /// The bounds of the requests of the link. A test replaces them so it sees a
    /// link that went silent within a second instead of a minute.
    pub bounds: LinkBounds,
    /// Where a TCP link asks for the addresses of the name of the server. The
    /// resolver of this computer has no bound, so the link asks through
    /// [`crate::LinkLookup`] with a bound of its own. A test replaces it, so no test
    /// of a name reads the resolver of this computer.
    pub lookup: crate::LinkLookup,
}

impl SupervisorConfig {
    /// The settings of the supervisor of `target` and `command`, with the queue,
    /// the mirror and the device name of `directory`, and the secrets of
    /// `secrets`.
    ///
    /// # Errors
    /// Returns why the device name of this computer cannot be read or written.
    pub fn for_server(
        target: &str,
        command: &str,
        directory: &Path,
        secrets: Arc<dyn SecretTarget>,
    ) -> Result<Self, SyncError> {
        let device = device_name(directory)?;
        Ok(Self::at(target, command, directory, device, secrets))
    }

    /// The settings of the link of a computer that holds the token of a server.
    ///
    /// The name of the link is the name of that login, which the join wrote before
    /// it took the token: this call never proposes another name. `None` answers
    /// when this computer holds no name for its login, so its link has nothing to
    /// present and a person joins it again.
    ///
    /// # Errors
    /// Returns why the device name of this computer cannot be read.
    pub fn for_login(
        target: &str,
        command: &str,
        directory: &Path,
        secrets: Arc<dyn SecretTarget>,
    ) -> Result<Option<Self>, SyncError> {
        let Some(device) = files::stored_device_name(directory)? else {
            return Ok(None);
        };
        Ok(Some(Self::at(target, command, directory, device, secrets)))
    }

    /// The settings of a link of this computer under `device`, with the files and
    /// the secrets that its home gives.
    fn at(target: &str, command: &str, directory: &Path, device: String, secrets: Arc<dyn SecretTarget>) -> Self {
        Self {
            target: target.to_owned(),
            route: None,
            command: command.to_owned(),
            program: horizon_browser_control::ssh_forward::ssh_program(),
            address: None,
            token_store: None,
            lookup: crate::LinkLookup::system(),
            device,
            queue_file: directory.join(QUEUE_FILE),
            mirror_file: directory.join(MIRROR_FILE),
            machines_dir: directory.to_path_buf(),
            settings_path: None,
            settings_mirror_file: directory.join(SETTINGS_MIRROR_FILE),
            secrets,
            bounds: LinkBounds::STANDARD,
        }
    }

    /// The command of the child, with the program, the flags and the target.
    /// A test replaces the program, and the rest of the call stays the same as
    /// the call of the browser forward.
    #[must_use]
    pub fn command_line(&self) -> Command {
        match &self.route {
            Some(route) => ssh_command_with_options(
                &self.program,
                &route.login_option_args(),
                &route.transport_target(),
                &self.command,
            ),
            None => ssh_command(&self.program, &self.target, &self.command),
        }
    }

    /// The token of this computer on its server, or `None` when it is not logged
    /// in with a TCP link.
    ///
    /// This reads the secrets file, so it runs on the worker thread of the link.
    fn token(&self) -> Option<String> {
        let store = self.token_store.as_deref()?;
        match secret_store::read_token(store) {
            Ok(token) => token.map(|token| token.to_string()),
            Err(error) => {
                tracing::warn!(%error, "the token of this computer could not be read");
                None
            }
        }
    }

    /// The address of a TCP link: one address of the settings and one token.
    fn tcp_address(&self) -> Option<&str> {
        self.address
            .as_deref()
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .filter(|_| self.token_store.is_some())
    }
}

/// The records of the state with its waiting intents applied, and remembered as
/// the answer of this load, so a save turns only what its caller changed into an
/// intent.
/// The supervisor of the link: its thread, the transport that it owns, and the
/// stop flag that ends both.
#[derive(Debug)]
pub struct Supervisor {
    handle: SyncHandle,
    stop: Arc<AtomicBool>,
    halt: Arc<Mutex<Option<Halt>>>,
    thread: Option<JoinHandle<()>>,
    /// The worker that writes the settings store of this computer, so no link waits
    /// for the file sync of a store.
    settings_thread: Option<JoinHandle<()>>,
}

impl Supervisor {
    /// Starts the supervisor of one computer. Nothing runs before this call: the
    /// thread opens its transport, and a server that is away costs one failure
    /// and one delay.
    ///
    /// # Errors
    /// Returns why the queue, the mirror or the device name cannot be read or
    /// written.
    pub fn start(config: SupervisorConfig, wake: impl Fn() + Send + Sync + 'static) -> Result<Self, SyncError> {
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let mut handle = SyncHandle::open(&config, Arc::clone(&wake))?;
        let (kick, kicked) = mpsc::channel();
        let (ask, asked) = mpsc::channel();
        {
            let mut state = handle.lock();
            state.client.away();
            state.generation = state.generation.wrapping_add(1);
        }
        handle.kick = kick;
        handle.requests = ask;
        let stop = Arc::new(AtomicBool::new(false));
        let halt = Arc::new(Mutex::new(None));
        let settings_thread = Some(
            store_writes::start(&handle, &stop)
                .map_err(|error| SyncError::Queue(format!("the thread of the settings store: {error}")))?,
        );
        let thread = {
            let (handle, stop, halt) = (handle.clone(), Arc::clone(&stop), Arc::clone(&halt));
            thread::Builder::new()
                .name("horizon-sync-supervisor".to_owned())
                .spawn(move || run(&config, &handle, &stop, &halt, &kicked, &asked))
                .map_err(|error| SyncError::Queue(format!("the supervisor thread: {error}")))?
        };
        Ok(Self {
            handle,
            stop,
            halt,
            thread: Some(thread),
            settings_thread,
        })
    }

    /// The machine list and the link state of this computer.
    #[must_use]
    pub fn handle(&self) -> &SyncHandle {
        &self.handle
    }

    /// Ends the link and the thread. A transport that is running is ended, so a
    /// blocked read returns at once and the thread ends within one slice.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        halt(&self.halt);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.settings_thread.take() {
            let _ = thread.join();
        }
        reap(&self.halt);
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The life of one supervisor: a link, then the delays of a lost one, until the
/// stop flag is set or the server removes this computer.
fn run(
    config: &SupervisorConfig,
    handle: &SyncHandle,
    stop: &AtomicBool,
    halt_slot: &Arc<Mutex<Option<Halt>>>,
    kicked: &Receiver<()>,
    asked: &Receiver<(ServerRequest, mpsc::Sender<ServerReply>)>,
) {
    let mut attempt: u32 = 1;
    while !stop.load(Ordering::SeqCst) {
        let started = Instant::now();
        // The reason of this attempt: the words of the child, the error of a
        // socket and the loop of the link write it, and the window reads it when
        // the attempt is over.
        let refusal = RefusalCell::default();
        let end = match connect(config, handle, stop, halt_slot, &refusal) {
            Ok(greeted) => {
                let Greeted {
                    mut input,
                    answers,
                    server_protocol,
                } = greeted;
                // An idle wait is proven alive at the bound of the server that
                // answered the greeting, and that bound is shorter from version 5.
                let bounds = config.bounds.of_server(server_protocol);
                serve_link(LinkParts {
                    config,
                    handle,
                    stop,
                    bounds: &bounds,
                    input: &mut input,
                    answers: &answers,
                    kicked,
                    asked,
                    refusal: &refusal,
                })
            }
            Err(failed) => {
                let reason = &failed.reason;
                tracing::warn!(%reason, "the link to the Horizon server did not open");
                failed.end
            }
        };
        halt(halt_slot);
        let exit_code = reap(halt_slot);
        // A question that the link that just ended did not ask waits for no
        // attempt that may never come: the window reads the same sentence that a
        // question of a link that is away reads.
        answer_unasked(asked);
        let reason = refusal
            .get()
            .or_else(|| exit_code.and_then(of_exit_code))
            .unwrap_or(LinkRefusal::Other);
        if end == End::LoggedOut {
            // The server removed this computer, or does not know its token. The
            // token is gone and no retry can bring it back, so the link waits for
            // a person to join this computer again.
            (handle.wake)();
            break;
        }
        if started.elapsed() >= horizon_core::RECONNECT_RESET_AFTER {
            attempt = 1;
        } else {
            attempt = attempt.saturating_add(1);
        }
        {
            let mut state = handle.lock();
            state.client.away();
            state.last_refusal = Some(reason);
            // The request of the link that ended waits for no answer any more. Its
            // start stays with the `Away` state, so a reader that measures the bound
            // of this lost link measures the instant that this link timed.
            let lost = state.request_since.take();
            state.lost_request_since = lost;
            state.generation = state.generation.wrapping_add(1);
        }
        (handle.wake)();
        if stop.load(Ordering::SeqCst) {
            break;
        }
        if sleep(horizon_core::reconnect_delay(attempt), stop, kicked) {
            // A person saved a setting or a secret during the wait, and the attempt
            // that it started counts as a new start: the wait after it counts from
            // the first delay again, as a key press on a machine terminal forgets
            // the attempts of that terminal.
            attempt = 1;
        }
    }
}

/// Why one attempt at a link did not open it.
///
/// A transport that failed and a server that stopped answering are both a lost
/// link, and the delays of [`horizon_core::place::reconnect_delay`] start another
/// attempt. A greeting that the server refused because it does not accept the
/// login of this computer is not one of them: no attempt can succeed until a
/// person joins this computer again.
struct FailedLink {
    /// One sentence for the log.
    reason: String,
    /// How this attempt ended, which decides whether another one starts.
    end: End,
}

impl From<String> for FailedLink {
    fn from(reason: String) -> Self {
        Self { reason, end: End::Link }
    }
}

impl FailedLink {
    /// The refusal of the server that no later attempt can repeat, because the
    /// token of this computer is gone.
    fn logged_out(reason: &str, code: crate::protocol::LinkErrorCode) -> Self {
        let reason = format!("the server refused the link: {}: {reason}", code.as_str());
        Self {
            reason,
            end: End::LoggedOut,
        }
    }
}

/// The halves of a link that the server greeted: the side that this computer
/// writes, the answers that it reads, and the protocol version of that server.
struct Greeted {
    input: Box<dyn Write + Send>,
    answers: Receiver<Frame>,
    server_protocol: u32,
}

/// Opens the sync link: the greeting of [`greet`] with the hello of the queue of
/// this computer, whose state keeps the answer.
fn connect(
    config: &SupervisorConfig,
    handle: &SyncHandle,
    stop: &AtomicBool,
    halt_slot: &Arc<Mutex<Option<Halt>>>,
    refusal: &RefusalCell,
) -> Result<Greeted, FailedLink> {
    let hello = |token: Option<&str>| handle.lock().client.hello_frame(token);
    let logged_out = |code| logged_out(config, handle, code);
    let (greeted, answer) = greet(config, stop, halt_slot, refusal, hello, logged_out)?;
    accept_hello(handle, &answer)?;
    Ok(greeted)
}

/// The frames of the server that may wait for the loop of a link. The reader of
/// the link waits with the next one, so a link holds a bounded number of them.
const ANSWER_DEPTH: usize = 32;

/// Starts one transport and its reader, sends the hello that `hello` makes with
/// the token of this computer, and answers the two halves of the link and the
/// greeting of the server once the server greeted this computer. `logged_out`
/// hears the code of a refusal that no later attempt can repeat.
fn greet(
    config: &SupervisorConfig,
    stop: &AtomicBool,
    halt_slot: &Arc<Mutex<Option<Halt>>>,
    refusal: &RefusalCell,
    hello: impl FnOnce(Option<&str>) -> Frame,
    logged_out: impl FnOnce(crate::protocol::LinkErrorCode),
) -> Result<(Greeted, crate::protocol::HelloOk), FailedLink> {
    // A TCP link reads the token row of this computer, and an `ssh` link leaves
    // the program to the machine terminal of the person. Both run here, on the
    // worker thread of the link.
    let token = config.tcp_address().and_then(|_| config.token());
    if config.tcp_address().is_some() && token.is_none() {
        return Err("this computer has no token of the server; join it again"
            .to_owned()
            .into());
    }
    let opened = if let Some(address) = config.tcp_address() {
        connect::tcp(address, &config.lookup, refusal)?
    } else {
        let mut command = config.command_line();
        connect::ssh(&mut command, &config.program, refusal)?
    };
    let Opened {
        writer,
        answers,
        halt: handle_halt,
    } = opened;
    *halt_slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle_halt);

    let (frames, incoming) = mpsc::sync_channel(ANSWER_DEPTH);
    thread::Builder::new()
        .name("horizon-sync-reader".to_owned())
        .spawn(move || read_answers(answers, &frames))
        .map_err(|error| format!("the reader thread: {error}"))?;

    let hello = hello(token.as_deref());
    let mut input = writer;
    write_frame(&mut input, &hello).map_err(|error| format!("the hello was not sent: {error}"))?;
    let deadline = Instant::now() + HELLO_BOUND;
    loop {
        if stop.load(Ordering::SeqCst) {
            return Err("the link was stopped before the server answered".to_owned().into());
        }
        match incoming.recv_timeout(LINK_SLICE) {
            Ok(Frame::HelloOk(answer)) => {
                let greeted = Greeted {
                    input,
                    answers: incoming,
                    server_protocol: answer.protocol,
                };
                return Ok((greeted, answer));
            }
            Ok(Frame::Error(error)) => {
                let code = error.code;
                let reason = error.message;
                if matches!(
                    code,
                    crate::protocol::LinkErrorCode::Removed | crate::protocol::LinkErrorCode::NotLoggedIn
                ) {
                    logged_out(code);
                    return Err(FailedLink::logged_out(&reason, code));
                }
                return Err(FailedLink {
                    reason: format!("the server refused the link: {}: {reason}", code.as_str()),
                    end: End::Link,
                });
            }
            Ok(_) => return Err("the server answered the hello with another frame".to_owned().into()),
            Err(RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                refusal.set(LinkRefusal::Silent);
                return Err("the server did not answer the hello in time".to_owned().into());
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err("the link ended before the server answered the hello".to_owned().into());
            }
        }
    }
}

/// Forgets the token of this computer and publishes the state that the person
/// reads, because the server does not accept it any more.
pub(super) fn logged_out(config: &SupervisorConfig, handle: &SyncHandle, code: crate::protocol::LinkErrorCode) {
    let not_logged_in = code == crate::protocol::LinkErrorCode::NotLoggedIn;
    if let Some(store) = config.token_store.as_deref()
        && let Err(error) = secret_store::remove_token(store)
    {
        tracing::warn!(%error, "the token of this computer could not be removed");
    }
    let mut state = handle.lock();
    state.client.logged_out(not_logged_in);
}

/// Fills the bodies of the secret intents of one push from where this computer
/// keeps them, and drops an intent whose local value is gone.
///
/// The queue of a computer holds the id of a secret and never the value, so the
/// body comes into the frame here and nowhere else. A value that this computer no
/// longer holds is dropped with one line, because the intent names a value that
/// the server cannot be given; the local value itself is untouched.
/// Reads the frames of the transport into `answers` until the link ends.
fn read_answers(mut output: impl std::io::Read, answers: &mpsc::SyncSender<Frame>) {
    loop {
        match read_frame(&mut output) {
            Ok(Some(frame)) => {
                if answers.send(frame).is_err() {
                    return;
                }
            }
            Ok(None) => return,
            Err(error) => {
                tracing::debug!(%error, "the link of the server ended");
                return;
            }
        }
    }
}

/// Waits for the delay of the next attempt, and answers whether a local change
/// ended the wait. A stopped supervisor and a link without a kick answer `false`.
///
/// A local change is a person who saved a setting or a secret while the server
/// was away, so it starts one attempt at once instead of waiting for the rest of
/// the delay: the person who saves a setting expects it to travel.
fn sleep(delay: Duration, stop: &AtomicBool, kicked: &Receiver<()>) -> bool {
    let deadline = Instant::now() + delay;
    while !stop.load(Ordering::SeqCst) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        match kicked.recv_timeout(remaining.min(LINK_SLICE)) {
            Ok(()) => {
                tracing::debug!("a local change ended the wait of the next attempt of the link");
                return true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
    false
}

/// Answers every question that no link asked, so the Server tab ends its wait at
/// once instead of at the truth of a channel that only an open link reads.
fn answer_unasked(asked: &Receiver<(ServerRequest, mpsc::Sender<ServerReply>)>) {
    while let Ok((_, reply)) = asked.try_recv() {
        let _ = reply.send(ServerReply::Failed(questions::NO_LINK_SENTENCE.to_owned()));
    }
}

/// Ends the transport of the link, so a read that waits returns.
fn halt(halt_slot: &Arc<Mutex<Option<Halt>>>) {
    let mut slot = halt_slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(halt) = slot.as_mut() {
        halt.end();
    }
}

/// Waits for the child of the link that ended, so no zombie stays behind, and
/// answers the exit status that names a reason of its own.
fn reap(halt_slot: &Arc<Mutex<Option<Halt>>>) -> Option<i32> {
    let halt = halt_slot.lock().unwrap_or_else(PoisonError::into_inner).take();
    halt.and_then(Halt::reap)
}

fn queue_error(error: &SyncError) -> MachineError {
    MachineError::Unavailable {
        reason: error.to_string(),
    }
}

/// One machine store of a computer that follows a server: the records are the
/// mirror of the server with the intents that wait for it applied, and a save
/// writes intents instead of a file.
#[derive(Clone, Debug)]
pub struct SyncedMachineStore {
    handle: SyncHandle,
}

impl SyncedMachineStore {
    /// The store of the machine list of `handle`.
    #[must_use]
    pub fn new(handle: SyncHandle) -> Self {
        Self { handle }
    }

    /// The handle of the supervisor behind this store.
    #[must_use]
    pub fn handle(&self) -> &SyncHandle {
        &self.handle
    }
}

impl MachineStore for SyncedMachineStore {
    fn load(&self) -> Result<LoadedMachines, MachineError> {
        let mut state = self.handle.lock();
        let records = records_of(&mut state);
        let keys = view_bindings(&state).into_iter().collect();
        Ok(LoadedMachines { records, keys })
    }

    fn save(&self, records: &[Machine]) -> Result<(), MachineError> {
        self.handle.save(records)
    }

    fn save_bindings(&self, bindings: &MachineBindings) -> Result<(), MachineError> {
        self.handle.save_bindings(bindings)
    }

    fn holds_bindings(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::os::unix::fs::PermissionsExt as _;

    use crate::client::Client;
    use crate::protocol::check_device_name;
    use crate::test_support::keys_of_a_test;

    use super::files::DEVICE_FILE;
    use super::*;

    #[test]
    fn a_wait_may_take_the_bound_of_the_server_and_its_margin() {
        assert_eq!(WAIT_ANSWER_BOUND, crate::server::WAIT_BOUND + WAIT_ANSWER_MARGIN);
        assert_eq!(LinkBounds::STANDARD.wait, WAIT_ANSWER_BOUND);
        assert_eq!(LinkBounds::STANDARD.request, REQUEST_BOUND);
        assert_eq!(LinkBounds::default(), LinkBounds::STANDARD);
        assert!(
            REQUEST_BOUND < WAIT_ANSWER_BOUND,
            "a wait stays open at the server, and a push and a pull do not"
        );
        assert!(
            HELLO_BOUND >= REQUEST_BOUND,
            "the hello holds the connect of the SSH of the link"
        );
    }

    /// A normal answer of a first link is not a warning. The row of the server
    /// decides a write that names no version, and its content says whether the
    /// person lost a value: the settings and the secrets write the line about a
    /// loss themselves, so the line of the refusal is a debug line. A refusal that
    /// loses a change of this computer with no other line stays a warning.
    #[test]
    fn a_refusal_that_lost_nothing_is_a_debug_line_and_a_loss_is_a_warning() {
        use std::sync::{Arc, Mutex};

        use crate::error::Refusal;

        /// The captured lines of one log.
        struct Sink(Arc<Mutex<Vec<u8>>>);

        impl std::io::Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("the log").extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&log);
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || Sink(Arc::clone(&sink)))
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            // The answer of the first link of a computer that already holds the
            // eleven sections of the server: the row of the server decides.
            super::pull::log_refusal(7, &Refusal::AlreadyAnswered { version: 1 });
            // A change of this computer that another computer got first: no other
            // line says why it is gone for a record that is not a setting.
            super::pull::log_refusal(8, &Refusal::ChangedSinceSeen { seen: 1, version: 2 });
        });

        let text = String::from_utf8_lossy(&log.lock().expect("the log")).into_owned();
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| line.contains("horizon_sync::supervisor::pull"))
            .collect();
        let answered: Vec<&&str> = lines.iter().filter(|line| line.contains("already_answered")).collect();
        assert_eq!(answered.len(), 1, "the equal-content case has one line: {text}");
        assert!(
            answered[0].contains("DEBUG"),
            "the normal answer is a debug line: {}",
            answered[0]
        );
        assert!(
            !lines
                .iter()
                .any(|line| line.contains("WARN") && line.contains("already_answered")),
            "a normal case is no warning: {text}"
        );
        let lost: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains("changed_since_seen"))
            .collect();
        assert_eq!(lost.len(), 1, "the real loss has one line: {text}");
        assert!(
            lost[0].contains("WARN"),
            "a refusal that a person must know about is a warning: {}",
            lost[0]
        );
    }

    /// A server that answers with another store than the one this computer saw
    /// last replaces the machine list and the settings of this computer at the next
    /// pull, so the person reads one line about it. The first link of a computer is
    /// no such change, and a line is not repeated for one store.
    #[test]
    fn a_server_with_another_store_tells_the_person() {
        let directory = tempfile::tempdir().expect("a directory");
        let keys = horizon_core::MachineKeys::new(directory.path().join("keys"));
        let secrets = keys_of_a_test(keys);
        let mut config =
            SupervisorConfig::for_server("server-vm", "/opt/horizon --server-link", directory.path(), secrets)
                .expect("the settings of the link");
        config.program = "/bin/true".to_owned();
        let handle = SyncHandle::open(&config, Arc::new(|| {})).expect("the handle of a link");
        let hello = |store: &str| crate::protocol::HelloOk {
            protocol: crate::protocol::PROTOCOL_VERSION,
            store: store.to_owned(),
            now_millis: 1_760_000_000_000,
            secrets_lost: false,
        };

        super::pull::accept_hello(&handle, &hello("one")).expect("the first hello of the link");
        assert!(
            handle.take_notices().is_empty(),
            "the first link of a computer says nothing about the store"
        );

        super::pull::accept_hello(&handle, &hello("two")).expect("the hello of another store");
        let notices = handle.take_notices();
        assert_eq!(notices.len(), 1, "another store is one line: {notices:?}");
        assert!(
            notices[0].contains("another store") && notices[0].contains("machine list"),
            "the line names what the person lost and what replaced it: {notices:?}"
        );

        super::pull::accept_hello(&handle, &hello("two")).expect("the same store again");
        assert!(
            handle.take_notices().is_empty(),
            "a store that this computer already took says nothing"
        );
    }

    /// B7: an idle link proves that it is alive at the bound of the server that
    /// answered the greeting, and a silent server program is read as away within
    /// about twenty seconds. A server of an older version holds its wait for fifty
    /// seconds, so this computer keeps the longer bound for it: a shorter one would
    /// end a healthy link again and again.
    #[test]
    fn an_idle_link_is_proven_alive_at_the_bound_of_the_server_that_it_reached() {
        let modern = LinkBounds::STANDARD.of_server(crate::protocol::SERVER_PROTOCOL_VERSION);
        assert_eq!(modern.wait, WAIT_ANSWER_BOUND);
        assert!(
            modern.wait <= Duration::from_secs(21),
            "an idle link is proven within about 20 seconds"
        );
        assert_eq!(
            LinkBounds::STANDARD
                .of_server(crate::protocol::SERVER_PROTOCOL_VERSION + 1)
                .wait,
            WAIT_ANSWER_BOUND,
            "a later server speaks the same bound"
        );
        for older in [1, 2, 3, 4] {
            assert_eq!(
                LinkBounds::STANDARD.of_server(older).wait,
                LEGACY_WAIT_ANSWER_BOUND,
                "a server of protocol {older} holds its wait for fifty seconds"
            );
        }
        assert_eq!(
            LinkBounds::STANDARD
                .of_server(crate::protocol::SHORT_WAIT_PROTOCOL_VERSION)
                .wait,
            WAIT_ANSWER_BOUND,
            "a server of the version before the live link holds its wait for fifteen seconds"
        );
        assert_eq!(
            modern.request, REQUEST_BOUND,
            "the bound of a push and a pull does not depend on the server"
        );

        // The traffic of an idle computer: one wait and one answer of changes for
        // every bound of the server, and each frame is small.
        let wait = serde_json::to_string(&Frame::Wait(crate::protocol::Wait { since: 4_194_304 }))
            .expect("the wait of an idle link");
        let changes = serde_json::to_string(&Frame::Changes(crate::store::Changes {
            index: 4_194_304,
            reset: false,
            rows: Vec::new(),
        }))
        .expect("the answer of an idle wait");
        // The frame of a link is its length as four bytes and then its JSON.
        let round = wait.len() + changes.len() + 8;
        let per_minute = round * (60 / usize::try_from(crate::server::WAIT_BOUND.as_secs()).expect("seconds"));
        assert_eq!(
            per_minute,
            348,
            "an idle computer sends {round} bytes for every {} seconds, {per_minute} bytes a minute",
            crate::server::WAIT_BOUND.as_secs()
        );
        assert!(
            per_minute < 4096,
            "an idle link costs less than four kilobytes a minute: {per_minute}"
        );
    }

    #[test]
    fn a_secret_notice_of_the_link_is_shown_once() {
        let directory = tempfile::tempdir().expect("a directory");
        let keys = horizon_core::MachineKeys::new(directory.path().join("keys"));
        let secrets = keys_of_a_test(keys);
        let mut config =
            SupervisorConfig::for_server("server-vm", "/opt/horizon --server-link", directory.path(), secrets)
                .expect("the settings of the link");
        config.program = "/bin/true".to_owned();
        let handle = SyncHandle::open(&config, Arc::new(|| {})).expect("the handle of a link");

        // The worker of the secrets keeps the line for the person, and the frame
        // takes it once: a frame that ran twice shows one line, not two.
        handle.push_notice("the server kept its value of studio".to_owned());
        handle.push_notice("the server kept its value of shop".to_owned());
        assert_eq!(handle.take_notices().len(), 2);
        assert!(handle.take_notices().is_empty(), "a taken line is not shown twice");
    }

    #[test]
    fn a_list_that_a_save_keeps_writes_no_intent() {
        let directory = tempfile::tempdir().expect("a directory");
        let keys = horizon_core::MachineKeys::new(directory.path().join("keys"));
        let secrets = keys_of_a_test(keys);
        let mut config =
            SupervisorConfig::for_server("server-vm", "/opt/horizon --server-link", directory.path(), secrets)
                .expect("the settings of the link");
        config.program = "/bin/true".to_owned();
        let handle = SyncHandle::open(&config, Arc::new(|| {})).expect("the handle of a link");
        let store = SyncedMachineStore::new(handle.clone());
        let records = vec![horizon_core::Machine {
            name: "build-1".to_owned(),
            host: "203.0.113.10".to_owned(),
            added_at_millis: 1,
            ..horizon_core::Machine::default()
        }];

        // One change of the sidebar writes one intent, and the next save of the
        // list that this computer holds writes none: the sidebar writes the whole
        // list, so a save that keeps it is not a change that waits for a server.
        store.save(&records).expect("the first change");
        assert_eq!(handle.pending(), BTreeSet::from(["build-1".to_string()]));
        store.save(&records).expect("the same list again");
        assert_eq!(
            handle.pending(),
            BTreeSet::from(["build-1".to_string()]),
            "a save that keeps the list counts no second change"
        );
    }

    #[test]
    fn a_device_name_is_written_once_and_stays() {
        let directory = tempfile::tempdir().expect("a directory");
        let first = device_name(directory.path()).expect("a name");
        assert!(check_device_name(&first), "{first}");
        assert_eq!(device_name(directory.path()).expect("the same name"), first);
        assert_eq!(
            std::fs::metadata(directory.path().join(DEVICE_FILE))
                .expect("the file")
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "the device name is private"
        );
    }

    #[test]
    fn a_refusal_names_the_version_that_the_mark_reads_again() {
        let directory = tempfile::tempdir().expect("a directory");
        let mut client = Client::open(directory.path().join("server-queue.json"), "mbp-7c1").expect("a queue");
        client.accept_mark(9).expect("the mark");
        assert_eq!(client.read_from(), 9, "a fresh link reads from its mark");

        // A refusal at the index that this computer already holds, as a value of
        // the server that this computer applied before a reseed.
        client.replay_before(9);
        assert_eq!(client.read_from(), 8, "the value of that version comes again");
        client.accept_mark(10).expect("the mark");
        assert_eq!(client.read_from(), 8, "an answer of the old request does not end it");
        client.accept_replay(8);
        assert_eq!(client.read_from(), 10, "its own answer ends it");

        // A version that the next request covers anyway needs no second read, and
        // a refusal without a version never moves the mark.
        client.replay_before(12);
        assert_eq!(client.read_from(), 10);
        client.replay_before(0);
        assert_eq!(client.read_from(), 10);
    }

    #[test]
    fn a_device_file_of_another_rule_is_replaced() {
        let directory = tempfile::tempdir().expect("a directory");
        std::fs::write(directory.path().join(DEVICE_FILE), "Not A Device/Name\n").expect("a bad name");
        let name = device_name(directory.path()).expect("a name");
        assert!(check_device_name(&name), "{name}");
        assert!(name.starts_with("horizon-"), "{name}");
    }

    #[test]
    fn a_login_reads_the_name_of_its_join_and_never_proposes_another() {
        let directory = tempfile::tempdir().expect("a directory");
        let keys = horizon_core::MachineKeys::new(directory.path().join("keys"));
        let proposed = proposed_device_name();
        assert!(check_device_name(&proposed), "{proposed}");
        assert_eq!(
            stored_device_name(directory.path()).expect("no name yet"),
            None,
            "a fresh computer has no name until a start or a join writes one"
        );

        // A join writes the name of this computer before it takes the token, so the
        // link of the login reads the name of the join.
        write_device_name(directory.path(), "window-b").expect("the name of the join");
        assert_eq!(
            stored_device_name(directory.path()).expect("a name"),
            Some("window-b".to_owned())
        );
        let config = SupervisorConfig::for_login(
            "server-vm",
            "/opt/horizon --server-link",
            directory.path(),
            keys_of_a_test(keys),
        )
        .expect("the settings of the link")
        .expect("the name of the login");
        assert_eq!(config.device, "window-b");

        // A login that holds no name is no link: the name is never invented for it.
        let nameless = tempfile::tempdir().expect("a directory");
        let keys = horizon_core::MachineKeys::new(nameless.path().join("keys"));
        assert!(
            SupervisorConfig::for_login(
                "server-vm",
                "/opt/horizon --server-link",
                nameless.path(),
                keys_of_a_test(keys),
            )
            .expect("the settings of the link")
            .is_none()
        );
        assert_eq!(
            stored_device_name(nameless.path()).expect("no name"),
            None,
            "a login that read no name wrote none"
        );
        assert!(write_device_name(nameless.path(), "Not A Name").is_err());
    }
}
