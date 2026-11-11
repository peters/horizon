//! The secrets of one computer, of every kind, behind the one trait that the link
//! uses.
//!
//! A machine key is a file family under `machine-keys/` of the Horizon home, a
//! credential value is a row of the secrets file under `credential/<reference>`, and
//! a named secret is a row of that file under `named/<NAME>` that the agent panels of
//! this computer read as an environment variable. All are reached here, so the link
//! has one place that reads the value of an id, writes the value that the server
//! decided, and keeps a local value that lost.
//!
//! The version rule of the server decides between two computers that changed one
//! secret: the first write on a version wins. The loser keeps its own value as
//! `<name>.conflict-<time>` for a machine key, and only as one line for the person
//! for a credential value, because the file holds one value per reference and the
//! person must decide which one is right.

use std::{fmt, sync::Arc};

use zeroize::Zeroizing;

use super::{CredentialVault, MachineKeyFiles, SecretError, SecretId, SecretKind, SecretTarget};
use crate::MachineKeys;
use crate::named_secrets::{NamedSecretVault, SecretName};

/// What this computer keeps of every secret.
pub struct SecretVault {
    keys: MachineKeys,
    credentials: Arc<CredentialVault>,
    named: Arc<NamedSecretVault>,
}

impl fmt::Debug for SecretVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretVault")
            .field("keys", &self.keys.key_directory())
            .field("credentials", &self.credentials.references().len())
            .field("named_secrets", &self.named.names().len())
            .finish()
    }
}

impl SecretVault {
    /// The keys of `keys`, the credential values of `credentials`, and the named
    /// secrets of `named`, so a value that a server holds becomes the environment of
    /// an agent panel here.
    #[must_use]
    pub fn new(keys: MachineKeys, credentials: Arc<CredentialVault>, named: Arc<NamedSecretVault>) -> Self {
        Self {
            keys,
            credentials,
            named,
        }
    }

    /// The credential values of this computer.
    #[must_use]
    pub fn credentials(&self) -> &Arc<CredentialVault> {
        &self.credentials
    }

    /// The named secrets of this computer.
    #[must_use]
    pub fn named(&self) -> &Arc<NamedSecretVault> {
        &self.named
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
                Some(files) if files.is_usable() => {
                    let body = serde_json::to_vec(&files).map_err(|error| SecretError::Unavailable {
                        reason: format!("the machine key {}: {error}", id.name()),
                    })?;
                    Ok(Some(Zeroizing::new(body)))
                }
                Some(_) => Err(SecretError::Unreadable {
                    message: format!(
                        "The machine key {} of this Horizon home is incomplete, so it is not sent",
                        id.name()
                    ),
                }),
                None => Ok(None),
            },
            SecretKind::Credential => self.credentials.value(id.name()),
            SecretKind::Named => self
                .named
                .value(&named_secret_name(id)?)
                .map_err(|error| named_failure(&error)),
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
            SecretKind::Credential => self.credentials.write(id.name(), body),
            SecretKind::Named => {
                let name = named_secret_name(id)?;
                self.named
                    .put(&name, body)
                    .map(|_| ())
                    .map_err(|error| named_failure(&error))
            }
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
            SecretKind::Credential => self.credentials.remove(id.name()),
            SecretKind::Named => {
                let name = named_secret_name(id)?;
                self.named
                    .remove(&name)
                    .map(|_| ())
                    .map_err(|error| named_failure(&error))
            }
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
                let conflict = self
                    .keys
                    .write_conflict(id.name(), &files, at_millis)
                    .map_err(|error| SecretError::Unavailable {
                        reason: error.to_string(),
                    })?;
                Ok(format!(
                    "The machine key {} of this computer was kept as {conflict} because the server has another one",
                    id.name()
                ))
            }
            SecretKind::Credential => Ok(format!(
                "The value of the credential {} of this computer was replaced by the value of the server",
                id.name()
            )),
            SecretKind::Named => Ok(format!(
                "The value of the named secret {} of this computer was replaced by the value of the server",
                id.name()
            )),
        }
    }

    fn same_value(&self, id: &SecretId, left: &[u8], right: &[u8]) -> bool {
        if id.kind() != SecretKind::MachineKey {
            return left == right;
        }
        let Ok(parsed_left) = serde_json::from_slice::<MachineKeyFiles>(left) else {
            return left == right;
        };
        let Ok(parsed_right) = serde_json::from_slice::<MachineKeyFiles>(right) else {
            return left == right;
        };
        parsed_left.private_key.trim() == parsed_right.private_key.trim()
            && parsed_left.public_key.trim() == parsed_right.public_key.trim()
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
            SecretKind::Credential => self
                .credentials
                .present()
                .into_iter()
                .filter_map(|reference| SecretId::credential(&reference).ok())
                .map(|id| id.as_str().to_owned())
                .collect(),
            SecretKind::Named => self.named.local_ids(),
        }
    }
}

/// The name of the named secret that `id` holds, or the refusal of an id that is
/// not one.
fn named_secret_name(id: &SecretId) -> Result<SecretName, SecretError> {
    SecretName::parse(id.name()).map_err(|error| SecretError::InvalidId {
        message: error.to_string(),
    })
}

/// One refusal of a named secret of this computer, as the link reports it. A value that
/// the rule of the feature refuses is unreadable; everything else is the store.
fn named_failure(error: &crate::named_secrets::NamedSecretError) -> SecretError {
    match error {
        crate::named_secrets::NamedSecretError::InvalidValue { .. }
        | crate::named_secrets::NamedSecretError::ValueNotText { .. }
        | crate::named_secrets::NamedSecretError::InvalidName { .. }
        | crate::named_secrets::NamedSecretError::ReservedName { .. } => SecretError::Unreadable {
            message: error.to_string(),
        },
        crate::named_secrets::NamedSecretError::Unavailable { .. } => SecretError::Unavailable {
            reason: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests;
