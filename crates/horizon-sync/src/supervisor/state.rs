//! The state of one link, its snapshot and the lock that a worker holds.
//!
//! A frame of the window reads the snapshot and never takes the lock of the
//! state: a worker writes the queue and the mirror of this computer, and the
//! drop of the lock runs those writes after it releases the lock.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        mpsc::{self, Sender},
    },
    time::Instant,
};

use horizon_core::machines::MachineBindings;
use horizon_core::secret::{SecretId, SecretKind};
use horizon_core::ui_thread;
use horizon_core::{Machine, MachineError, SecretTarget};

use super::SupervisorConfig;
use super::files::{MirrorFile, MirroredMachine, read_bindings, read_mirror, seed_records, write_mirror};
use super::pull::{merged_bindings, mirror_of};
use super::secrets::local_machine_keys;
use super::view::records_of;
use super::{SyncError, queue_error};
use crate::client::{Client, LinkState};
use crate::intent::Op;
use crate::protocol::{DeviceRow, Invited};

/// One question that the window or a worker asks the server.
///
/// The supervisor sends it on the open link, so a computer that reaches its
/// server over `ssh` can ask the same questions as one that joined it with a
/// code. The answer comes back on a channel that the caller polls, so no frame
/// waits for the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerRequest {
    /// One invitation for another computer.
    Invite,
    /// The list of the computers of this server.
    Devices,
    /// The removal of one computer.
    Remove(String),
}

/// The answer to one [`ServerRequest`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerReply {
    /// One invitation, with its code, its link and the time that it still lives.
    Invited(Invited),
    /// The computers of this server, and the time of the server that answered.
    Devices {
        /// The rows, in name order.
        rows: Vec<DeviceRow>,
        /// The time of the server when it answered.
        now_millis: u64,
    },
    /// The removal of one computer: whether the server held it.
    Removed {
        /// The name of the computer.
        device: String,
        /// Whether the server held that computer.
        removed: bool,
    },
    /// The question did not reach its answer, with one sentence for the person.
    Failed(String),
}

/// What a frame of the window reads about the link: the state, the names that
/// wait for the server, the records that only the server gave, the key names of
/// this computer and the generation of the records.
///
/// The supervisor thread publishes this value, and a frame reads the one that
/// stands now without taking the lock of the state. That lock covers the writes
/// of the queue and the mirror, so a frame that took it would wait for a disk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkSnapshot {
    state: LinkState,
    device: String,
    pending: BTreeSet<String>,
    foreign: BTreeSet<String>,
    machine_keys: BTreeSet<String>,
    generation: u64,
}

impl LinkSnapshot {
    /// The state of the link.
    #[must_use]
    pub fn state(&self) -> &LinkState {
        &self.state
    }

    /// The name that this computer has on the server, which the join of another
    /// server defaults to.
    #[must_use]
    pub fn device(&self) -> &str {
        &self.device
    }

    /// The names of the machines that have an intent the server did not decide.
    #[must_use]
    pub fn pending(&self) -> &BTreeSet<String> {
        &self.pending
    }

    /// Whether this computer holds no key binding for `name` because the record
    /// arrived from the server alone.
    #[must_use]
    pub fn is_foreign(&self, name: &str) -> bool {
        self.foreign.contains(name)
    }

    /// Whether this computer holds a machine key with that name now.
    #[must_use]
    pub fn has_machine_key(&self, name: &str) -> bool {
        self.machine_keys.contains(name)
    }

    /// The number of changes of the records, so a UI reads them again only when
    /// one of them changed.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// What one drop of the lock of the state must write: the files of the change,
/// built under the lock so their writes run after it.
#[derive(Default)]
pub(crate) struct Pending {
    /// The queue file of this computer, encoded under the lock.
    queue: Option<(PathBuf, Vec<u8>)>,
    /// The mirror of the machine list, built under the lock.
    mirror: Option<(PathBuf, MirrorFile)>,
    /// The number of the write of the queue, so a write that a later change
    /// replaced does not land.
    queue_version: u64,
    mirror_version: u64,
}

impl Pending {
    /// Whether a file waits for its write.
    const fn waits(&self) -> bool {
        self.queue.is_some() || self.mirror.is_some()
    }
}

/// The writes of the queue and the mirror, and the newest version of each one
/// that reached the disk.
///
/// It serializes the writes that run outside the lock of the state. It covers no read of the state, and a frame never
/// takes it: the workers of the link write the files of the person.
#[derive(Default)]
pub(crate) struct WriteOrder {
    next: u64,
    written_queue: u64,
    written_mirror: u64,
}

/// The state behind the handle.
pub(crate) struct SyncState {
    /// The queue of the link, exposed to the other modules of the supervisor.
    pub(super) client: Client,
    /// The queue file of this computer.
    pub(crate) client_path: PathBuf,
    pub(super) mirror_file: PathBuf,
    pub(super) mirror: BTreeMap<String, MirroredMachine>,
    pub(super) owned: BTreeSet<String>,
    /// The key that each machine uses, as the server gave it.
    pub(crate) bindings: BTreeMap<String, String>,
    /// The bindings that this computer kept before its first link.
    pub(crate) local_bindings: BTreeMap<String, String>,
    /// The machine keys that this computer holds now, so the machine section
    /// knows whether a machine can connect here yet.
    pub(crate) machine_keys: BTreeSet<String>,
    /// The places where this computer keeps its secrets.
    pub(crate) secrets: Arc<dyn SecretTarget>,
    /// The records of the file store of this computer, until the first pull of
    /// the server tells which of them the server does not have.
    pub(crate) pending_seed: Option<Vec<Machine>>,
    pub(crate) seeded: bool,
    /// The records that the last load answered, so a save turns only what its
    /// caller changed into an intent.
    pub(crate) view: BTreeMap<String, Machine>,
    /// The number of changes of the records, for a UI that reads them again.
    pub(crate) generation: u64,
    /// The mirror of the machine list does not hold the records of the state yet.
    pub(crate) mirror_dirty: bool,
    /// The secrets that the server refused and whose value no answer of changes
    /// covered yet. The value of the server that follows decides whether the local
    /// value differs at all.
    pub(crate) refused_secrets: BTreeSet<String>,
    /// When the request that the link has sent and not had answered left. The
    /// link times the bound of that request from this instant, so a reader that
    /// measures the same bound measures it from the same instant.
    pub(crate) request_since: Option<Instant>,
    /// The instant of the request that the link that ended last was waiting for,
    /// published with the `Away` state of that link. A reader that measures the
    /// bound of a lost link reads the instant that the link itself timed, and not
    /// the instant of a later attempt.
    pub(crate) lost_request_since: Option<Instant>,
}

impl SyncState {
    /// What a frame reads about this state now.
    fn snapshot(&self) -> LinkSnapshot {
        let pending: BTreeSet<String> = self
            .client
            .pending()
            .iter()
            .map(|intent| intent.op.object_id().to_owned())
            .collect();
        let foreign = self
            .mirror
            .iter()
            .filter(|(name, machine)| {
                !machine.mine
                    && !self.client.pending().iter().any(|intent| {
                        intent.op.object_id() == name.as_str() && matches!(intent.op, Op::MachinePut { .. })
                    })
            })
            .map(|(name, _)| name.clone())
            .collect();
        LinkSnapshot {
            state: self.client.state().clone(),
            device: self.client.device().to_owned(),
            pending,
            foreign,
            machine_keys: self.machine_keys.clone(),
            generation: self.generation,
        }
    }

    /// The files that a change of this state writes, built under the lock so the
    /// caller writes them after it drops the lock.
    fn pending_writes(&mut self) -> Pending {
        let queue = self
            .client
            .take_queue_write()
            .map(|encoded| (self.client_path.clone(), encoded));
        let mirror = std::mem::take(&mut self.mirror_dirty).then(|| (self.mirror_file.clone(), mirror_of(self)));
        Pending {
            queue,
            mirror,
            ..Pending::default()
        }
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
    pub(crate) inner: Arc<Mutex<SyncState>>,
    /// The snapshot that a frame reads, behind a lock that no thread ever holds
    /// during a file or a process.
    pub(crate) view: Arc<Mutex<Arc<LinkSnapshot>>>,
    /// The lines for the person about a secret of this computer that the server
    /// decided differently. The frame takes them without the lock of the state.
    pub(crate) notices: Arc<Mutex<Vec<String>>>,
    /// The order of the writes of the queue and the mirror.
    pub(crate) writes: Arc<Mutex<WriteOrder>>,
    pub(crate) kick: Sender<()>,
    pub(crate) requests: Sender<(ServerRequest, Sender<ServerReply>)>,
    pub(crate) wake: Arc<dyn Fn() + Send + Sync>,
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

impl SyncHandle {
    /// Opens the queue and the mirror of one computer, with no supervisor yet.
    ///
    /// # Errors
    /// Returns why the device name breaks its rule, or why the queue or the
    /// mirror file cannot be read.
    pub(super) fn open(config: &SupervisorConfig, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, SyncError> {
        let mut client = Client::open(config.queue_file.clone(), &config.device)?;
        // The writes of the queue run after the lock of the state, so a frame
        // that reads the link never waits for that disk.
        client.defer_writes();
        let file = read_mirror(&config.mirror_file)?;
        let pending_seed = if file.seeded {
            None
        } else {
            Some(seed_records(&config.machines_dir)?)
        };
        let local_bindings = if file.seeded {
            BTreeMap::new()
        } else {
            read_bindings(&config.machines_dir)?
        };
        let machine_keys = config
            .secrets
            .local_ids(SecretKind::MachineKey)
            .into_iter()
            .filter_map(|id| SecretId::parse(&id).ok())
            .map(|id| id.name().to_owned())
            .collect();
        let (kick, _) = mpsc::channel();
        let (requests, _) = mpsc::channel();
        let handle = Self {
            inner: Arc::new(Mutex::new(SyncState {
                client,
                client_path: config.queue_file.clone(),
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
                bindings: file.bindings,
                local_bindings,
                machine_keys,
                secrets: Arc::clone(&config.secrets),
                pending_seed,
                seeded: file.seeded,
                view: BTreeMap::new(),
                generation: 0,
                mirror_dirty: true,
                refused_secrets: BTreeSet::new(),
                request_since: None,
                lost_request_since: None,
            })),
            view: Arc::new(Mutex::new(Arc::new(LinkSnapshot::default()))),
            notices: Arc::new(Mutex::new(Vec::new())),
            writes: Arc::new(Mutex::new(WriteOrder::default())),
            kick,
            requests,
            wake,
        };
        {
            // The first mirror of this computer, and the snapshot that a frame
            // reads until the first answer of the server.
            let mut state = handle.lock();
            state.mirror_dirty = true;
        }
        Ok(handle)
    }

    /// The snapshot of the link that a frame reads: no file, no process and no
    /// lock that a worker holds during either of them.
    #[must_use]
    pub fn snapshot(&self) -> Arc<LinkSnapshot> {
        Arc::clone(&self.view.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// The state of the link, for the sidebar and a tool answer.
    #[must_use]
    pub fn state(&self) -> LinkState {
        self.snapshot().state.clone()
    }

    /// When the request of the link that is away left, or `None` while this
    /// computer holds no link that is away.
    ///
    /// The link times the bound of that request from this instant and publishes it
    /// with the `Away` state, under one lock, so a test that measures the bound of a
    /// lost link measures the instant that the link timed. It never reads a request
    /// that has not left yet, and never one of a later attempt.
    #[must_use]
    pub fn lost_request_since(&self) -> Option<Instant> {
        let state = self.lock();
        if !matches!(state.client.state(), LinkState::Away { .. }) {
            return None;
        }
        state.lost_request_since
    }

    /// The number of changes of the records, so a caller reads them again only
    /// when one of them changed.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.snapshot().generation
    }

    /// The names of the machines that have an intent the server did not decide.
    #[must_use]
    pub fn pending(&self) -> BTreeSet<String> {
        self.snapshot().pending.clone()
    }

    /// The machine keys that this computer holds now, by name, so the machine
    /// section knows whether a machine can connect here yet.
    #[must_use]
    pub fn has_machine_key(&self, name: &str) -> bool {
        self.snapshot().has_machine_key(name)
    }

    /// The version of the secret `id` that this computer saw, when it saw one.
    /// A local change of that secret is an intent on that version.
    #[must_use]
    pub fn secret_version(&self, id: &str) -> Option<u64> {
        self.lock().client.secret_version(id)
    }

    /// The lines for the person about a secret that the server decided
    /// differently, oldest first.
    #[must_use]
    pub fn take_secret_notices(&self) -> Vec<String> {
        let mut notices = self.notices.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut *notices)
    }

    /// Adds one line for the person about a secret of this computer. The worker
    /// of the secrets calls it, and no frame waits for the lock it takes.
    pub(super) fn push_secret_notice(&self, line: String) {
        self.notices.lock().unwrap_or_else(PoisonError::into_inner).push(line);
    }

    /// Queues the value of the secret `id` for the server. The value itself stays
    /// where this computer keeps it, and the supervisor reads it when it pushes.
    ///
    /// # Errors
    /// Returns why the queue file cannot be written.
    pub fn queue_secret(&self, id: &SecretId) -> Result<(), MachineError> {
        ui_thread::blocking("the queue of the link is written and synced");
        let mut state = self.lock();
        state
            .client
            .enqueue_secret(id.as_str())
            .map_err(|error| queue_error(&error))?;
        state.generation = state.generation.wrapping_add(1);
        drop(state);
        self.notify();
        Ok(())
    }

    /// Queues the removal of the secret `id`.
    ///
    /// # Errors
    /// Returns why the queue file cannot be written.
    pub fn queue_secret_remove(&self, id: &SecretId) -> Result<(), MachineError> {
        ui_thread::blocking("the queue of the link is written and synced");
        let mut state = self.lock();
        state
            .client
            .enqueue_secret_remove(id.as_str())
            .map_err(|error| queue_error(&error))?;
        state.generation = state.generation.wrapping_add(1);
        drop(state);
        self.notify();
        Ok(())
    }

    /// Turns the key bindings of this computer into intents, so the key that a
    /// machine uses follows the person to the other computers, and writes the
    /// mirror.
    ///
    /// # Errors
    /// Returns why the queue file or the mirror cannot be written.
    pub fn save_bindings(&self, bindings: &MachineBindings) -> Result<(), MachineError> {
        ui_thread::blocking("the queue and the mirror of the link are written and synced");
        let mut state = self.lock();
        let next = bindings.names().clone();
        let previous = merged_bindings(&state);
        let mut asked = Vec::new();
        for (machine, key) in &next {
            if previous.get(machine) != Some(key) {
                asked.push((machine.clone(), Some(key.clone())));
            }
        }
        for machine in previous.keys() {
            if !next.contains_key(machine) {
                asked.push((machine.clone(), None));
            }
        }
        let mut written = false;
        for (machine, key) in asked {
            state
                .client
                .enqueue_binding(&machine, key.as_deref())
                .map_err(|error| queue_error(&error))?;
            written = true;
        }
        if written {
            state.generation = state.generation.wrapping_add(1);
            // The drop of the lock writes the mirror and the queue.
            state.mirror_dirty = true;
        }
        drop(state);
        self.notify();
        Ok(())
    }

    /// Reads the machine keys of this computer again, after a value of the server
    /// arrived or a local key changed.
    pub(super) fn refresh_machine_keys(&self) {
        let mut state = self.lock();
        state.machine_keys = local_machine_keys(&state.secrets);
    }

    /// Whether this computer holds no key binding for `name` because the record
    /// arrived from the server alone.
    #[must_use]
    pub fn is_foreign(&self, name: &str) -> bool {
        self.snapshot().is_foreign(name)
    }

    /// The records of this computer as the person sees them: the mirror of the
    /// server with the intents that wait for it applied, in name order.
    ///
    /// The answer is what the next [`Self::save`] compares against, so a change
    /// of the server between the two never becomes an intent of this computer.
    #[must_use]
    pub fn records(&self) -> Vec<Machine> {
        records_of(&mut self.lock())
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
        ui_thread::blocking("the queue and the mirror of the link are written and synced");
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
            // The drop of the lock writes the mirror and the queue.
            state.mirror_dirty = true;
        }
        drop(state);
        self.notify();
        Ok(())
    }

    /// Asks the server one question, and answers the channel of its answer.
    ///
    /// The question travels on the link that the supervisor already holds, so it
    /// reaches a server that this computer follows over `ssh` as well as one that
    /// it joined with a code. Nothing waits for the answer here: the caller polls
    /// the channel of the answer when its worker or its frame reads it.
    ///
    /// A supervisor that ended answers a channel that is closed, which a caller
    /// reads as "no answer".
    #[must_use]
    pub fn ask(&self, request: ServerRequest) -> mpsc::Receiver<ServerReply> {
        let (reply, answers) = mpsc::channel();
        if self.requests.send((request, reply)).is_err() {
            tracing::debug!("the link ended before a question reached it");
        }
        self.notify();
        answers
    }

    /// Asks the supervisor for a turn that sends what the queue holds, instead of
    /// waiting for the next round of the link.
    pub fn notify(&self) {
        // One unread kick is enough: a second local change before the next turn
        // of the supervisor finds the queue of the first one too.
        let _ = self.kick.send(());
        (self.wake)();
    }

    /// The state of the link, with the publish and the writes of its change on
    /// the drop of the guard.
    ///
    /// A worker writes the queue and the mirror of this computer, so the drop
    /// runs the file writes after it releases the lock: a frame that reads the
    /// link waits for no disk.
    #[track_caller]
    pub(super) fn lock(&self) -> SyncGuard<'_> {
        ui_thread::blocking("the lock of the link state is taken");
        SyncGuard {
            handle: self,
            state: Some(self.inner.lock().unwrap_or_else(PoisonError::into_inner)),
        }
    }

    /// The snapshot of `state`, in the place that a frame reads.
    pub(super) fn publish(&self, snapshot: LinkSnapshot) {
        let mut view = self.view.lock().unwrap_or_else(PoisonError::into_inner);
        *view = Arc::new(snapshot);
    }
}

/// The lock of the state of one link, released on the drop of this value.
pub(crate) struct SyncGuard<'a> {
    handle: &'a SyncHandle,
    state: Option<MutexGuard<'a, SyncState>>,
}

impl std::fmt::Debug for SyncGuard<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("SyncGuard").finish_non_exhaustive()
    }
}

impl std::ops::Deref for SyncGuard<'_> {
    type Target = SyncState;

    fn deref(&self) -> &SyncState {
        match self.state.as_deref() {
            Some(state) => state,
            None => unreachable!("a guard holds the state until its drop"),
        }
    }
}

impl std::ops::DerefMut for SyncGuard<'_> {
    fn deref_mut(&mut self) -> &mut SyncState {
        match self.state.as_deref_mut() {
            Some(state) => state,
            None => unreachable!("a guard holds the state until its drop"),
        }
    }
}

impl Drop for SyncGuard<'_> {
    fn drop(&mut self) {
        let Some(mut state) = self.state.take() else {
            return;
        };
        // The snapshot and the bytes of the files are built under the lock.
        self.handle.publish(state.snapshot());
        let mut pending = state.pending_writes();
        if pending.queue.is_some() || pending.mirror.is_some() {
            let mut order = self.handle.writes.lock().unwrap_or_else(PoisonError::into_inner);
            order.next = order.next.wrapping_add(1);
            if pending.queue.is_some() {
                pending.queue_version = order.next;
            }
            if pending.mirror.is_some() {
                pending.mirror_version = order.next;
            }
        }
        drop(state);
        if !pending.waits() {
            return;
        }
        let mut order = self.handle.writes.lock().unwrap_or_else(PoisonError::into_inner);
        if let (Some((path, encoded)), true) = (pending.queue.take(), pending.queue_version > order.written_queue) {
            order.written_queue = pending.queue_version;
            if let Err(error) = horizon_core::AtomicFile::new(path).write(&encoded) {
                tracing::warn!(%error, "the queue of the link was not written");
            }
        }
        if let (Some((path, file)), true) = (pending.mirror.take(), pending.mirror_version > order.written_mirror) {
            order.written_mirror = pending.mirror_version;
            if let Err(error) = write_mirror(&path, &file) {
                tracing::warn!(%error, "the mirror of the machine list was not written");
            }
        }
    }
}
