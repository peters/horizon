//! The secrets of one computer, of both kinds, behind the one trait that the link
//! uses.
//!
//! A machine key is a file family under `machine-keys/` of the Horizon home, and a
//! credential value is an item of the OS credential store. Both are reached here,
//! so the link has one place that reads the value of an id, writes the value that
//! the server decided, and keeps a local value that lost.
//!
//! The version rule of the server decides between two computers that changed one
//! secret: the first write on a version wins. The loser keeps its own value as
//! `<name>.conflict-<time>` for a machine key, and only as one line for the person
//! for a credential value, because the OS store holds one value per reference and
//! the person must decide which one is right.

use std::fmt;

use zeroize::Zeroizing;

use super::{CredentialVault, MachineKeyFiles, SecretError, SecretId, SecretKind, SecretTarget};
use crate::MachineKeys;

/// What this computer keeps of every secret.
pub struct SecretVault {
    keys: MachineKeys,
    credentials: Option<CredentialVault>,
}

impl fmt::Debug for SecretVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretVault")
            .field("keys", &self.keys.key_directory())
            .field("credentials", &self.credentials.is_some())
            .finish()
    }
}

impl SecretVault {
    /// The keys of `keys`, and the credential values of `credentials` when this
    /// computer has an OS credential store.
    #[must_use]
    pub fn new(keys: MachineKeys, credentials: Option<CredentialVault>) -> Self {
        Self { keys, credentials }
    }

    /// The keys of `keys` alone, for a computer without an OS credential store.
    #[must_use]
    pub fn keys_only(keys: MachineKeys) -> Self {
        Self {
            keys,
            credentials: None,
        }
    }

    /// The credential values of this computer, when it has an OS store.
    #[must_use]
    pub fn credentials(&self) -> Option<&CredentialVault> {
        self.credentials.as_ref()
    }

    fn key_files(&self, id: &SecretId) -> Result<Option<MachineKeyFiles>, SecretError> {
        self.keys.files(id.name()).map_err(|error| SecretError::Unavailable {
            reason: error.to_string(),
        })
    }
}

impl SecretTarget for SecretVault {
    fn read(&self, id: &SecretId) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        match id.kind() {
            SecretKind::MachineKey => match self.key_files(id)? {
                Some(files) => {
                    let body = serde_json::to_vec(&files).map_err(|error| SecretError::Unavailable {
                        reason: format!("the machine key {}: {error}", id.name()),
                    })?;
                    Ok(Some(Zeroizing::new(body)))
                }
                None => Ok(None),
            },
            SecretKind::Credential => match &self.credentials {
                Some(credentials) => credentials.value(id.name()),
                None => Err(SecretError::Unsupported {
                    reason: "This computer has no OS credential store, so it holds no credential value".to_owned(),
                }),
            },
        }
    }

    fn write(&self, id: &SecretId, body: &[u8]) -> Result<(), SecretError> {
        match id.kind() {
            SecretKind::MachineKey => {
                let files: MachineKeyFiles = serde_json::from_slice(body).map_err(|error| SecretError::Unreadable {
                    message: format!(
                        "The machine key {} of the server is not one that this Horizon reads: {error}",
                        id.name()
                    ),
                })?;
                if !files.is_usable() {
                    return Err(SecretError::Unreadable {
                        message: format!(
                            "The machine key {} of the server holds no private key or no public line",
                            id.name()
                        ),
                    });
                }
                self.keys
                    .write_files(id.name(), &files)
                    .map_err(|error| SecretError::Unavailable {
                        reason: error.to_string(),
                    })
            }
            SecretKind::Credential => match &self.credentials {
                Some(credentials) => credentials.write(id.name(), body),
                None => Err(SecretError::Unsupported {
                    reason: "This computer has no OS credential store, so it keeps no credential value".to_owned(),
                }),
            },
        }
    }

    fn remove(&self, id: &SecretId) -> Result<(), SecretError> {
        match id.kind() {
            SecretKind::MachineKey => {
                if self.key_files(id)?.is_none() {
                    return Ok(());
                }
                self.keys.delete(id.name()).map_err(|error| SecretError::Unavailable {
                    reason: error.to_string(),
                })
            }
            SecretKind::Credential => match &self.credentials {
                Some(credentials) => credentials.remove(id.name()),
                None => Ok(()),
            },
        }
    }

    fn keep_local(&self, id: &SecretId, body: &[u8], at_millis: u64) -> Result<String, SecretError> {
        match id.kind() {
            SecretKind::MachineKey => {
                // A value that this computer never published is kept as a whole
                // pair, so the person can still read it and put it on the machine.
                let files: MachineKeyFiles = serde_json::from_slice(body).map_err(|error| SecretError::Unreadable {
                    message: format!("the machine key {}: {error}", id.name()),
                })?;
                self.keys
                    .write_files(&conflict_name(id.name(), at_millis), &files)
                    .map_err(|error| SecretError::Unavailable {
                        reason: error.to_string(),
                    })?;
                Ok(format!(
                    "The machine key {} of this computer was kept as {} because the server has another one",
                    id.name(),
                    conflict_name(id.name(), at_millis)
                ))
            }
            SecretKind::Credential => Ok(format!(
                "The value of the credential {} of this computer was replaced by the value of the server",
                id.name()
            )),
        }
    }

    fn local_ids(&self, kind: SecretKind) -> Vec<String> {
        match kind {
            SecretKind::MachineKey => self
                .keys
                .list()
                .map(|keys| {
                    keys.into_iter()
                        .filter_map(|key| SecretId::machine_key(&key.name).ok())
                        .map(|id| id.as_str().to_owned())
                        .collect()
                })
                .unwrap_or_default(),
            SecretKind::Credential => match &self.credentials {
                Some(credentials) => credentials
                    .present()
                    .into_iter()
                    .filter_map(|reference| SecretId::credential(&reference).ok())
                    .map(|id| id.as_str().to_owned())
                    .collect(),
                None => Vec::new(),
            },
        }
    }
}

/// The name of the copy of a machine key that lost the version rule.
fn conflict_name(name: &str, at_millis: u64) -> String {
    format!("{name}.conflict-{at_millis}")
}

#[cfg(test)]
mod tests;
