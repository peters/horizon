//! SSH keys. Creating a server with the operation's key stops Hetzner from
//! generating a root password and emailing it. The host masks its own sshd, so
//! the key opens no login on the host by itself.
use super::{Hetzner, Method, OPERATION_LABEL, resource_name};
use crate::{Cancellation, CloudError, valid_public_key};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SshKey {
    pub id: u64,
    pub name: String,
    pub public_key: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}
impl SshKey {
    /// # Errors
    /// Refuses a key another operation owns.
    pub fn verify(&self, operation_id: &str) -> Result<(), CloudError> {
        if self.name != resource_name(operation_id)?
            || self.labels.get(OPERATION_LABEL).map(String::as_str) != Some(operation_id)
        {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct Single {
    ssh_key: SshKey,
}

impl Hetzner {
    /// Registers the operation's public key, or returns the key it registered
    /// earlier. Keys cost nothing, so a lost response is looked up again rather
    /// than fenced.
    /// # Errors
    /// Refuses invalid keys, a key registered under another name and a registered
    /// key whose material differs.
    pub fn ensure_ssh_key(
        &self,
        operation_id: &str,
        public_key: &str,
        cancel: &Cancellation,
    ) -> Result<SshKey, CloudError> {
        let name = resource_name(operation_id)?;
        if !valid_public_key(public_key) {
            return Err(CloudError::Invalid("Worker requires an Ed25519 public key"));
        }
        if let Some(key) = self.owned_ssh_key(operation_id, cancel)? {
            return same_material(key, public_key);
        }
        let body = json!({"name": name, "public_key": public_key, "labels": {OPERATION_LABEL: operation_id}});
        match self.send(Method::Post, "/ssh_keys", Some(body), cancel) {
            Ok(value) => {
                let single: Single = serde_json::from_value(value).map_err(|_| CloudError::InvalidResponse)?;
                single.ssh_key.verify(operation_id)?;
                same_material(single.ssh_key, public_key)
            }
            // Either our earlier request succeeded or the key exists under another name.
            Err(failure) if failure.name_taken() => match self.owned_ssh_key(operation_id, cancel)? {
                Some(key) => same_material(key, public_key),
                None => Err(CloudError::Invalid(
                    "This public key is already registered in the Hetzner project under another name",
                )),
            },
            Err(failure) => Err(failure.into()),
        }
    }

    /// Keys carrying the operation's label.
    /// # Errors
    /// Refuses invalid operation IDs and reports provider failures.
    pub fn find_ssh_keys(&self, operation_id: &str, cancel: &Cancellation) -> Result<Vec<SshKey>, CloudError> {
        resource_name(operation_id)?;
        Ok(self.list_all(
            "/ssh_keys",
            &format!("label_selector={OPERATION_LABEL}%3D{operation_id}"),
            "ssh_keys",
            cancel,
        )?)
    }

    /// Deletes the operation's key and proves no key with its label remains.
    /// # Errors
    /// Refuses keys another operation owns and reports unfinished deletion.
    pub fn delete_ssh_key(&self, operation_id: &str, cancel: &Cancellation) -> Result<(), CloudError> {
        let keys = self.find_ssh_keys(operation_id, cancel)?;
        // Every labelled key must be ours before any is deleted.
        for key in &keys {
            key.verify(operation_id)?;
        }
        for key in keys {
            match self.send(Method::Delete, &format!("/ssh_keys/{}", key.id), None, cancel) {
                Ok(_) => {}
                Err(failure) if failure.not_found() => {}
                Err(failure) => return Err(failure.into()),
            }
        }
        if self.find_ssh_keys(operation_id, cancel)?.is_empty() {
            Ok(())
        } else {
            Err(CloudError::Invalid("SSH key deletion pending; reconcile again"))
        }
    }

    fn owned_ssh_key(&self, operation_id: &str, cancel: &Cancellation) -> Result<Option<SshKey>, CloudError> {
        let mut found = self.find_ssh_keys(operation_id, cancel)?;
        if found.len() > 1 {
            return Err(CloudError::DuplicateWorkers);
        }
        let Some(key) = found.pop() else { return Ok(None) };
        key.verify(operation_id)?;
        Ok(Some(key))
    }
}

/// The provider may drop the comment, so only the key type and material are compared.
fn same_material(key: SshKey, public_key: &str) -> Result<SshKey, CloudError> {
    if material(&key.public_key) != material(public_key) {
        return Err(CloudError::IdentityMismatch);
    }
    Ok(key)
}

fn material(text: &str) -> Vec<&str> {
    text.split_ascii_whitespace().take(2).collect()
}
