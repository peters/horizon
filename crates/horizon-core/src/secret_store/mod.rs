//! Every secret value of this computer, in one SQLite file.
//!
//! A secret value is one value that Horizon must keep for a person: the value
//! under one credential reference of a remote browser provider, and the value of
//! one named secret that every agent panel of this computer gets as an
//! environment variable. Both are here, as rows of `<Horizon home>/secrets.sqlite`
//! under the id `credential/<reference>` and `named/<NAME>`.
//!
//! The file is the whole protection: the folder is `0700`, the file and its
//! `-wal` and `-shm` are `0600` from the first byte, and a value is stored as it
//! is, never sealed. A person who can read one file of that home reads every value,
//! as a person who can read the machine keys under `machine-keys/` reads every key
//! and a person who can read `~/.ssh` reads every ssh key. The pages
//! [`docs/review/trust-boundaries/named-secrets.md`](../../../../docs/review/trust-boundaries/named-secrets.md)
//! and [`docs/architecture/settings-store.md`](../../../../docs/architecture/settings-store.md)
//! state that plainly.
//!
//! The store is one connection behind one lock, so every write of this process is
//! serialized, and no method here runs on the UI thread: the Settings workbench,
//! the named secrets worker and the link to a server each open the file on their
//! own worker thread. The write-ahead log lets a second Horizon process read while
//! this one writes.
//!
//! A machine key is not here. `ssh` needs the private key as a path, so a machine
//! key stays the three files of `machine-keys/<name>`.

use std::{
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
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
    /// # Errors
    /// Returns the path and the reason when the folder or the file cannot be
    /// made, made private, read or written.
    pub fn open(path: PathBuf) -> Result<Self, RemoteCredentialError> {
        let connection = schema::open(&path)?;
        Ok(Self {
            path,
            connection: Mutex::new(connection),
        })
    }

    /// A store that holds its values in memory and no file, for a test and for a
    /// preview of the window.
    ///
    /// # Errors
    /// Returns why a store in memory cannot be made.
    pub fn in_memory() -> Result<Self, RemoteCredentialError> {
        let connection = schema::in_memory()?;
        Ok(Self {
            path: PathBuf::from(schema::IN_MEMORY),
            connection: Mutex::new(connection),
        })
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

impl Sealed for SecretStore {}

impl RemoteCredentialStore for SecretStore {
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
