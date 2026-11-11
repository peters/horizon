//! The secrets of the server: one table of its own, every body sealed with a key
//! that is not part of the database.
//!
//! A secret is one value that more than one computer holds: a machine key pair
//! with its known-hosts lines, or one credential value. The server never reads a
//! body, so a body is opaque bytes to it, and the row of a secret in the log
//! holds the kind, the id and the version only. The table `secrets` keeps the
//! sealed body, which no other table holds.
//!
//! The key is one 32-byte file, `<server home>/secret.key`, at mode 0600, made
//! at the first start. It is never sent to a computer and never part of the
//! database, so a copy of `server.sqlite` alone holds no readable secret. Each
//! body is sealed with XChaCha20-Poly1305, a random 24-byte nonce per write and
//! the id of the object as associated data, so a body that a person moves to
//! another id does not open.
//!
//! A lost or replaced key does not stop the server: a live row that this key
//! cannot open is reported as no row at all, so a computer that still holds the
//! value sends it again, and the hello of the link says that secrets are lost.

use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Generate, Payload},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::error::StoreError;
use crate::intent::KIND_SECRET;
use crate::store::rows::{Row, to_sql};

/// The file of the key of the secrets, in the server home beside the database.
pub(super) const KEY_FILE: &str = "secret.key";

/// The number of bytes of the key.
const KEY_BYTES: usize = 32;

/// The mode of the key file: the server user alone reads it.
const KEY_MODE: u32 = 0o600;

/// The key that seals every secret of one server file.
pub(super) struct SecretKey {
    cipher: XChaCha20Poly1305,
    path: PathBuf,
}

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretKey")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl SecretKey {
    /// The key of the server home that holds `database`, made when it is not
    /// there. The file is private from its first byte, as the store file is.
    ///
    /// # Errors
    /// Returns why the key file cannot be read, made or made private.
    pub(super) fn beside(database: &Path) -> Result<Self, StoreError> {
        let path = database
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .join(KEY_FILE);
        let bytes = match read_key(&path)? {
            Some(bytes) => bytes,
            None => make_key(&path)?,
        };
        Ok(Self {
            cipher: XChaCha20Poly1305::new_from_slice(&bytes)
                .map_err(|_| corrupt(&path, "its length is not 32 bytes"))?,
            path,
        })
    }

    /// The file of this key.
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// Seals one body for the object `id`, with a random nonce. Answers the nonce
    /// and the sealed body.
    ///
    /// # Errors
    /// Returns why the system has no random nonce or why the cipher refused.
    pub(super) fn seal(&self, id: &str, body: &[u8]) -> Result<(Vec<u8>, Vec<u8>), StoreError> {
        let nonce = XNonce::try_generate().map_err(|error| StoreError::SecretKey {
            path: self.path.clone(),
            reason: format!("the random nonce of a secret: {error}"),
        })?;
        let sealed = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: body,
                    aad: id.as_bytes(),
                },
            )
            .map_err(|_| StoreError::SecretKey {
                path: self.path.clone(),
                reason: "the cipher refused a body".to_owned(),
            })?;
        Ok((nonce.to_vec(), sealed))
    }

    /// Opens the body of the object `id`.
    ///
    /// # Errors
    /// Returns [`StoreError::SecretUnreadable`] when this key is not the key that
    /// sealed the body, including a body that was moved to another id.
    pub(super) fn open(&self, id: &str, nonce: &[u8], sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, StoreError> {
        let nonce = XNonce::try_from(nonce).map_err(|_| StoreError::SecretUnreadable { id: id.to_owned() })?;
        self.cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: sealed,
                    aad: id.as_bytes(),
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| StoreError::SecretUnreadable { id: id.to_owned() })
    }
}

/// The bytes of the key file, or `None` when it is not there.
fn read_key(path: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(StoreError::SecretKey {
                path: path.to_path_buf(),
                reason: error.to_string(),
            });
        }
    };
    let mut bytes = Zeroizing::new(Vec::with_capacity(KEY_BYTES));
    file.read_to_end(&mut bytes).map_err(|error| StoreError::SecretKey {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    if bytes.len() != KEY_BYTES {
        return Err(corrupt(path, "its length is not 32 bytes"));
    }
    let mode = file
        .metadata()
        .map_err(|error| StoreError::SecretKey {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?
        .permissions()
        .mode()
        & 0o777;
    if mode != KEY_MODE {
        // A key that a copy or a restore left readable is made private again
        // before the server uses it.
        fs::set_permissions(path, fs::Permissions::from_mode(KEY_MODE)).map_err(|error| StoreError::SecretKey {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
    }
    Ok(Some(bytes))
}

/// Writes a new key at `path`, private from the byte that creates the file.
fn make_key(path: &Path) -> Result<Zeroizing<Vec<u8>>, StoreError> {
    let bytes: [u8; KEY_BYTES] = <[u8; KEY_BYTES]>::try_generate().map_err(|error| StoreError::SecretKey {
        path: path.to_path_buf(),
        reason: format!("the random key of the secrets: {error}"),
    })?;
    let bytes = Zeroizing::new(bytes.to_vec());
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(KEY_MODE)
        .open(path)
        .map_err(|error| StoreError::SecretKey {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| StoreError::SecretKey {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
    Ok(bytes)
}

fn corrupt(path: &Path, reason: &str) -> StoreError {
    StoreError::SecretKey {
        path: path.to_path_buf(),
        reason: reason.to_owned(),
    }
}

/// Writes one secret at this version, or its tombstone when `body` is `None`.
pub(super) fn put(
    transaction: &Transaction<'_>,
    key: &SecretKey,
    id: &str,
    body: Option<&[u8]>,
    index: u64,
) -> Result<(), StoreError> {
    let (nonce, sealed) = match body {
        Some(body) => key.seal(id, body)?,
        None => (Vec::new(), Vec::new()),
    };
    transaction.execute(
        "INSERT INTO secrets (id, nonce, sealed, version, deleted) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (id) DO UPDATE SET nonce = excluded.nonce, sealed = excluded.sealed,
             version = excluded.version, deleted = excluded.deleted",
        params![id, nonce, sealed, to_sql(index)?, i64::from(body.is_none())],
    )?;
    Ok(())
}

/// The version of one live secret, or `None` when it is not there, when it is
/// removed, or when this key cannot open it.
pub(super) fn version_of(transaction: &Transaction<'_>, key: &SecretKey, id: &str) -> Result<Option<u64>, StoreError> {
    let stored: Option<(i64, Vec<u8>, Vec<u8>)> = transaction
        .query_row(
            "SELECT version, nonce, sealed FROM secrets WHERE id = ?1 AND deleted = 0",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((version, nonce, sealed)) = stored else {
        return Ok(None);
    };
    if key.open(id, &nonce, &sealed).is_err() {
        return Ok(None);
    }
    Ok(u64::try_from(version).ok())
}

/// The number of live secrets.
pub(super) fn live_count(transaction: &Transaction<'_>) -> Result<u64, StoreError> {
    let count: i64 = transaction.query_row("SELECT COUNT(*) FROM secrets WHERE deleted = 0", [], |row| row.get(0))?;
    Ok(u64::try_from(count).unwrap_or(0))
}

/// The secret rows that changed after `since`, in version order. A live row that
/// this key cannot open is left out, with one line of the log, so a computer that
/// still holds the value sends it again.
pub(super) fn rows(connection: &Connection, since: u64, key: &SecretKey) -> Result<Vec<Row>, StoreError> {
    let mut statement = connection.prepare_cached(
        "SELECT id, nonce, sealed, version, deleted FROM secrets WHERE version > ?1 ORDER BY version, id",
    )?;
    let stored = statement
        .query_map([to_sql(since)?], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut rows = Vec::with_capacity(stored.len());
    for (id, nonce, sealed, version, deleted) in stored {
        let version = u64::try_from(version)
            .map_err(|_| StoreError::Damaged(format!("the version {version} of a secret is not one")))?;
        if deleted != 0 {
            rows.push(Row {
                kind: KIND_SECRET.to_owned(),
                id,
                body: Value::Null,
                version,
                deleted: true,
            });
            continue;
        }
        let Ok(body) = key.open(&id, &nonce, &sealed) else {
            tracing::warn!(
                id = %id,
                "a secret of the server cannot be opened with the key of this server, so it is not sent; a computer that holds it sends it again"
            );
            continue;
        };
        let Ok(text) = String::from_utf8(body.to_vec()) else {
            tracing::warn!(id = %id, "a secret of the server is not text, so it is not sent");
            continue;
        };
        rows.push(Row {
            kind: KIND_SECRET.to_owned(),
            id,
            body: Value::String(text),
            version,
            deleted: false,
        });
    }
    Ok(rows)
}

/// Whether a live secret of the server is sealed with another key, so this
/// server cannot open it. The hello of the link says so, and every computer that
/// still holds a secret sends it again.
pub(super) fn lost(connection: &Connection, key: &SecretKey) -> Result<bool, StoreError> {
    let stored: Option<(String, Vec<u8>, Vec<u8>)> = connection
        .query_row(
            "SELECT id, nonce, sealed FROM secrets WHERE deleted = 0 ORDER BY version DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    Ok(stored.is_some_and(|(id, nonce, sealed)| key.open(&id, &nonce, &sealed).is_err()))
}
