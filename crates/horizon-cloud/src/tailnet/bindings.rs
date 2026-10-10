//! Nonsecret recovery journal; a fresh key slot preserves the previous binding.
use super::{Catalog, Error, Result, Tailnet, keychain, sync_directory, valid_id, valid_key, valid_name, write};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    path::PathBuf,
};

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Records {
    #[serde(default)]
    tailnets: Vec<Tailnet>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    slots: BTreeMap<String, String>,
}
impl Records {
    fn validate(&self) -> Result<()> {
        let mut ids = BTreeSet::new();
        let mut slots = BTreeSet::new();
        if self.tailnets.iter().any(|t| {
            !valid_id(&t.id)
                || !valid_name(&t.name)
                || !ids.insert(&t.id)
                || !slots.insert(self.slots.get(&t.id).unwrap_or(&t.id))
        }) || self.slots.iter().any(|(id, slot)| !ids.contains(id) || !valid_id(slot))
            || serde_json::to_vec_pretty(self).map_err(|_| Error::Storage)?.len() > 64 * 1024
        {
            return Err(Error::Storage);
        }
        Ok(())
    }
    fn slot(&self, id: &str) -> Result<String> {
        self.tailnets
            .iter()
            .any(|t| t.id == id)
            .then(|| self.slots.get(id).map_or_else(|| id.to_owned(), Clone::clone))
            .ok_or(Error::Missing)
    }
    fn catalog(self) -> Catalog {
        Catalog {
            tailnets: self.tailnets,
        }
    }
    fn credentials(&self) -> BTreeSet<String> {
        self.tailnets
            .iter()
            .map(|t| self.slots.get(&t.id).unwrap_or(&t.id).clone())
            .collect()
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    before: Records,
    after: Records,
    created: Option<String>,
    retired: Option<String>,
}
impl Pending {
    fn validate(&self) -> Result<()> {
        self.before.validate()?;
        self.after.validate()?;
        let before = self.before.credentials();
        let after = self.after.credentials();
        if after.difference(&before).cloned().collect::<Vec<_>>() != self.created.iter().cloned().collect::<Vec<_>>()
            || before.difference(&after).cloned().collect::<Vec<_>>()
                != self.retired.iter().cloned().collect::<Vec<_>>()
        {
            return Err(Error::Storage);
        }
        Ok(())
    }
}
trait Credentials {
    fn put(&self, slot: &str, key: &str) -> Result<()>;
    fn delete(&self, slot: &str) -> Result<()>;
}
struct Native;
impl Credentials for Native {
    fn put(&self, slot: &str, key: &str) -> Result<()> {
        keychain::put(slot, key)
    }
    fn delete(&self, slot: &str) -> Result<()> {
        keychain::delete(slot)
    }
}

/// Machine-local metadata and write-only OS-store bindings.
/// Credential reads belong to the private host deployment adapter.
///
/// ```compile_fail
/// let store = horizon_cloud::tailnet::Store::new("settings".into());
/// store.enroll("work", |bytes| { println!("{bytes:?}"); Ok(()) });
/// ```
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}
/// Catalog ownership follows held cloud lifecycle locks, before metadata or credential mutation.
pub struct CatalogOwnership {
    store: Store,
    lock: File,
}

impl CatalogOwnership {
    /// # Errors
    /// Refuses corrupt metadata and unresolved credential generations.
    pub fn load(&self) -> Result<Catalog> {
        if let Some(pending) = self.store.read::<Pending>("tailnets.pending.json", 192 * 1024)? {
            pending.validate()?;
            return Err(Error::Storage);
        }
        self.store.load()
    }

    /// Caller retains every checked cloud lock through this operation.
    /// # Errors
    /// Missing binding or failed durable catalog and credential deletion.
    pub fn delete(&self, id: &str) -> Result<Catalog> {
        self.load()?;
        self.store.delete_locked_with(id, &Native)
    }
}

impl Drop for CatalogOwnership {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
}

impl Store {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    /// Acquire the existing root catalog mutation lock after cloud lifecycle ownership.
    /// # Errors
    /// Unavailable catalog storage or lock.
    pub fn own_catalog(&self) -> Result<CatalogOwnership> {
        Ok(CatalogOwnership {
            store: self.clone(),
            lock: self.lock()?,
        })
    }
    /// # Errors
    /// Refuses corrupt metadata; never contacts the credential store.
    pub fn load(&self) -> Result<Catalog> {
        Ok(self.records()?.catalog())
    }
    /// Nonsecret locator for the private host deployment adapter, including legacy bindings.
    /// # Errors
    /// Missing binding or corrupt metadata.
    pub fn credential_slot(&self, id: &str) -> Result<String> {
        self.records()?.slot(id)
    }
    fn records(&self) -> Result<Records> {
        let records = self.read::<Records>("tailnets.json", 64 * 1024)?.unwrap_or_default();
        records.validate()?;
        Ok(records)
    }
    fn read<T: serde::de::DeserializeOwned>(&self, name: &str, limit: usize) -> Result<Option<T>> {
        match std::fs::read(self.root.join(name)) {
            Ok(bytes) if bytes.len() <= limit => serde_json::from_slice(&bytes).map(Some).map_err(|_| Error::Storage),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            _ => Err(Error::Storage),
        }
    }
    /// Recover an interrupted write without reading any secret bytes.
    /// # Errors
    /// Unavailable keychain, conflicting/corrupt metadata or storage failure. The journal remains retryable.
    pub fn recover(&self) -> Result<()> {
        let _lock = self.lock()?;
        self.recover_with(&Native)
    }
    fn recover_with(&self, keys: &impl Credentials) -> Result<()> {
        let Some(pending) = self.read::<Pending>("tailnets.pending.json", 192 * 1024)? else {
            return Ok(());
        };
        pending.validate()?;
        let current = self.records()?;
        let obsolete = if current == pending.after {
            pending.retired
        } else if current == pending.before {
            pending.created
        } else {
            return Err(Error::Storage);
        };
        // Re-establish durable catalog ownership before deleting either generation.
        write(&self.root.join("tailnets.json"), &current)?;
        if let Some(slot) = obsolete {
            keys.delete(&slot)?;
        }
        std::fs::remove_file(self.root.join("tailnets.pending.json")).map_err(|_| Error::Storage)?;
        sync_directory(&self.root)
    }
    /// Save or replace only; saved keys are never loaded into Settings.
    /// # Errors
    /// Invalid input, unavailable keychain or failed durable metadata/cleanup.
    pub fn save(&self, id: Option<&str>, name: &str, key: &str) -> Result<Catalog> {
        self.save_with(id, name, key, &Native)
    }
    fn save_with(&self, id: Option<&str>, name: &str, key: &str, keys: &impl Credentials) -> Result<Catalog> {
        if !valid_name(name) || !valid_key(key) {
            return Err(Error::Invalid);
        }
        let _lock = self.lock()?;
        self.recover_with(keys)?;
        let before = self.records()?;
        let mut after = before.clone();
        let id = id.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
        if !valid_id(&id) {
            return Err(Error::Invalid);
        }
        let retired = before.slot(&id).ok();
        if let Some(t) = after.tailnets.iter_mut().find(|t| t.id == id) {
            t.name = name.trim().into();
        } else {
            after.tailnets.push(Tailnet {
                id: id.clone(),
                name: name.trim().into(),
            });
        }
        let created = uuid::Uuid::new_v4().to_string();
        after.slots.insert(id, created.clone());
        let pending = Pending {
            before,
            after,
            created: Some(created.clone()),
            retired,
        };
        pending.validate()?;
        write(&self.root.join("tailnets.pending.json"), &pending)?;
        keys.put(&created, key)?;
        write(&self.root.join("tailnets.json"), &pending.after)?;
        self.recover_with(keys)?;
        Ok(pending.after.catalog())
    }
    /// # Errors
    /// Missing binding or failed durable deletion/cleanup. Existing cloud assignments remain explicit missing selections.
    pub fn delete(&self, id: &str) -> Result<Catalog> {
        self.delete_with(id, &Native)
    }
    fn delete_with(&self, id: &str, keys: &impl Credentials) -> Result<Catalog> {
        let _lock = self.lock()?;
        self.delete_locked_with(id, keys)
    }
    fn delete_locked_with(&self, id: &str, keys: &impl Credentials) -> Result<Catalog> {
        self.recover_with(keys)?;
        let before = self.records()?;
        let retired = Some(before.slot(id)?);
        let mut after = before.clone();
        after.tailnets.retain(|t| t.id != id);
        after.slots.remove(id);
        let pending = Pending {
            before,
            after,
            created: None,
            retired,
        };
        pending.validate()?;
        write(&self.root.join("tailnets.pending.json"), &pending)?;
        write(&self.root.join("tailnets.json"), &pending.after)?;
        self.recover_with(keys)?;
        Ok(pending.after.catalog())
    }
    fn lock(&self) -> Result<File> {
        std::fs::create_dir_all(&self.root).map_err(|_| Error::Storage)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join("tailnets.lock"))
            .map_err(|_| Error::Storage)?;
        file.lock().map_err(|_| Error::Storage)?;
        Ok(file)
    }
}

#[cfg(test)]
mod tests;
