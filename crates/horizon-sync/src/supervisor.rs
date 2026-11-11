//! The supervisor of the link of one computer: one `ssh` child, the protocol of
//! the store over its standard input and output, and the state that the sidebar
//! reads.
//!
//! The supervisor runs on a worker thread and never on the UI thread. It starts
//! `ssh <target> <command>` with the bounded flags of the browser forward, speaks
//! the frames of [`crate::client`] over that child, pushes the queue of the
//! computer, pulls the changes into its mirror, and then waits for the next
//! change. When the child ends it starts it again on the delays of
//! [`horizon_core::place::reconnect_delay`].
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
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use horizon_browser_control::ssh_forward::ssh_command;
use horizon_core::SecretTarget;
use horizon_core::machines::{LoadedMachines, MachineBindings, MachineStore};
use horizon_core::{Machine, MachineError};

use crate::error::SyncError;
use crate::frame::{read_frame, write_frame};
use crate::protocol::Frame;
use files::{MIRROR_FILE, MIRROR_FORMAT, QUEUE_FILE, device_name};
use pull::{accept_hello, accept_pushed, apply_changes, merged_bindings, now_millis};
use secrets::fill_secrets;
use view::records_of;

mod files;
mod pull;
mod secrets;
mod state;
mod view;

pub(crate) use state::SyncState;
pub use state::{LinkSnapshot, SyncHandle};

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
/// the server stays open for [`crate::server::WAIT_BOUND`] (50 seconds) and then
/// answers with the changes that it has, so its answer is not due at any shorter
/// time.
pub const WAIT_ANSWER_MARGIN: Duration = Duration::from_secs(15);
/// How long a wait of this computer may stay open before the link is lost. It is
/// the bound of the server plus [`WAIT_ANSWER_MARGIN`], and a test keeps the two
/// in step.
pub const WAIT_ANSWER_BOUND: Duration = Duration::from_secs(65);
/// The longest line of the standard error of `ssh` that is logged.
const MAX_STDERR_LINE: usize = 400;

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
}

impl Default for LinkBounds {
    fn default() -> Self {
        Self::STANDARD
    }
}

/// The settings of one supervisor: where the server is, how to reach it and which
/// files of this computer hold the queue and the mirror.
#[derive(Clone, Debug)]
pub struct SupervisorConfig {
    /// SSH target of the server, as the settings entry names it.
    pub target: String,
    /// Command that the child runs on the server.
    pub command: String,
    /// The program of the child: `ssh`, or a stand-in of a test.
    pub program: String,
    /// The name of this computer on the server.
    pub device: String,
    /// The queue file of this computer.
    pub queue_file: PathBuf,
    /// The mirror file of this computer.
    pub mirror_file: PathBuf,
    /// The directory of the machine list that this computer kept before it
    /// followed a server, whose records seed the first link.
    pub machines_dir: PathBuf,
    /// The places where this computer keeps its secrets: its machine keys and its
    /// credential values. The link writes every value that the server decided
    /// there, reads the value of a local change from there, and never sends a
    /// value that is not a secret.
    pub secrets: Arc<dyn SecretTarget>,
    /// The bounds of the requests of the link. A test replaces them so it sees a
    /// link that went silent within a second instead of a minute.
    pub bounds: LinkBounds,
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
        Ok(Self {
            target: target.to_owned(),
            command: command.to_owned(),
            program: horizon_browser_control::ssh_forward::ssh_program(),
            device: device_name(directory)?,
            queue_file: directory.join(QUEUE_FILE),
            mirror_file: directory.join(MIRROR_FILE),
            machines_dir: directory.to_path_buf(),
            secrets,
            bounds: LinkBounds::STANDARD,
        })
    }

    /// The command of the child, with the program, the flags and the target.
    /// A test replaces the program, and the rest of the call stays the same as
    /// the call of the browser forward.
    #[must_use]
    pub fn command_line(&self) -> Command {
        ssh_command(&self.program, &self.target, &self.command)
    }
}

/// The records of the state with its waiting intents applied, and remembered as
/// the answer of this load, so a save turns only what its caller changed into an
/// intent.
/// The supervisor of the link: its thread, the child that it owns, and the stop
/// flag that ends both.
#[derive(Debug)]
pub struct Supervisor {
    handle: SyncHandle,
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    thread: Option<JoinHandle<()>>,
}

impl Supervisor {
    /// Starts the supervisor of one computer. Nothing runs before this call: the
    /// thread opens its child, and a server that is away costs one failure and
    /// one delay.
    ///
    /// # Errors
    /// Returns why the queue, the mirror or the device name cannot be read or
    /// written.
    pub fn start(config: SupervisorConfig, wake: impl Fn() + Send + Sync + 'static) -> Result<Self, SyncError> {
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let mut handle = SyncHandle::open(&config, Arc::clone(&wake))?;
        let (kick, kicked) = mpsc::channel();
        {
            let mut state = handle.lock();
            state.client.away();
            state.generation = state.generation.wrapping_add(1);
        }
        handle.kick = kick;
        let stop = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(None));
        let thread = {
            let (handle, stop, child) = (handle.clone(), Arc::clone(&stop), Arc::clone(&child));
            thread::Builder::new()
                .name("horizon-sync-supervisor".to_owned())
                .spawn(move || run(&config, &handle, &stop, &child, &kicked))
                .map_err(|error| SyncError::Queue(format!("the supervisor thread: {error}")))?
        };
        Ok(Self {
            handle,
            stop,
            child,
            thread: Some(thread),
        })
    }

    /// The machine list and the link state of this computer.
    #[must_use]
    pub fn handle(&self) -> &SyncHandle {
        &self.handle
    }

    /// Ends the link and the thread. A child that is running is killed, so a
    /// blocked read returns at once and the thread ends within one slice.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        kill(&self.child);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        kill(&self.child);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The life of one supervisor: a link, then the delays of a lost one, until the
/// stop flag is set.
fn run(
    config: &SupervisorConfig,
    handle: &SyncHandle,
    stop: &AtomicBool,
    child_slot: &Arc<Mutex<Option<Child>>>,
    kicked: &Receiver<()>,
) {
    let mut attempt: u32 = 1;
    while !stop.load(Ordering::SeqCst) {
        let started = Instant::now();
        match connect(config, handle, stop, child_slot, kicked) {
            Ok((mut input, answers)) => {
                serve_link(handle, stop, &config.bounds, &mut input, &answers, kicked);
                kill(child_slot);
                reap(child_slot);
                if started.elapsed() >= horizon_core::RECONNECT_RESET_AFTER {
                    attempt = 1;
                } else {
                    attempt = attempt.saturating_add(1);
                }
            }
            Err(reason) => {
                tracing::warn!(%reason, "the link to the Horizon server did not open");
                kill(child_slot);
                reap(child_slot);
                attempt = attempt.saturating_add(1);
            }
        }
        {
            let mut state = handle.lock();
            state.client.away();
            state.generation = state.generation.wrapping_add(1);
        }
        (handle.wake)();
        if stop.load(Ordering::SeqCst) {
            break;
        }
        sleep(horizon_core::reconnect_delay(attempt), stop, kicked);
    }
}

/// Starts one child and its reader, sends the hello, and answers the two halves
/// of the link once the server greeted this computer.
fn connect(
    config: &SupervisorConfig,
    handle: &SyncHandle,
    stop: &AtomicBool,
    child_slot: &Arc<Mutex<Option<Child>>>,
    _kicked: &Receiver<()>,
) -> Result<(ChildStdin, Receiver<Frame>), String> {
    let mut command = config.command_line();
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start `{}`: {error}", config.program))?;
    let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("the child of the link has no standard input or output".to_owned());
    };
    if let Some(stderr) = child.stderr.take() {
        thread::Builder::new()
            .name("horizon-sync-stderr".to_owned())
            .spawn(move || log_stderr(stderr))
            .ok();
    }
    *child_slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(child);

    let (answers, incoming) = mpsc::channel();
    thread::Builder::new()
        .name("horizon-sync-reader".to_owned())
        .spawn(move || read_answers(output, &answers))
        .map_err(|error| format!("the reader thread: {error}"))?;

    let hello = {
        let mut state = handle.lock();
        state.client.hello_frame()
    };
    let mut input = input;
    write_frame(&mut input, &hello).map_err(|error| format!("the hello was not sent: {error}"))?;
    let deadline = Instant::now() + HELLO_BOUND;
    loop {
        if stop.load(Ordering::SeqCst) {
            return Err("the link was stopped before the server answered".to_owned());
        }
        match incoming.recv_timeout(LINK_SLICE) {
            Ok(Frame::HelloOk(answer)) => {
                accept_hello(handle, &answer)?;
                return Ok((input, incoming));
            }
            Ok(Frame::Error(error)) => {
                return Err(format!(
                    "the server refused the link: {}: {}",
                    error.code.as_str(),
                    error.message
                ));
            }
            Ok(_) => return Err("the server answered the hello with another frame".to_owned()),
            Err(RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                return Err("the server did not answer the hello in time".to_owned());
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err("the link ended before the server answered the hello".to_owned());
            }
        }
    }
}

/// Answers the frames of one open link until it ends, the stop flag is set, or a
/// request stays without an answer for its bound.
///
/// A bound is what an idle computer needs most: a server that dies leaves the
/// socket of the child open for as long as the `ssh` of the link lives, so a wait
/// that never answers is the only sign of a link that is over. The answer of a
/// push and of a pull is due at once, and the answer of a wait is due at the
/// bound of the server that holds it open.
fn serve_link(
    handle: &SyncHandle,
    stop: &AtomicBool,
    bounds: &LinkBounds,
    input: &mut ChildStdin,
    answers: &Receiver<Frame>,
    kicked: &Receiver<()>,
) {
    let mut waiting: Option<Instant> = None;
    // The index that the open request reads from, so the answer is known to be
    // the one that a mark set back or not.
    let mut asking_from: u64 = 0;
    let mut sending: Option<Instant> = None;
    // The first request of a link is a pull: the server answers it at once, while
    // a wait stays open at its bound when nothing changed yet.
    let mut first = true;
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        // One push at a time, and no push while another one waits for its answer.
        if sending.is_none()
            && let Some(chunk) = {
                let state = handle.lock();
                state.client.next_push()
            }
        {
            let intents = fill_secrets(handle, chunk);
            if !intents.is_empty() {
                let frame = Frame::Push(crate::protocol::Push { intents });
                if let Err(error) = write_frame(input, &frame) {
                    tracing::warn!(%error, "the link could not send a push");
                    return;
                }
                sending = Some(Instant::now());
            }
        }
        if waiting.is_none() {
            let frame = {
                let state = handle.lock();
                asking_from = state.client.read_from();
                if first {
                    state.client.pull_frame()
                } else {
                    state.client.wait_frame()
                }
            };
            if let Err(error) = write_frame(input, &frame) {
                tracing::warn!(%error, "the link could not ask for changes");
                return;
            }
            waiting = Some(Instant::now());
        }
        match answers.recv_timeout(LINK_SLICE) {
            Ok(Frame::Pushed(reply)) => {
                sending = None;
                accept_pushed(handle, &reply);
            }
            Ok(Frame::Changes(changes)) => {
                waiting = None;
                first = false;
                if let Err(error) = apply_changes(handle, &changes, asking_from) {
                    tracing::warn!(%error, "the changes of the server were not applied");
                    return;
                }
                (handle.wake)();
            }
            Ok(Frame::Error(error)) => {
                tracing::warn!(code = error.code.as_str(), reason = %error.message, "the server ended the link");
                return;
            }
            Ok(_) => {
                tracing::warn!("the server answered a frame that this computer did not ask for");
                return;
            }
            Err(RecvTimeoutError::Timeout) => {
                let answer_due = if first { bounds.request } else { bounds.wait };
                if past_bound(sending, bounds.request) {
                    tracing::warn!(
                        bound_secs = bounds.request.as_secs(),
                        "the link did not answer a push in time, so it is lost"
                    );
                    return;
                }
                if past_bound(waiting, answer_due) {
                    tracing::warn!(
                        bound_secs = answer_due.as_secs(),
                        "the link did not answer a wait in time, so it is lost"
                    );
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
        // A local change is the only work of a turn that the server did not
        // answer; the links of both are read on the next pass of the loop.
        while kicked.try_recv().is_ok() {}
    }
}

/// Whether a request that was sent at `since` has waited for its answer longer
/// than `bound`.
fn past_bound(since: Option<Instant>, bound: Duration) -> bool {
    since.is_some_and(|since| since.elapsed() >= bound)
}

/// Fills the bodies of the secret intents of one push from where this computer
/// keeps them, and drops an intent whose local value is gone.
///
/// The queue of a computer holds the id of a secret and never the value, so the
/// body comes into the frame here and nowhere else. A value that this computer no
/// longer holds is dropped with one line, because the intent names a value that
/// the server cannot be given; the local value itself is untouched.
/// Reads the frames of the child into `answers` until the link ends.
fn read_answers(mut output: impl std::io::Read, answers: &Sender<Frame>) {
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

/// Logs the standard error of the child, a line at a time. A pipe that nobody
/// reads stops the child, so this thread runs for every link.
fn log_stderr(stderr: impl std::io::Read) {
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { return };
        let line = line.chars().take(MAX_STDERR_LINE).collect::<String>();
        if !line.trim().is_empty() {
            tracing::warn!(target: "horizon_sync::ssh", "{line}");
        }
    }
}

/// Waits for the delay of the next attempt, and returns at once when the
/// supervisor is stopped or a local change arrives.
fn sleep(delay: Duration, stop: &AtomicBool, kicked: &Receiver<()>) {
    let deadline = Instant::now() + delay;
    while !stop.load(Ordering::SeqCst) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        match kicked.recv_timeout(remaining.min(LINK_SLICE)) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Ends the child of the link, so a read that waits for it returns.
fn kill(child_slot: &Arc<Mutex<Option<Child>>>) {
    let mut slot = child_slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(child) = slot.as_mut() {
        let _ = child.kill();
    }
}

/// Waits for the child of the link that ended, so no zombie stays behind.
fn reap(child_slot: &Arc<Mutex<Option<Child>>>) {
    let child = child_slot.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(mut child) = child {
        let _ = child.wait();
    }
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
        let keys = merged_bindings(&state).into_iter().collect();
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
    use std::os::unix::fs::PermissionsExt as _;

    use crate::client::Client;
    use crate::protocol::check_device_name;

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
        handle.push_secret_notice("the server kept its value of studio".to_owned());
        handle.push_secret_notice("the server kept its value of shop".to_owned());
        assert_eq!(handle.take_secret_notices().len(), 2);
        assert!(
            handle.take_secret_notices().is_empty(),
            "a taken line is not shown twice"
        );
    }

    /// The secrets of a computer whose key folder holds no value, for a handle
    /// that a test reads without a link.
    fn keys_of_a_test(keys: horizon_core::MachineKeys) -> Arc<dyn SecretTarget> {
        use horizon_core::named_secrets::NamedSecretVault;
        use horizon_core::secret::{CredentialVault, SecretVault};
        use horizon_core::secret_store::SecretStore;

        let credentials = Arc::new(CredentialVault::new(Box::new(
            SecretStore::in_memory().expect("a store in memory"),
        )));
        credentials.register(&horizon_browser::remote::RemoteBrowserConfig::default());
        let named = Arc::new(NamedSecretVault::new(Box::new(
            SecretStore::in_memory().expect("a store in memory"),
        )));
        Arc::new(SecretVault::new(keys, credentials, named))
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
}
