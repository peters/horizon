//! The computer side of a link: the local queue of unsent intents, the mark of the
//! log that this computer holds, and the mirror of the objects.
//!
//! A computer keeps its working state when the server is away. An intent goes to a
//! queue file with the counter that names it, written as a whole through a
//! temporary file and a rename, and it is sent again with the same identifier
//! until the server decides it. One push is in flight at a time. The mark is the
//! index and the tip of the log that the mirror holds, and it lives in the same
//! file as the queue, so the queue and the mark move together.
//!
//! Nothing here reads a clock of the computer: an intent carries no time, and the
//! server stamps every change with its own.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::error::{Refusal, SyncError};
use crate::frame::{read_frame, write_frame};
use crate::intent::{Intent, MAX_INTENTS_PER_PUSH, MAX_SECRETS_PER_PUSH, Op, PushReply};
use crate::protocol::{Frame, Hello, HelloOk, PROTOCOL_VERSION, Pull, Push, Wait, check_device_name};
use crate::store::{Changes, Row};
use crate::tip::tip;

/// The state of the link to the server, for the UI of a later slice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkState {
    /// This computer has no server.
    Off,
    /// The link is open and the hello is in flight.
    Connecting,
    /// The mirror is in step with the server at this index.
    InStep { index: u64 },
    /// The mirror is behind the server, so a pull is due.
    Behind,
    /// The server is away, and this many intents wait in the queue.
    Away { pending: usize },
}

impl LinkState {
    /// The stable code of this state, for a sidebar line, a log line and a tool
    /// answer.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Connecting => "connecting",
            Self::InStep { .. } => "in_step",
            Self::Behind => "behind",
            Self::Away { .. } => "away",
        }
    }

    /// The state of a link whose hello starts now. Only the first attempt of a
    /// link says `connecting`: an attempt after a link that failed keeps the state
    /// that the failure left, so a server that does not answer does not read
    /// `connecting` for as long as it stays silent.
    #[must_use]
    pub fn started(self, first_attempt: bool) -> Self {
        if first_attempt { Self::Connecting } else { self }
    }

    /// The intents that wait for the server, which is none for a link that
    /// answers.
    #[must_use]
    pub fn pending(&self) -> usize {
        match self {
            Self::Away { pending } => *pending,
            Self::Off | Self::Connecting | Self::InStep { .. } | Self::Behind => 0,
        }
    }
}

/// Where a computer keeps the objects that it pulled.
pub trait Mirror {
    /// Applies one row: the object is stored, or removed when the row says that it
    /// is gone.
    ///
    /// # Errors
    /// Returns why the row cannot be applied.
    fn apply(&mut self, row: &Row) -> Result<(), SyncError>;

    /// Replaces every object with `rows`, because the server is not the log that
    /// this computer held.
    ///
    /// # Errors
    /// Returns why the rows cannot be applied.
    fn replace(&mut self, rows: &[Row]) -> Result<(), SyncError>;
}

/// A mirror in memory, for a test or a caller that draws the objects itself.
#[derive(Debug, Default)]
pub struct ObjectMirror {
    rows: BTreeMap<(String, String), Row>,
}

impl ObjectMirror {
    /// The live object of that kind and id.
    #[must_use]
    pub fn get(&self, kind: &str, id: &str) -> Option<&Row> {
        self.rows
            .get(&(kind.to_owned(), id.to_owned()))
            .filter(|row| !row.deleted)
    }

    /// The live rows, in the order of their kind and id.
    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.rows.values().filter(|row| !row.deleted)
    }

    /// The number of live objects.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows().count()
    }

    /// Whether the mirror holds no live object.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Mirror for ObjectMirror {
    fn apply(&mut self, row: &Row) -> Result<(), SyncError> {
        let key = (row.kind.clone(), row.id.clone());
        if row.deleted {
            self.rows.remove(&key);
        } else {
            self.rows.insert(key, row.clone());
        }
        Ok(())
    }

    fn replace(&mut self, rows: &[Row]) -> Result<(), SyncError> {
        self.rows.clear();
        for row in rows.iter().filter(|row| !row.deleted) {
            self.rows.insert((row.kind.clone(), row.id.clone()), row.clone());
        }
        Ok(())
    }
}

/// What one call of [`Client::push_pending`] settled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PushOutcome {
    /// The intents that the server applied.
    pub applied: Vec<u64>,
    /// The intents that the server refused, with the reason.
    pub refused: Vec<(u64, Refusal)>,
}

/// What one pull or wait brought.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PullOutcome {
    /// The index of the server after the rows.
    pub index: u64,
    /// Whether the mirror was replaced instead of updated.
    pub reset: bool,
    /// The number of rows that the mirror applied.
    pub rows: usize,
}

/// The computer side of a link: the queue, the mark and the state for the UI.
#[derive(Debug)]
pub struct Client {
    file: PathBuf,
    state: ClientState,
    link: LinkState,
    /// Whether this link already started an attempt. The first attempt of a link
    /// says that it connects; an attempt after one that failed keeps the state
    /// that the failure left.
    attempted: bool,
    /// The mark that the next request must use instead of the index of this
    /// computer, because a refusal of a secret named a version that the mirror
    /// already passed. The answer to that request brings the row again, and then
    /// the index of this computer is the mark once more.
    replay: Option<u64>,
}

impl Client {
    /// Opens the queue file of `device`, creating it when it is not there.
    ///
    /// # Errors
    /// Refuses a device name that breaks the rule, and returns why the queue file
    /// cannot be read or written.
    pub fn open(path: PathBuf, device: &str) -> Result<Self, SyncError> {
        if !check_device_name(device) {
            return Err(SyncError::Device(device.to_owned()));
        }
        let state = if let Some(state) = read_state(&path)? {
            if state.device != device {
                return Err(SyncError::Queue(format!(
                    "the queue file at {} belongs to the computer {:?}, not {device:?}",
                    path.display(),
                    state.device
                )));
            }
            state
        } else {
            let state = ClientState {
                device: device.to_owned(),
                ..ClientState::default()
            };
            write_state(&path, &state)?;
            state
        };
        Ok(Self {
            file: path,
            state,
            link: LinkState::Off,
            attempted: false,
            replay: None,
        })
    }

    /// The computer that this client is.
    #[must_use]
    pub fn device(&self) -> &str {
        &self.state.device
    }

    /// The state of the link, for the UI.
    #[must_use]
    pub fn state(&self) -> &LinkState {
        &self.link
    }

    /// The index of the log that this computer holds.
    #[must_use]
    pub fn index(&self) -> u64 {
        self.state.index
    }

    /// The tip of the log that this computer holds.
    #[must_use]
    pub fn mark(&self) -> &str {
        &self.state.tip
    }

    /// Makes the next request read from before `version`, so its answer brings
    /// the row of that version again.
    ///
    /// A refusal of a secret names the version that the server decided, and the
    /// value of that version is what the person must see beside the local one: an
    /// index that is already past the version would hide it, and a later answer
    /// that carries no such row would leave the question open. A version that the
    /// index of this computer covers anyway changes nothing.
    pub fn replay_before(&mut self, version: u64) {
        let Some(target) = version.checked_sub(1) else {
            return;
        };
        if target < self.state.index {
            self.replay = Some(target);
        }
    }

    /// The index that the next request reads from: the index of this computer, or
    /// the mark that a refused secret set back.
    #[must_use]
    pub fn read_from(&self) -> u64 {
        self.replay.unwrap_or(self.state.index)
    }

    /// Ends the mark that a refused secret set back, because the answer of a
    /// request that read from `from` arrived: the row of that version came with
    /// it, or the server holds no such row.
    pub fn accept_replay(&mut self, from: u64) {
        if self.replay == Some(from) {
            self.replay = None;
        }
    }

    /// The intents that are not settled yet, in the order of their counters.
    #[must_use]
    pub fn pending(&self) -> &[Intent] {
        &self.state.pending
    }

    /// Queues the value of the secret `id` for the server. The queue file holds
    /// the id and the version that this computer saw, never the value: the
    /// supervisor reads the value from the key folder or the credential store when
    /// it pushes.
    ///
    /// # Errors
    /// Returns why the queue file cannot be written.
    pub fn enqueue_secret(&mut self, id: &str) -> Result<u64, SyncError> {
        let seen = self.state.secret_versions.get(id).copied();
        self.enqueue(
            seen,
            Op::SecretPut {
                id: id.to_owned(),
                body: None,
            },
        )
    }

    /// Queues the removal of the secret `id`.
    ///
    /// # Errors
    /// Returns why the queue file cannot be written.
    pub fn enqueue_secret_remove(&mut self, id: &str) -> Result<u64, SyncError> {
        self.enqueue(None, Op::SecretRemove { id: id.to_owned() })
    }

    /// Queues the key that `machine` uses, or the removal of that binding when
    /// `key` is `None`.
    ///
    /// # Errors
    /// Returns why the queue file cannot be written.
    pub fn enqueue_binding(&mut self, machine: &str, key: Option<&str>) -> Result<u64, SyncError> {
        self.enqueue(
            None,
            Op::BindingPut {
                machine: machine.to_owned(),
                key: key.map(str::to_owned),
            },
        )
    }

    /// The version of the secret `id` that this computer saw, when it saw one.
    #[must_use]
    pub fn secret_version(&self, id: &str) -> Option<u64> {
        self.state.secret_versions.get(id).copied()
    }

    /// Remembers the version of one secret of the server, so the next local
    /// change of it is an intent on that version.
    ///
    /// # Errors
    /// Returns why the queue file cannot be written.
    pub fn remember_secret(&mut self, id: &str, version: u64) -> Result<(), SyncError> {
        if self.state.secret_versions.insert(id.to_owned(), version) == Some(version) {
            return Ok(());
        }
        self.save()
    }

    /// Forgets the version of one secret, because the server removed it.
    ///
    /// # Errors
    /// Returns why the queue file cannot be written.
    pub fn forget_secret(&mut self, id: &str) -> Result<(), SyncError> {
        if self.state.secret_versions.remove(id).is_none() {
            return Ok(());
        }
        self.save()
    }

    /// Forgets every version of the secrets, because the server cannot read what
    /// it holds and every value that this computer has is new to it.
    ///
    /// # Errors
    /// Returns why the queue file cannot be written.
    pub fn forget_secrets(&mut self) -> Result<(), SyncError> {
        if self.state.secret_versions.is_empty() {
            return Ok(());
        }
        self.state.secret_versions.clear();
        self.save()
    }

    /// Whether an intent on the secret `id` waits for the server.
    #[must_use]
    pub fn secret_is_pending(&self, id: &str) -> bool {
        self.state
            .pending
            .iter()
            .any(|intent| intent.op.is_secret() && intent.op.object_id() == id)
    }

    /// Drops the intents of these counters, which this computer could not send
    /// and will not send: a secret whose local value is gone.
    ///
    /// # Errors
    /// Returns why the queue file cannot be written.
    pub fn forget_intents(&mut self, ids: &[u64]) -> Result<(), SyncError> {
        if ids.is_empty() {
            return Ok(());
        }
        self.state.pending.retain(|intent| !ids.contains(&intent.id));
        self.save()
    }

    /// Queues one intent and answers its counter. The file holds it before this
    /// call returns.
    ///
    /// # Errors
    /// Returns why the queue file cannot be written.
    pub fn enqueue(&mut self, seen: Option<u64>, op: Op) -> Result<u64, SyncError> {
        let id = self.state.next_intent;
        self.state.next_intent = id.saturating_add(1);
        self.state.pending.push(Intent {
            device: self.state.device.clone(),
            id,
            seen,
            op,
        });
        self.save()?;
        self.queue_changed();
        Ok(id)
    }

    /// Sends the hello, and remembers the store of the server. A store that this
    /// computer did not see before makes the next pull a full reset.
    ///
    /// # Errors
    /// Returns why the link failed or the server refused the hello.
    pub fn hello<R: Read, W: Write>(&mut self, link: &mut Link<R, W>) -> Result<HelloOk, SyncError> {
        let frame = self.hello_frame();
        link.send(&frame)?;
        let answer = match link.receive("hello_ok") {
            Ok(Frame::HelloOk(answer)) => answer,
            Ok(frame) => {
                let error = SyncError::Unexpected {
                    answered: frame_name(&frame),
                    asked: "hello_ok",
                };
                self.away();
                return Err(error);
            }
            Err(error) => {
                self.away();
                return Err(error);
            }
        };
        self.accept_hello(&answer)?;
        Ok(answer)
    }

    /// The hello of this computer, for a caller that owns the two halves of the
    /// link itself, such as the supervisor of a child process.
    pub fn hello_frame(&mut self) -> Frame {
        self.link = std::mem::replace(&mut self.link, LinkState::Off).started(!self.attempted);
        self.attempted = true;
        Frame::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device: self.state.device.clone(),
            store: self.state.store.clone(),
        })
    }

    /// Remembers the store that the server answered. A store that this computer
    /// did not see before makes the next pull a full reset.
    ///
    /// # Errors
    /// Returns why the queue file cannot be written.
    pub fn accept_hello(&mut self, answer: &HelloOk) -> Result<(), SyncError> {
        if self.state.store.as_deref() != Some(answer.store.as_str()) {
            self.state.store = Some(answer.store.clone());
            self.state.index = 0;
            self.state.tip.clear();
            self.save()?;
        }
        self.link = LinkState::Behind;
        Ok(())
    }

    /// Sends the queued intents in order, with the identifiers that they already
    /// have, until the queue is empty or the link fails.
    ///
    /// # Errors
    /// Returns why the link failed; the queue still holds every intent that the
    /// server did not decide.
    pub fn push_pending<R: Read, W: Write>(&mut self, link: &mut Link<R, W>) -> Result<PushOutcome, SyncError> {
        let mut outcome = PushOutcome::default();
        while let Some(chunk) = self.next_push() {
            let reply = match link.push(&chunk) {
                Ok(reply) => reply,
                Err(error) => {
                    self.away();
                    return Err(error);
                }
            };
            let settled = self.accept_push(&reply)?;
            outcome.applied.extend(settled.applied);
            outcome.refused.extend(settled.refused);
        }
        Ok(outcome)
    }

    /// The intents of the next push, or `None` when every intent is decided.
    ///
    /// One push holds at most [`MAX_INTENTS_PER_PUSH`] intents and at most
    /// [`MAX_SECRETS_PER_PUSH`] secrets, because a secret body is the largest
    /// value of the protocol and the frame has its own bound. The order of the
    /// queue is kept, so a push never skips an intent that waits.
    #[must_use]
    pub fn next_push(&self) -> Option<Vec<Intent>> {
        let mut chunk = Vec::new();
        let mut secrets = 0;
        for intent in &self.state.pending {
            if chunk.len() >= MAX_INTENTS_PER_PUSH {
                break;
            }
            if intent.op.is_secret() {
                if secrets >= MAX_SECRETS_PER_PUSH {
                    break;
                }
                secrets += 1;
            }
            chunk.push(intent.clone());
        }
        (!chunk.is_empty()).then_some(chunk)
    }

    /// Removes the intents that one answer decided from the queue and writes the
    /// queue file.
    ///
    /// # Errors
    /// Returns why the queue file cannot be written.
    pub fn accept_push(&mut self, reply: &PushReply) -> Result<PushOutcome, SyncError> {
        let mut outcome = PushOutcome::default();
        self.settle(reply, &mut outcome)?;
        if !outcome.applied.is_empty() {
            self.link = LinkState::Behind;
        }
        Ok(outcome)
    }

    /// Pulls the changes after the index that this computer holds and applies them
    /// to `mirror`. A pull that answers `reset` replaces the mirror.
    ///
    /// # Errors
    /// Returns why the link failed or the mirror refused a row.
    pub fn pull<R: Read, W: Write, M: Mirror>(
        &mut self,
        link: &mut Link<R, W>,
        mirror: &mut M,
    ) -> Result<PullOutcome, SyncError> {
        let frame = self.pull_frame();
        let changes = match link.ask(&frame) {
            Ok(changes) => changes,
            Err(error) => {
                self.away();
                return Err(error);
            }
        };
        self.apply(&changes, mirror)
    }

    /// The pull frame of this computer: the index and the tip that it holds.
    #[must_use]
    pub fn pull_frame(&self) -> Frame {
        Frame::Pull(Pull {
            since: self.read_from(),
            tip: self.state.tip.clone(),
        })
    }

    /// The wait frame of this computer, which the server answers at its bound or
    /// at the next change.
    #[must_use]
    pub fn wait_frame(&self) -> Frame {
        Frame::Wait(Wait {
            since: self.read_from(),
        })
    }

    /// Waits for the next change of the server and applies it. The server answers
    /// at its bound when nothing changes.
    ///
    /// # Errors
    /// Returns why the link failed or the mirror refused a row.
    pub fn wait<R: Read, W: Write, M: Mirror>(
        &mut self,
        link: &mut Link<R, W>,
        mirror: &mut M,
    ) -> Result<PullOutcome, SyncError> {
        let frame = self.wait_frame();
        let changes = match link.ask(&frame) {
            Ok(changes) => changes,
            Err(error) => {
                self.away();
                return Err(error);
            }
        };
        self.apply(&changes, mirror)
    }

    /// Moves the mark of this computer to `index`, which the server answered,
    /// and writes the queue file.
    ///
    /// # Errors
    /// Returns why the queue file cannot be written.
    pub fn accept_mark(&mut self, index: u64) -> Result<(), SyncError> {
        self.state.index = index;
        self.state.tip = match self.state.store.as_deref() {
            Some(store) => tip(store, index),
            None => String::new(),
        };
        self.save()?;
        self.link = LinkState::InStep { index };
        Ok(())
    }

    /// Reports the queue of a link that is away again, so a change that waits
    /// for the server is counted at once instead of at the next attempt.
    pub fn queue_changed(&mut self) {
        if matches!(self.link, LinkState::Away { .. }) {
            self.link = LinkState::Away {
                pending: self.state.pending.len(),
            };
        }
    }

    /// Marks the link as away, with the queue that waits for it.
    pub fn away(&mut self) {
        self.link = LinkState::Away {
            pending: self.state.pending.len(),
        };
    }

    /// Marks the link as off, for a computer that has no server again.
    pub fn off(&mut self) {
        self.link = LinkState::Off;
    }

    /// Removes the results that the server decided from the queue and writes the
    /// queue file.
    fn settle(&mut self, reply: &PushReply, outcome: &mut PushOutcome) -> Result<(), SyncError> {
        for result in &reply.results {
            match &result.outcome {
                crate::intent::Outcome::Applied { .. } => outcome.applied.push(result.id),
                crate::intent::Outcome::Refused(refusal) => outcome.refused.push((result.id, refusal.clone())),
            }
        }
        self.state
            .pending
            .retain(|intent| !reply.results.iter().any(|result| result.id == intent.id));
        self.save()
    }

    /// Applies the rows of one answer and moves the mark.
    fn apply<M: Mirror>(&mut self, changes: &Changes, mirror: &mut M) -> Result<PullOutcome, SyncError> {
        if changes.reset {
            mirror.replace(&changes.rows)?;
        } else {
            for row in &changes.rows {
                mirror.apply(row)?;
            }
        }
        self.accept_mark(changes.index)?;
        Ok(PullOutcome {
            index: changes.index,
            reset: changes.reset,
            rows: changes.rows.len(),
        })
    }

    fn save(&self) -> Result<(), SyncError> {
        write_state(&self.file, &self.state)
    }
}

/// The computer side of the frames, over any pair of byte streams.
#[derive(Debug)]
pub struct Link<R: Read, W: Write> {
    reader: R,
    writer: W,
}

impl<R: Read, W: Write> Link<R, W> {
    /// A link over one pair of byte streams, such as two in-memory sockets or the
    /// stdin and the stdout of the `ssh` child of a later slice.
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }

    /// Sends the hello of `device` and reads the answer of the server.
    ///
    /// # Errors
    /// Returns why the link failed or the server refused the hello.
    pub fn hello(&mut self, device: &str, store: Option<&str>) -> Result<HelloOk, SyncError> {
        self.send(&Frame::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device: device.to_owned(),
            store: store.map(str::to_owned),
        }))?;
        match self.receive("hello_ok")? {
            Frame::HelloOk(answer) => Ok(answer),
            frame => Err(SyncError::Unexpected {
                answered: frame_name(&frame),
                asked: "hello_ok",
            }),
        }
    }

    /// Sends intents and reads their results.
    ///
    /// # Errors
    /// Returns why the link failed or the server refused the push.
    pub fn push(&mut self, intents: &[Intent]) -> Result<PushReply, SyncError> {
        self.send(&Frame::Push(Push {
            intents: intents.to_vec(),
        }))?;
        match self.receive("pushed")? {
            Frame::Pushed(reply) => Ok(reply),
            frame => Err(SyncError::Unexpected {
                answered: frame_name(&frame),
                asked: "pushed",
            }),
        }
    }

    /// Sends a pull from `since` with the tip of this computer.
    ///
    /// # Errors
    /// Returns why the link failed or the server refused the pull.
    pub fn pull(&mut self, since: u64, tip: &str) -> Result<Changes, SyncError> {
        self.send(&Frame::Pull(Pull {
            since,
            tip: tip.to_owned(),
        }))?;
        self.changes()
    }

    /// Sends a wait from `since`, and reads the changes when the server answers.
    ///
    /// # Errors
    /// Returns why the link failed or the server refused the wait.
    pub fn wait(&mut self, since: u64) -> Result<Changes, SyncError> {
        self.send(&Frame::Wait(Wait { since }))?;
        self.changes()
    }

    /// Sends a pull or a wait frame and reads the changes that answer it.
    ///
    /// # Errors
    /// Returns why the link failed or the server refused the frame.
    pub fn ask(&mut self, frame: &Frame) -> Result<Changes, SyncError> {
        self.send(frame)?;
        self.changes()
    }

    fn changes(&mut self) -> Result<Changes, SyncError> {
        match self.receive("changes")? {
            Frame::Changes(changes) => Ok(changes),
            frame => Err(SyncError::Unexpected {
                answered: frame_name(&frame),
                asked: "changes",
            }),
        }
    }

    /// Sends one frame.
    ///
    /// # Errors
    /// Returns why the frame cannot be written.
    pub fn send(&mut self, frame: &Frame) -> Result<(), SyncError> {
        write_frame(&mut self.writer, frame).map_err(SyncError::Frame)
    }

    /// Reads one frame, and a typed error frame as the refusal that it is.
    ///
    /// # Errors
    /// Returns why the link failed, why the server refused the frame, or why it
    /// ended before the answer.
    pub fn receive(&mut self, asked: &'static str) -> Result<Frame, SyncError> {
        match read_frame(&mut self.reader).map_err(SyncError::Frame)? {
            Some(Frame::Error(error)) => Err(SyncError::Refused {
                code: error.code.as_str(),
                message: error.message,
            }),
            Some(frame) => Ok(frame),
            None => Err(SyncError::Ended { asked }),
        }
    }
}

/// The queue file of one computer: its device, its counter, its mark and the
/// intents that the server has not decided.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct ClientState {
    device: String,
    next_intent: u64,
    index: u64,
    tip: String,
    store: Option<String>,
    pending: Vec<Intent>,
    /// The version of each secret that this computer saw, by id. It holds no
    /// value, so the queue file never holds the body of a secret.
    secret_versions: BTreeMap<String, u64>,
}

impl Default for ClientState {
    fn default() -> Self {
        Self {
            device: String::new(),
            next_intent: 1,
            index: 0,
            tip: String::new(),
            store: None,
            pending: Vec::new(),
            secret_versions: BTreeMap::new(),
        }
    }
}

fn read_state(path: &Path) -> Result<Option<ClientState>, SyncError> {
    let file = horizon_core::AtomicFile::new(path.to_path_buf());
    let Some(text) = file.read().map_err(|error| queue_error(path, &error))? else {
        return Ok(None);
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| SyncError::Queue(format!("the queue file at {}: {error}", path.display())))
}

fn write_state(path: &Path, state: &ClientState) -> Result<(), SyncError> {
    let encoded = serde_json::to_vec_pretty(state)
        .map_err(|error| SyncError::Queue(format!("the queue of this computer: {error}")))?;
    horizon_core::AtomicFile::new(path.to_path_buf())
        .write(&encoded)
        .map_err(|error| queue_error(path, &error))
}

fn queue_error(path: &Path, error: &std::io::Error) -> SyncError {
    SyncError::Queue(format!("the queue file at {}: {error}", path.display()))
}

fn frame_name(frame: &Frame) -> &'static str {
    match frame {
        Frame::Hello(_) => "hello",
        Frame::HelloOk(_) => "hello_ok",
        Frame::Push(_) => "push",
        Frame::Pushed(_) => "pushed",
        Frame::Pull(_) => "pull",
        Frame::Changes(_) => "changes",
        Frame::Wait(_) => "wait",
        Frame::Error(_) => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::{Client, Frame, HelloOk, LinkState, PROTOCOL_VERSION};

    fn client() -> (tempfile::TempDir, Client) {
        let directory = tempfile::tempdir().expect("a directory");
        let client = Client::open(directory.path().join("server-queue.json"), "mbp-7c1").expect("a queue");
        (directory, client)
    }

    #[test]
    fn only_the_first_attempt_of_a_link_says_that_it_connects() {
        let (_directory, mut client) = client();
        // A start leaves the queue of an away server, as the supervisor does.
        client.away();
        assert!(matches!(client.hello_frame(), Frame::Hello(_)));
        assert_eq!(client.state(), &LinkState::Connecting, "the first attempt connects");

        // The attempt that failed leaves `away`, and the next attempt keeps it
        // until a hello answers: a server that does not answer does not read
        // `connecting` for as long as it stays silent.
        client.away();
        let _ = client.hello_frame();
        assert_eq!(client.state(), &LinkState::Away { pending: 0 });

        client
            .accept_hello(&HelloOk {
                protocol: PROTOCOL_VERSION,
                store: "6f1c".to_owned(),
                now_millis: 1_760_000_000_000,
                secrets_lost: false,
            })
            .expect("the hello");
        assert_eq!(client.state(), &LinkState::Behind, "a hello that answers moves on");
    }
}
