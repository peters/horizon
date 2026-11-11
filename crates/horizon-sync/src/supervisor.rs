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
//! Nothing here reads a clock of the computer for the store, and no frame of the
//! server makes this computer run a command: the only process is the `ssh` of the
//! entry, and the only value that reaches it is its validated target and command.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use horizon_browser_control::ssh_forward::ssh_command;
use horizon_core::machines::{LoadedMachines, MachineStore};
use horizon_core::{Machine, MachineError};

use crate::client::{Client, LinkState, PullOutcome};
use crate::error::SyncError;
use crate::frame::{read_frame, write_frame};
use crate::intent::{Op, PushReply};
use crate::protocol::{Frame, HelloOk};
use crate::store::Changes;
use files::{
    MIRROR_FILE, MIRROR_FORMAT, MirrorFile, MirroredMachine, QUEUE_FILE, device_name, read_mirror, seed_records,
    write_mirror,
};

mod files;

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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// The bounds of the requests of the link. A test replaces them so it sees a
    /// link that went silent within a second instead of a minute.
    pub bounds: LinkBounds,
}

impl SupervisorConfig {
    /// The settings of the supervisor of `target` and `command`, with the queue,
    /// the mirror and the device name of `directory`.
    ///
    /// # Errors
    /// Returns why the device name of this computer cannot be read or written.
    pub fn for_server(target: &str, command: &str, directory: &Path) -> Result<Self, SyncError> {
        Ok(Self {
            target: target.to_owned(),
            command: command.to_owned(),
            program: horizon_browser_control::ssh_forward::ssh_program(),
            device: device_name(directory)?,
            queue_file: directory.join(QUEUE_FILE),
            mirror_file: directory.join(MIRROR_FILE),
            machines_dir: directory.to_path_buf(),
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

/// The state of the machine list of one computer: the queue of intents, the
/// mirror that the server gave, and the state of the link for the UI.
///
/// The handle is cheap to clone and is shared by the supervisor thread and by the
/// store that the UI loads. Every method holds the state for a short read or
/// write and never waits for a process or a file of the server.
#[derive(Clone)]
pub struct SyncHandle {
    inner: Arc<Mutex<SyncState>>,
    kick: Sender<()>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl std::fmt::Debug for SyncHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.lock();
        formatter
            .debug_struct("SyncHandle")
            .field("device", &state.client.device())
            .field("link", &state.client.state())
            .field("machines", &state.mirror.len())
            .finish()
    }
}

/// The state behind the handle.
struct SyncState {
    client: Client,
    mirror_file: PathBuf,
    mirror: BTreeMap<String, MirroredMachine>,
    owned: BTreeSet<String>,
    /// The records of the file store of this computer, until the first pull of
    /// the server tells which of them the server does not have.
    pending_seed: Option<Vec<Machine>>,
    seeded: bool,
    /// The records that the last load answered, so a save turns only what its
    /// caller changed into an intent.
    view: BTreeMap<String, Machine>,
    /// The number of changes of the records, for a UI that reads them again.
    generation: u64,
}

impl SyncHandle {
    /// Opens the queue and the mirror of one computer, with no supervisor yet.
    ///
    /// # Errors
    /// Returns why the device name breaks its rule, or why the queue or the
    /// mirror file cannot be read.
    fn open(config: &SupervisorConfig, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, SyncError> {
        let client = Client::open(config.queue_file.clone(), &config.device)?;
        let file = read_mirror(&config.mirror_file)?;
        let pending_seed = if file.seeded {
            None
        } else {
            Some(seed_records(&config.machines_dir)?)
        };
        let (kick, _) = mpsc::channel();
        let handle = Self {
            inner: Arc::new(Mutex::new(SyncState {
                client,
                mirror_file: config.mirror_file.clone(),
                mirror: file
                    .machines
                    .into_iter()
                    .map(|record| {
                        let mine = file.owned.contains(&record.name);
                        (record.name.clone(), MirroredMachine { record, mine })
                    })
                    .collect(),
                owned: file.owned,
                pending_seed,
                seeded: file.seeded,
                view: BTreeMap::new(),
                generation: 0,
            })),
            kick,
            wake,
        };
        handle.save_mirror()?;
        Ok(handle)
    }

    /// The state of the link, for the sidebar and a tool answer.
    #[must_use]
    pub fn state(&self) -> LinkState {
        self.lock().client.state().clone()
    }

    /// The number of changes of the records, so a caller reads them again only
    /// when one of them changed.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// The names of the machines that have an intent the server did not decide.
    #[must_use]
    pub fn pending(&self) -> BTreeSet<String> {
        self.lock()
            .client
            .pending()
            .iter()
            .map(|intent| intent.op.object_id().to_owned())
            .collect()
    }

    /// Whether this computer holds no key binding for `name` because the record
    /// arrived from the server alone.
    #[must_use]
    pub fn is_foreign(&self, name: &str) -> bool {
        let state = self.lock();
        state.mirror.get(name).is_some_and(|machine| !machine.mine)
            && !state
                .client
                .pending()
                .iter()
                .any(|intent| intent.op.object_id() == name && matches!(intent.op, Op::MachinePut { .. }))
    }

    /// The records of this computer as the person sees them: the mirror of the
    /// server with the intents that wait for it applied, in name order.
    ///
    /// The answer is what the next [`Self::save`] compares against, so a change
    /// of the server between the two never becomes an intent of this computer.
    #[must_use]
    pub fn records(&self) -> Vec<Machine> {
        let mut state = self.lock();
        let merged = merged_records(&state);
        state.view = merged
            .iter()
            .map(|machine| (machine.name.clone(), machine.clone()))
            .collect();
        merged
    }

    /// Whether this computer compared the machine list that it kept before the
    /// server with the server.
    #[must_use]
    pub fn is_seeded(&self) -> bool {
        self.lock().seeded
    }

    /// Turns the difference between `records` and the last load into intents and
    /// writes the queue, so the sidebar of this computer changes at once and the
    /// server commits the change when the link is there.
    ///
    /// # Errors
    /// Returns why the queue file cannot be written.
    pub fn save(&self, records: &[Machine]) -> Result<(), MachineError> {
        let mut state = self.lock();
        let previous = state.view.clone();
        let next: BTreeMap<&str, &Machine> = records.iter().map(|machine| (machine.name.as_str(), machine)).collect();
        let mut asked = Vec::new();
        for (name, record) in &next {
            if previous.get(*name) != Some(*record) {
                asked.push((
                    (*name).to_owned(),
                    Op::MachinePut {
                        record: (*record).clone(),
                    },
                ));
            }
        }
        for name in previous.keys() {
            if !next.contains_key(name.as_str()) {
                asked.push((name.clone(), Op::MachineRemove { name: name.clone() }));
            }
        }
        let mut written = false;
        for (name, op) in asked {
            state.client.enqueue(None, op).map_err(|error| queue_error(&error))?;
            state.owned.insert(name);
            written = true;
        }
        state.view = next
            .into_iter()
            .map(|(name, record)| (name.to_owned(), record.clone()))
            .collect();
        if written {
            state.generation = state.generation.wrapping_add(1);
            let file = mirror_of(&state);
            write_mirror(&state.mirror_file, &file).map_err(|error| queue_error(&error))?;
        }
        drop(state);
        self.notify();
        Ok(())
    }

    /// Asks the supervisor for a turn that sends what the queue holds, instead of
    /// waiting for the next round of the link.
    pub fn notify(&self) {
        // One unread kick is enough: a second local change before the next turn
        // of the supervisor finds the queue of the first one too.
        let _ = self.kick.send(());
        (self.wake)();
    }

    fn lock(&self) -> MutexGuard<'_, SyncState> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes the mirror file of the state, with the records that it holds now.
    fn save_mirror(&self) -> Result<(), SyncError> {
        let state = self.lock();
        let file = mirror_of(&state);
        write_mirror(&state.mirror_file, &file)
    }
}

/// The records of the state with its waiting intents applied, in name order.
fn merged_records(state: &SyncState) -> Vec<Machine> {
    let mut merged: BTreeMap<String, Machine> = state
        .mirror
        .iter()
        .map(|(name, machine)| (name.clone(), machine.record.clone()))
        .collect();
    for intent in state.client.pending() {
        match &intent.op {
            Op::MachinePut { record } => {
                merged.insert(record.name.clone(), record.clone());
            }
            Op::MachineRemove { name } => {
                merged.remove(name);
            }
            Op::ObjectPut { .. } | Op::ObjectRemove { .. } => {}
        }
    }
    merged.into_values().collect()
}

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
            let frame = Frame::Push(crate::protocol::Push { intents: chunk });
            if let Err(error) = write_frame(input, &frame) {
                tracing::warn!(%error, "the link could not send a push");
                return;
            }
            sending = Some(Instant::now());
        }
        if waiting.is_none() {
            let frame = {
                let state = handle.lock();
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
                if let Err(error) = apply_changes(handle, &changes) {
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

/// Keeps one answer of a push in the queue and the mirror, and marks the records
/// as changed for the UI.
fn accept_pushed(handle: &SyncHandle, reply: &PushReply) {
    let outcome = {
        let mut state = handle.lock();
        state.client.accept_push(reply)
    };
    match outcome {
        Ok(outcome) => {
            for (id, refusal) in &outcome.refused {
                tracing::warn!(
                    id,
                    code = refusal.code(),
                    reason = %refusal,
                    "the server refused an intent"
                );
            }
        }
        Err(error) => {
            tracing::warn!(%error, "the answer of a push was not kept");
            return;
        }
    }
    let mut state = handle.lock();
    state.generation = state.generation.wrapping_add(1);
    let file = mirror_of(&state);
    if let Err(error) = write_mirror(&state.mirror_file, &file) {
        tracing::warn!(%error, "the mirror of the machine list was not written");
    }
    drop(state);
    (handle.wake)();
}

/// Applies one answer of changes to the mirror and moves the mark of this
/// computer.
fn apply_changes(handle: &SyncHandle, changes: &Changes) -> Result<PullOutcome, SyncError> {
    let mut state = handle.lock();
    let applied = apply_rows(&mut state, changes)?;
    seed_local_records(&mut state)?;
    state.client.accept_mark(changes.index)?;
    state.generation = state.generation.wrapping_add(1);
    let file = mirror_of(&state);
    write_mirror(&state.mirror_file, &file)?;
    tracing::debug!(
        index = applied.index,
        rows = applied.rows,
        reset = applied.reset,
        "the machine list moved"
    );
    Ok(applied)
}

/// Applies the rows of one answer to the mirror of the state.
fn apply_rows(state: &mut SyncState, changes: &Changes) -> Result<PullOutcome, SyncError> {
    if changes.reset {
        state.mirror.clear();
    }
    let mut applied = 0;
    for row in &changes.rows {
        let name = row.id.clone();
        if row.deleted {
            state.mirror.remove(&name);
            applied += 1;
            continue;
        }
        let record: Machine = serde_json::from_value(row.body.clone()).map_err(|error| {
            SyncError::Queue(format!(
                "the server sent a machine record that this Horizon cannot read: {error}"
            ))
        })?;
        let record = horizon_core::machines::check_record(&record).map_err(|error| {
            SyncError::Queue(format!(
                "the server sent a machine record that breaks its rule: {error}"
            ))
        })?;
        let mine = state.owned.contains(&name);
        state.mirror.insert(name, MirroredMachine { record, mine });
        applied += 1;
    }
    Ok(PullOutcome {
        index: changes.index,
        reset: changes.reset,
        rows: applied,
    })
}

/// Keeps the records that this computer had before the server, once the first
/// pull says which of them the server does not have.
fn seed_local_records(state: &mut SyncState) -> Result<(), SyncError> {
    let Some(records) = state.pending_seed.take() else {
        return Ok(());
    };
    for record in records {
        if state.mirror.contains_key(&record.name) {
            // A name that both have keeps the record of the server.
            continue;
        }
        let name = record.name.clone();
        state.client.enqueue(None, Op::MachinePut { record })?;
        state.owned.insert(name);
    }
    state.seeded = true;
    Ok(())
}

/// The mirror file of the state.
fn mirror_of(state: &SyncState) -> MirrorFile {
    MirrorFile {
        format: MIRROR_FORMAT,
        seeded: state.seeded,
        owned: state.owned.clone(),
        machines: merged_records(state),
    }
}

/// Remembers the hello of the server.
fn accept_hello(handle: &SyncHandle, answer: &HelloOk) -> Result<(), String> {
    let mut state = handle.lock();
    state
        .client
        .accept_hello(answer)
        .map_err(|error| format!("the hello was not kept: {error}"))?;
    state.generation = state.generation.wrapping_add(1);
    drop(state);
    (handle.wake)();
    Ok(())
}

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
        Ok(LoadedMachines {
            records: self.handle.records(),
            keys: Vec::new(),
        })
    }

    fn save(&self, records: &[Machine]) -> Result<(), MachineError> {
        self.handle.save(records)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

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
    fn a_device_file_of_another_rule_is_replaced() {
        let directory = tempfile::tempdir().expect("a directory");
        std::fs::write(directory.path().join(DEVICE_FILE), "Not A Device/Name\n").expect("a bad name");
        let name = device_name(directory.path()).expect("a name");
        assert!(check_device_name(&name), "{name}");
        assert!(name.starts_with("horizon-"), "{name}");
    }
}
