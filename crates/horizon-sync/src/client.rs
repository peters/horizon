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
use crate::intent::{Intent, MAX_INTENTS_PER_PUSH, Op, PushReply};
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

    /// The intents that are not settled yet, in the order of their counters.
    #[must_use]
    pub fn pending(&self) -> &[Intent] {
        &self.state.pending
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
        Ok(id)
    }

    /// Sends the hello, and remembers the store of the server. A store that this
    /// computer did not see before makes the next pull a full reset.
    ///
    /// # Errors
    /// Returns why the link failed or the server refused the hello.
    pub fn hello<R: Read, W: Write>(&mut self, link: &mut Link<R, W>) -> Result<HelloOk, SyncError> {
        self.link = LinkState::Connecting;
        let answer = match link.hello(&self.state.device, self.state.store.as_deref()) {
            Ok(answer) => answer,
            Err(error) => {
                self.away();
                return Err(error);
            }
        };
        if self.state.store.as_deref() != Some(answer.store.as_str()) {
            self.state.store = Some(answer.store.clone());
            self.state.index = 0;
            self.state.tip.clear();
            self.save()?;
        }
        self.link = LinkState::Behind;
        Ok(answer)
    }

    /// Sends the queued intents in order, with the identifiers that they already
    /// have, until the queue is empty or the link fails.
    ///
    /// # Errors
    /// Returns why the link failed; the queue still holds every intent that the
    /// server did not decide.
    pub fn push_pending<R: Read, W: Write>(&mut self, link: &mut Link<R, W>) -> Result<PushOutcome, SyncError> {
        let mut outcome = PushOutcome::default();
        while !self.state.pending.is_empty() {
            let chunk: Vec<Intent> = self.state.pending.iter().take(MAX_INTENTS_PER_PUSH).cloned().collect();
            let reply = match link.push(&chunk) {
                Ok(reply) => reply,
                Err(error) => {
                    self.away();
                    return Err(error);
                }
            };
            self.settle(&reply, &mut outcome)?;
            if !outcome.applied.is_empty() {
                self.link = LinkState::Behind;
            }
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
        let changes = match link.pull(self.state.index, &self.state.tip) {
            Ok(changes) => changes,
            Err(error) => {
                self.away();
                return Err(error);
            }
        };
        self.apply(&changes, mirror)
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
        let changes = match link.wait(self.state.index) {
            Ok(changes) => changes,
            Err(error) => {
                self.away();
                return Err(error);
            }
        };
        self.apply(&changes, mirror)
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
        self.state.index = changes.index;
        self.state.tip = match self.state.store.as_deref() {
            Some(store) => tip(store, changes.index),
            None => String::new(),
        };
        self.save()?;
        self.link = LinkState::InStep { index: changes.index };
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

    fn changes(&mut self) -> Result<Changes, SyncError> {
        match self.receive("changes")? {
            Frame::Changes(changes) => Ok(changes),
            frame => Err(SyncError::Unexpected {
                answered: frame_name(&frame),
                asked: "changes",
            }),
        }
    }

    fn send(&mut self, frame: &Frame) -> Result<(), SyncError> {
        write_frame(&mut self.writer, frame).map_err(SyncError::Frame)
    }

    fn receive(&mut self, asked: &'static str) -> Result<Frame, SyncError> {
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
