//! The server store: one SQLite file with one writer task and any number of
//! readers.
//!
//! A caller holds a [`Store`] handle and never sees the file. A write goes to the
//! writer task over a channel and answers after the commit; a read takes an idle
//! read connection of its own, so a long read never holds the writer. The handle
//! is cheap to clone: every link of every computer of one server holds one.

mod apply;
mod rows;
mod schema;
mod secrets;
mod writer;

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        mpsc::{self, Sender},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rusqlite::Connection;

use crate::error::StoreError;
use crate::intent::{Intent, PushReply};
use writer::Job;

pub use rows::{Changes, Row};

use secrets::SecretKey;

/// The most read connections that a store keeps for its next read.
const MAX_IDLE_READERS: usize = 8;

/// The state of the server store, shared with its readers and with the writer.
#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    /// The identifier that the server wrote when it made the file. A tip names it,
    /// so a computer that holds a log of another file learns to reset.
    store_id: String,
    /// The key that seals every secret body of this file. It lives in a file of
    /// the server home and never in the database, so a copy of the database alone
    /// holds no readable secret.
    key: Arc<SecretKey>,
    /// The channel to the writer, until the store is closed.
    writer: Mutex<Option<Sender<Job>>>,
    writer_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    readers: Readers,
    signal: Arc<Signal>,
}

impl Store {
    /// Opens the store at `path`, creating the file and its tables when they are
    /// not there, and starts its one writer task.
    ///
    /// # Errors
    /// Refuses a file from a newer Horizon, and returns why the file cannot be
    /// opened or written.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let mut connection = schema::open(path)?;
        let store_id = schema::prepare(&mut connection)?;
        let index = schema::index_of(&connection)?;
        // The key file is made here, at the first start of a server home, and a
        // file that a copy or a restore left readable is made private again.
        let key = Arc::new(SecretKey::beside(path)?);
        if secrets::lost(&connection, &key)? {
            tracing::warn!(
                key = %key.path().display(),
                "a secret of this server is sealed with another key, so this server cannot read it; every computer that still holds a secret sends it again"
            );
        }
        drop(connection);

        let signal = Arc::new(Signal::new(index));
        let (sender, jobs) = mpsc::channel();
        let writer_path = path.to_path_buf();
        let writer_signal = Arc::clone(&signal);
        let writer_key = Arc::clone(&key);
        let writer_thread = std::thread::Builder::new()
            .name("horizon-sync-writer".to_owned())
            .spawn(move || writer::run(&writer_path, &writer_signal, &writer_key, &jobs))
            .map_err(|error| StoreError::File(format!("the writer thread of the server store: {error}")))?;

        Ok(Self {
            inner: Arc::new(Inner {
                path: path.to_path_buf(),
                store_id,
                key,
                writer: Mutex::new(Some(sender)),
                writer_thread: Mutex::new(Some(writer_thread)),
                readers: Readers::new(path.to_path_buf()),
                signal,
            }),
        })
    }

    /// The identifier of this store, which one tip names.
    #[must_use]
    pub fn store_id(&self) -> &str {
        &self.inner.store_id
    }

    /// The file of this store.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// The last committed index, or 0 for a store that no change reached yet.
    #[must_use]
    pub fn index(&self) -> u64 {
        self.inner.signal.index()
    }

    /// The objects that changed after `since`.
    ///
    /// # Errors
    /// Returns why the objects cannot be read.
    pub fn changes(&self, since: u64) -> Result<Changes, StoreError> {
        let key = &self.inner.key;
        self.inner.read(|connection| rows::changes(connection, since, key))
    }

    /// Every object of the store, for a computer that must replace its mirror.
    ///
    /// # Errors
    /// Returns why the objects cannot be read.
    pub fn all(&self) -> Result<Changes, StoreError> {
        let key = &self.inner.key;
        self.inner.read(|connection| rows::all(connection, key))
    }

    /// Whether a live secret of this store is sealed with a key that this server
    /// does not have, so it cannot read it. The hello of a link says so, and every
    /// computer that still holds a secret then sends it again.
    #[must_use]
    pub fn secrets_lost(&self) -> bool {
        let key = &self.inner.key;
        self.inner
            .read(|connection| secrets::lost(connection, key))
            .unwrap_or(true)
    }

    /// The changes after `since`, when the tip of the computer names this log at
    /// that index. A computer that is ahead of this store, or that names another
    /// log, gets every row with `reset: true`, so its mirror equals this store
    /// afterwards.
    ///
    /// # Errors
    /// Returns why the objects cannot be read.
    pub fn pull(&self, since: u64, claimed: &str) -> Result<Changes, StoreError> {
        let key = &self.inner.key;
        self.inner.read(|connection| {
            let index = schema::index_of(connection)?;
            let same_log = since == 0 || claimed == crate::tip(&self.inner.store_id, since);
            if since > index || !same_log {
                return rows::all(connection, key);
            }
            rows::changes(connection, since, key)
        })
    }

    /// Waits for a change after `since`, or for `bound` to pass, and answers with
    /// what changed up to that moment.
    ///
    /// # Errors
    /// Returns why the objects cannot be read.
    pub fn wait(&self, since: u64, bound: Duration) -> Result<Changes, StoreError> {
        self.inner.signal.wait_after(since, Instant::now() + bound);
        self.changes(since)
    }

    /// Applies the intents of one push as one transaction, and answers with one
    /// result for each intent. The answer comes after the commit.
    ///
    /// # Errors
    /// Refuses a push above the intent limit of the protocol, and returns why the
    /// writer could not commit the push. A failed push writes nothing, so the
    /// caller can send the same intents again.
    pub fn push(&self, device: &str, intents: &[Intent]) -> Result<PushReply, StoreError> {
        if intents.len() > crate::intent::MAX_INTENTS_PER_PUSH {
            return Err(StoreError::TooManyIntents {
                limit: crate::intent::MAX_INTENTS_PER_PUSH,
            });
        }
        let (reply, answer) = mpsc::sync_channel(1);
        self.send(Job::Push {
            device: device.to_owned(),
            intents: intents.to_vec(),
            reply,
        })?;
        answer.recv().map_err(|_| StoreError::Stopped)?
    }

    /// Stops the writer after the changes that are queued, and closes the file so
    /// that a copy of it is whole. Every later call answers [`StoreError::Stopped`];
    /// a caller uses this to end a server, or a test to restart one.
    pub fn close(self) {
        let inner = self.inner;
        let sender = inner.writer.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(sender) = sender {
            let (reply, answer) = mpsc::sync_channel(1);
            if sender.send(Job::Stop(reply)).is_ok() {
                let _ = answer.recv();
            }
        }
        let thread = inner
            .writer_thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }

    /// Sends one job to the writer.
    fn send(&self, job: Job) -> Result<(), StoreError> {
        let sender = self.inner.writer.lock().unwrap_or_else(PoisonError::into_inner).clone();
        sender
            .ok_or(StoreError::Stopped)?
            .send(job)
            .map_err(|_| StoreError::Stopped)
    }
}

impl std::fmt::Debug for Store {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Store")
            .field("path", &self.inner.path)
            .field("store_id", &self.inner.store_id)
            .field("index", &self.index())
            .finish()
    }
}

impl Inner {
    /// Runs `read` on an idle read connection, or on a new one.
    fn read<T>(&self, read: impl FnOnce(&Connection) -> Result<T, StoreError>) -> Result<T, StoreError> {
        let connection = self.readers.take()?;
        let result = read(&connection);
        if result.is_ok() {
            let mut idle = self.readers.idle.lock().unwrap_or_else(PoisonError::into_inner);
            if idle.len() < MAX_IDLE_READERS {
                idle.push(connection);
            }
        }
        result
    }
}

/// The idle read connections of one store.
struct Readers {
    path: PathBuf,
    idle: Mutex<Vec<Connection>>,
}

impl Readers {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            idle: Mutex::new(Vec::new()),
        }
    }

    /// An idle connection, or a new one.
    fn take(&self) -> Result<Connection, StoreError> {
        let idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner).pop();
        match idle {
            Some(connection) => Ok(connection),
            None => schema::read_only(&self.path),
        }
    }
}

/// The last index that the writer committed, and a signal for the readers that
/// wait for the next one.
struct Signal {
    index: Mutex<u64>,
    changed: Condvar,
}

impl Signal {
    fn new(index: u64) -> Self {
        Self {
            index: Mutex::new(index),
            changed: Condvar::new(),
        }
    }

    fn index(&self) -> u64 {
        *self.index.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Publishes a committed index and wakes the readers that wait. This runs after
    /// the commit, so a reader that wakes reads a change that is on the disk.
    fn publish(&self, index: u64) {
        let mut current: MutexGuard<'_, u64> = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        if index > *current {
            *current = index;
        }
        drop(current);
        self.changed.notify_all();
    }

    /// Waits until the index is above `since` or `deadline` passes, and returns
    /// whether a change is there.
    fn wait_after(&self, since: u64, deadline: Instant) -> bool {
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if *index > since {
                return true;
            }
            let now = Instant::now();
            let Some(remaining) = deadline.checked_duration_since(now) else {
                return false;
            };
            let (guard, timeout) = self
                .changed
                .wait_timeout(index, remaining)
                .unwrap_or_else(PoisonError::into_inner);
            index = guard;
            if timeout.timed_out() && *index <= since {
                return false;
            }
        }
    }
}

/// The current time of the server, in milliseconds since the Unix epoch. Every
/// change carries this stamp, never the clock of a computer.
pub(super) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}
