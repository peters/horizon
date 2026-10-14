//! Desktop-only API credential bindings, independent of worker enrollment keys.
mod credential;
mod records;
use super::{Error, Result, Store, keychain, sync_directory, write};
pub use credential::{ApiCredential, ApiCredentialKind, ApiCredentialStatus};
use records::{Binding, Pending, Records};
use std::collections::BTreeSet;
use zeroize::Zeroizing;

const METADATA: &str = "tailnet-api-bindings.json";
const JOURNAL: &str = "tailnet-api.pending.json";

pub(super) trait Credentials {
    fn read(&self, slot: &str) -> Result<Option<Zeroizing<Vec<u8>>>>;
    fn put(&self, slot: &str, bytes: &[u8]) -> Result<()>;
    fn delete(&self, slot: &str) -> Result<()>;
}
pub(super) struct Native;
impl Credentials for Native {
    fn read(&self, slot: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        keychain::api_read(slot)
    }
    fn put(&self, slot: &str, bytes: &[u8]) -> Result<()> {
        keychain::api_put(slot, bytes)
    }
    fn delete(&self, slot: &str) -> Result<()> {
        keychain::api_delete(slot)
    }
}

fn records(store: &Store) -> Result<Records> {
    let records: Records = store.read(METADATA, 64 * 1024)?.unwrap_or_default();
    records.validate(store)?;
    Ok(records)
}

pub(super) fn recover(store: &Store, keys: &impl Credentials) -> Result<()> {
    let Some(pending) = store.read::<Pending>(JOURNAL, 192 * 1024)? else {
        return Ok(());
    };
    pending.validate(store)?;
    let current = records(store)?;
    let obsolete = if current == pending.after {
        pending.retired
    } else if current == pending.before {
        pending.created.into_iter().collect()
    } else {
        return Err(Error::Storage);
    };
    write(&store.root.join(METADATA), &current)?;
    for slot in obsolete {
        keys.delete(&slot)?;
    }
    std::fs::remove_file(store.root.join(JOURNAL)).map_err(|_| Error::Storage)?;
    sync_directory(&store.root)
}

pub(super) fn status(store: &Store, id: &str, keys: &impl Credentials) -> Result<ApiCredentialStatus> {
    recover(store, keys)?;
    store.require_tailnet(id)?;
    let records = records(store)?;
    let (slot, expected) = match records.bindings.get(id) {
        Some(Binding::Disabled) => return Ok(ApiCredentialStatus::Missing),
        Some(Binding::Active { slot, kind }) => (slot.as_str(), Some(*kind)),
        None => (id, None),
    };
    let Some(bytes) = keys.read(slot)? else {
        return Ok(ApiCredentialStatus::Missing);
    };
    let credential = ApiCredential::decode(&bytes)?;
    if expected.is_some_and(|kind| kind != credential.kind()) {
        return Err(Error::InvalidApiCredential);
    }
    Ok(ApiCredentialStatus::Stored(credential.kind()))
}

pub(super) fn save(store: &Store, id: &str, credential: &ApiCredential, keys: &impl Credentials) -> Result<()> {
    recover(store, keys)?;
    store.require_tailnet(id)?;
    let before = records(store)?;
    let mut after = before.clone();
    let slot = uuid::Uuid::new_v4().to_string();
    after.bindings.insert(
        id.into(),
        Binding::Active {
            slot: slot.clone(),
            kind: credential.kind(),
        },
    );
    let pending = pending(before, after, id, Some(slot.clone()));
    pending.validate(store)?;
    write(&store.root.join(JOURNAL), &pending)?;
    keys.put(&slot, &credential.encode()?)?;
    write(&store.root.join(METADATA), &pending.after)?;
    recover(store, keys)
}

fn pending(before: Records, after: Records, id: &str, created: Option<String>) -> Pending {
    let mut retired: BTreeSet<_> = before.slots().difference(&after.slots()).cloned().collect();
    retired.insert(id.into());
    Pending {
        before,
        after,
        created,
        retired,
    }
}

pub(super) fn remove(store: &Store, id: &str, keys: &impl Credentials) -> Result<()> {
    recover(store, keys)?;
    store.require_tailnet(id)?;
    let before = records(store)?;
    if before.bindings.get(id) == Some(&Binding::Disabled) {
        return keys.delete(id);
    }
    let mut after = before.clone();
    after.bindings.insert(id.into(), Binding::Disabled);
    let pending = pending(before, after, id, None);
    pending.validate(store)?;
    write(&store.root.join(JOURNAL), &pending)?;
    write(&store.root.join(METADATA), &pending.after)?;
    recover(store, keys)
}

pub(super) fn forget_removed_binding(store: &Store, id: &str) -> Result<()> {
    if store.load()?.tailnets.iter().any(|network| network.id == id)
        || store.read::<Pending>(JOURNAL, 192 * 1024)?.is_some()
    {
        return Err(Error::Storage);
    }
    let mut current = records(store)?;
    match current.bindings.get(id) {
        None => Ok(()),
        Some(Binding::Disabled) => {
            current.bindings.remove(id);
            write(&store.root.join(METADATA), &current)
        }
        Some(Binding::Active { .. }) => Err(Error::Storage),
    }
}

impl Store {
    /// Reports only the source kind; saved values never return to callers.
    /// # Errors
    /// Missing network, invalid metadata/secret format, or an unavailable OS store.
    pub fn api_credential_status(&self, id: &str) -> Result<ApiCredentialStatus> {
        let _lock = self.lock()?;
        status(self, id, &Native)
    }
    /// Saves in a fresh OS-store slot without changing the enrollment key.
    /// # Errors
    /// Missing network, conflicting metadata, or failed secret/persistence/cleanup.
    pub fn save_api_credential(&self, id: &str, credential: &ApiCredential) -> Result<()> {
        let _lock = self.lock()?;
        save(self, id, credential, &Native)
    }
    /// Removes both managed and staged credentials, disabling staged fallback.
    /// # Errors
    /// Missing network or failed durable cleanup. An interrupted removal is retryable.
    pub fn remove_api_credential(&self, id: &str) -> Result<()> {
        let _lock = self.lock()?;
        remove(self, id, &Native)
    }
}

#[cfg(test)]
pub(super) mod tests;
