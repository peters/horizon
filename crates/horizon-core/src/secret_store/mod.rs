//! Every secret value of this computer, in one SQLite file.
//!
//! A secret value is one value that Horizon must keep for a person: the value
//! under one credential reference of a remote browser provider, and the value of
//! one named secret that every agent panel of this computer gets as an
//! environment variable. Both are here, as rows of `<Horizon home>/secrets.sqlite`
//! under the id `credential/<reference>` and `named/<NAME>`.
//!
//! The file is the whole protection: the folder is `0700`, the file is `0600` from
//! the first byte, and a value is stored as it is, never sealed. A person who can
//! read one file of that home reads every value, as a person who can read the
//! machine keys under `machine-keys/` reads every key and a person who can read
//! `~/.ssh` reads every ssh key. The pages
//! [`docs/review/trust-boundaries/named-secrets.md`](../../../../docs/review/trust-boundaries/named-secrets.md)
//! and [`docs/architecture/settings-store.md`](../../../../docs/architecture/settings-store.md)
//! state that plainly.
//!
//! **One file has one connection in this process.** [`SecretStore::open`] answers
//! the store that this process already holds for that path, so the settings
//! workbench, the credential vault and the named secret vault of the link, and
//! every reader that a usage refresh starts, share one connection behind one
//! lock. The table holds a weak reference, so the last holder of a store closes
//! the file. A second connection to one file therefore cannot be made by
//! construction: two connections of one process do not see each other's POSIX
//! locks, which is what lets a write-ahead log of one of them be truncated while
//! the other writes into its mapping.
//!
//! The connection is one lock, so every write of this process is serialized, and
//! no method here runs on the UI thread: the Settings workbench, the named secrets
//! worker and the link to a server each reach the store from a worker thread. The
//! file uses the rollback journal, so no side file is mapped and a home that is
//! removed under a worker ends in the typed error of this store.
//!
//! A machine key is not here. `ssh` needs the private key as a path, so a machine
//! key stays the three files of `machine-keys/<name>`.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError, Weak},
    time::{SystemTime, UNIX_EPOCH},
};

use horizon_browser::remote::CredentialStoreKind;
use rusqlite::{Connection, OptionalExtension, params};
use zeroize::Zeroizing;

use crate::HorizonHome;
use crate::remote_browser_credential::{
    CredentialLocator, RemoteCredentialError, RemoteCredentialStore, Sealed, SecretSink, validate_secret,
};

mod schema;
#[cfg(test)]
mod tests;

pub use schema::SECRETS_SCHEMA_VERSION;

/// The name of the file of the secret values, in the Horizon home.
pub const SECRETS_FILE_NAME: &str = "secrets.sqlite";

/// The slot of each secrets file that this process opened, keyed by the canonical
/// path of the file.
static OPEN_FILES: Mutex<BTreeMap<PathBuf, Arc<FileSlot>>> = Mutex::new(BTreeMap::new());

/// How many times this process opened the file at a path, so a test can show that
/// one file is opened once.
#[cfg(test)]
static OPEN_COUNTS: Mutex<BTreeMap<PathBuf, usize>> = Mutex::new(BTreeMap::new());

/// The slot of one secrets file: the store that this process holds for that file,
/// or none while no one holds it.
///
/// The lock of a slot is the lock of its own file, so a thread that opens one file
/// never waits for another file to be opened, and no two threads open one file at
/// the same moment. The slot holds a weak reference, so a store goes when its last
/// holder goes.
#[derive(Default)]
struct FileSlot {
    store: Mutex<Option<Weak<SecretStore>>>,
}

/// The key of one secrets file: the canonical path of the file, so two spellings of
/// one path share one store.
///
/// A file that is not there yet is keyed by the canonical folder that is there and
/// the rest of the path under it, so the key of a file does not change when the
/// folder of the file is made.
fn file_key(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    let mut names = Vec::new();
    let mut current = path;
    loop {
        let Some(name) = current.file_name() else {
            return path.to_path_buf();
        };
        names.push(name.to_os_string());
        let Some(parent) = current.parent() else {
            return path.to_path_buf();
        };
        if let Ok(folder) = fs::canonicalize(parent) {
            let mut key = folder;
            for name in names.iter().rev() {
                key.push(name);
            }
            return key;
        }
        if parent.as_os_str().is_empty() {
            return path.to_path_buf();
        }
        current = parent;
    }
}

/// The slot of one secrets file, made on its first open.
fn slot_of(key: &Path) -> Arc<FileSlot> {
    let mut files = OPEN_FILES.lock().unwrap_or_else(PoisonError::into_inner);
    Arc::clone(files.entry(key.to_path_buf()).or_default())
}

/// The secret values of one Horizon home.
pub struct SecretStore {
    path: PathBuf,
    connection: Mutex<Connection>,
}

impl std::fmt::Debug for SecretStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl SecretStore {
    /// The store of the file at `path`, made when it is not there.
    ///
    /// This answers the store that this process already holds for the file. One
    /// lock covers the lookup and the making of the file, so a second connection to
    /// one file cannot be made and the tables of a new file are made once.
    ///
    /// # Errors
    /// Returns the path and the reason when the folder or the file cannot be
    /// made, made private, read or written.
    pub fn open(path: PathBuf) -> Result<Arc<Self>, RemoteCredentialError> {
        crate::ui_thread::blocking("the secrets file is opened");
        let key = file_key(&path);
        let slot = slot_of(&key);
        let mut held = slot.store.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(store) = held.as_ref().and_then(Weak::upgrade) {
            return Ok(store);
        }
        let connection = schema::open(&path)?;
        #[cfg(test)]
        {
            let mut counts = OPEN_COUNTS.lock().unwrap_or_else(PoisonError::into_inner);
            *counts.entry(key).or_insert(0) += 1;
        }
        let store = Arc::new(Self {
            path,
            connection: Mutex::new(connection),
        });
        *held = Some(Arc::downgrade(&store));
        Ok(store)
    }

    /// A store that holds its values in memory and no file, for a test and for a
    /// preview of the window.
    ///
    /// # Errors
    /// Returns why a store in memory cannot be made.
    pub fn in_memory() -> Result<Arc<Self>, RemoteCredentialError> {
        let connection = schema::in_memory()?;
        Ok(Arc::new(Self {
            path: PathBuf::from(schema::IN_MEMORY),
            connection: Mutex::new(connection),
        }))
    }

    /// How many times this process opened the file at `path`.
    #[cfg(test)]
    pub(super) fn opens(path: &Path) -> usize {
        let counts = OPEN_COUNTS.lock().unwrap_or_else(PoisonError::into_inner);
        counts.get(&file_key(path)).copied().unwrap_or(0)
    }

    /// The file of this store.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The value of `id`, or `None` when this store holds none.
    ///
    /// # Errors
    /// Returns the path and the reason when the row cannot be read.
    pub fn read(&self, id: &str) -> Result<Option<Zeroizing<Vec<u8>>>, RemoteCredentialError> {
        crate::ui_thread::blocking("a secret value is read");
        let connection = self.lock();
        let stored: Option<Vec<u8>> = connection
            .query_row("SELECT body FROM secrets WHERE id = ?1", params![id], |row| row.get(0))
            .optional()
            .map_err(|error| self.file_error(&error.to_string()))?;
        Ok(stored.map(Zeroizing::new))
    }

    /// Writes `body` at `id`, replacing a value that is there.
    ///
    /// # Errors
    /// Returns the path and the reason when the row cannot be written.
    pub fn write(&self, id: &str, body: &[u8]) -> Result<(), RemoteCredentialError> {
        crate::ui_thread::blocking("a secret value is written");
        let connection = self.lock();
        connection
            .execute(
                "INSERT INTO secrets (id, body, version, updated_millis) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (id) DO UPDATE SET body = excluded.body,
                     version = secrets.version + 1, updated_millis = excluded.updated_millis",
                params![id, body, 1_i64, now_millis()],
            )
            .map_err(|error| self.file_error(&error.to_string()))?;
        Ok(())
    }

    /// Forgets the value at `id`. A value that is not there is already forgotten.
    ///
    /// # Errors
    /// Returns the path and the reason when the row cannot be removed.
    pub fn remove(&self, id: &str) -> Result<(), RemoteCredentialError> {
        crate::ui_thread::blocking("a secret value is removed");
        let connection = self.lock();
        connection
            .execute("DELETE FROM secrets WHERE id = ?1", params![id])
            .map_err(|error| self.file_error(&error.to_string()))?;
        Ok(())
    }

    /// Whether this store holds a value at `id`.
    ///
    /// # Errors
    /// Returns the path and the reason when the row cannot be read.
    pub fn exists(&self, id: &str) -> Result<bool, RemoteCredentialError> {
        crate::ui_thread::blocking("a secret value is looked for");
        let connection = self.lock();
        let found: Option<i64> = connection
            .query_row("SELECT 1 FROM secrets WHERE id = ?1", params![id], |row| row.get(0))
            .optional()
            .map_err(|error| self.file_error(&error.to_string()))?;
        Ok(found.is_some())
    }

    /// The ids of every value that this store holds, in id order.
    ///
    /// # Errors
    /// Returns the path and the reason when the rows cannot be read.
    pub fn ids(&self) -> Result<Vec<String>, RemoteCredentialError> {
        crate::ui_thread::blocking("the ids of the secrets are read");
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT id FROM secrets ORDER BY id")
            .map_err(|error| self.file_error(&error.to_string()))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| self.file_error(&error.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| self.file_error(&error.to_string()))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn file_error(&self, reason: &str) -> RemoteCredentialError {
        RemoteCredentialError::File {
            path: self.path.clone(),
            reason: reason.to_owned(),
        }
    }
}

impl Sealed for Arc<SecretStore> {}

/// The one store of a secrets file, as the window and the link hold it.
///
/// The trait asks for `&mut self` while the store serializes every write behind its
/// own lock, so a shared handle to one file is the one store of that file.
impl RemoteCredentialStore for Arc<SecretStore> {
    fn kind(&self) -> CredentialStoreKind {
        CredentialStoreKind::File
    }

    fn put(&mut self, locator: &CredentialLocator, secret: &[u8]) -> Result<(), RemoteCredentialError> {
        validate_secret(secret)?;
        self.write(&locator.secret_id(), secret)
    }

    fn delete(&mut self, locator: &CredentialLocator) -> Result<(), RemoteCredentialError> {
        self.remove(&locator.secret_id())
    }

    fn contains(&self, locator: &CredentialLocator) -> Result<bool, RemoteCredentialError> {
        self.exists(&locator.secret_id())
    }

    fn with_secret(&self, locator: &CredentialLocator, sink: &mut dyn SecretSink) -> Result<(), RemoteCredentialError> {
        let value = self.read(&locator.secret_id())?.ok_or(RemoteCredentialError::Missing)?;
        sink.accept(&value)
    }
}

/// The place of the secret values of this computer when its file cannot be
/// opened: every operation answers that one error, with the path of the file.
///
/// A file that this computer cannot read is a fault, not a way of life, so the
/// window and the link keep one code path and say what happened.
pub struct UnreadableStore {
    error: RemoteCredentialError,
}

impl std::fmt::Debug for UnreadableStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UnreadableStore")
            .field("error", &self.error)
            .finish()
    }
}

impl UnreadableStore {
    /// A place that holds no value, because `error` says why.
    #[must_use]
    pub fn new(error: RemoteCredentialError) -> Self {
        Self { error }
    }

    /// Why this place holds no value.
    #[must_use]
    pub fn error(&self) -> &RemoteCredentialError {
        &self.error
    }
}

impl Sealed for UnreadableStore {}

impl RemoteCredentialStore for UnreadableStore {
    fn kind(&self) -> CredentialStoreKind {
        CredentialStoreKind::File
    }

    fn put(&mut self, _: &CredentialLocator, _: &[u8]) -> Result<(), RemoteCredentialError> {
        Err(self.error.clone())
    }

    fn delete(&mut self, _: &CredentialLocator) -> Result<(), RemoteCredentialError> {
        Err(self.error.clone())
    }

    fn contains(&self, _: &CredentialLocator) -> Result<bool, RemoteCredentialError> {
        Err(self.error.clone())
    }

    fn with_secret(&self, _: &CredentialLocator, _: &mut dyn SecretSink) -> Result<(), RemoteCredentialError> {
        Err(self.error.clone())
    }
}

/// The place of the secret values of `home`: its file, or a place that answers the
/// one typed error of a file that cannot be opened.
///
/// It opens a file, so it runs on a worker thread and never on the UI thread.
#[must_use]
pub fn open(home: &HorizonHome) -> Box<dyn RemoteCredentialStore + Send> {
    match SecretStore::open(home.secrets_path()) {
        Ok(store) => Box::new(store),
        Err(error) => {
            tracing::error!(
                %error,
                "the secrets file of this computer cannot be opened, so it holds no value"
            );
            Box::new(UnreadableStore::new(error))
        }
    }
}

/// The current time, in milliseconds since the Unix epoch.
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}
