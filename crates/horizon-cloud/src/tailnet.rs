//! Auth-key-only cloud networking. Persistent metadata never contains a secret.
mod keychain;
/// OS-store namespace; credentials have no public read API.
pub const KEYCHAIN_SERVICE: &str = "horizon-cloud-tailnets";
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Use a name and a Tailscale auth key beginning with tskey-auth-.")]
    Invalid,
    #[error("The OS credential store is unavailable or locked. Unlock it and try again.")]
    Keychain,
    #[error("Tailnet settings could not be read or saved.")]
    Storage,
    #[error("The selected tailnet is unavailable. Choose a saved tailnet in Settings.")]
    Missing,
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Tailnet {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    #[serde(default)]
    pub tailnets: Vec<Tailnet>,
}

#[must_use]
pub fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
#[must_use]
pub fn valid_key(value: &str) -> bool {
    value.starts_with("tskey-auth-")
        && (31..=256).contains(&value.len())
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
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
impl Store {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    /// # Errors
    /// Refuses corrupt metadata instead of treating it as an empty catalog.
    pub fn load(&self) -> Result<Catalog> {
        let bytes = match std::fs::read(self.root.join("tailnets.json")) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Catalog::default()),
            Err(_) => return Err(Error::Storage),
        };
        if bytes.len() > 64 * 1024 {
            return Err(Error::Storage);
        }
        let catalog: Catalog = serde_json::from_slice(&bytes).map_err(|_| Error::Storage)?;
        let mut ids = std::collections::BTreeSet::new();
        if catalog
            .tailnets
            .iter()
            .any(|t| !valid_id(&t.id) || !valid_name(&t.name) || !ids.insert(&t.id))
        {
            return Err(Error::Storage);
        }
        Ok(catalog)
    }
    /// Save or replace only; saved keys are never loaded into Settings.
    /// # Errors
    /// Invalid input, unavailable keychain or failed metadata write.
    pub fn save(&self, id: Option<&str>, name: &str, key: &str) -> Result<Catalog> {
        if !valid_name(name) || !valid_key(key) {
            return Err(Error::Invalid);
        }
        let _lock = self.lock()?;
        let mut catalog = self.load()?;
        let id = id.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
        if !valid_id(&id) {
            return Err(Error::Invalid);
        }
        if let Some(t) = catalog.tailnets.iter_mut().find(|t| t.id == id) {
            t.name = name.trim().into();
        } else {
            catalog.tailnets.push(Tailnet {
                id: id.clone(),
                name: name.trim().into(),
            });
        }
        keychain::put(&id, key)?;
        write(&self.root.join("tailnets.json"), &catalog)?;
        Ok(catalog)
    }
    /// # Errors
    /// Missing binding, keychain or storage failure. Existing cloud assignments remain
    /// explicit missing selections; they never silently switch to another tailnet.
    pub fn delete(&self, id: &str) -> Result<Catalog> {
        let _lock = self.lock()?;
        let mut catalog = self.load()?;
        if !catalog.tailnets.iter().any(|t| t.id == id) {
            return Err(Error::Missing);
        }
        keychain::delete(id)?;
        catalog.tailnets.retain(|t| t.id != id);
        write(&self.root.join("tailnets.json"), &catalog)?;
        Ok(catalog)
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
fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 80 && !name.chars().any(char::is_control)
}
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub tailnet: Option<String>,
}
impl Selection {
    /// # Errors
    /// Rejects malformed persisted selections.
    pub fn load(cloud: &Path) -> Result<Self> {
        match std::fs::read(cloud.join("tailnet.json")) {
            Ok(bytes) if bytes.len() <= 1024 => {
                let selection: Self = serde_json::from_slice(&bytes).map_err(|_| Error::Storage)?;
                if selection.tailnet.as_deref().is_some_and(|s| !valid_id(s)) {
                    return Err(Error::Invalid);
                }
                Ok(selection)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            _ => Err(Error::Storage),
        }
    }
    /// Caller holds the cloud lifecycle lock.
    /// # Errors
    /// Unknown tailnet or failed durable selection write.
    pub fn save(cloud: &Path, id: Option<&str>, catalog: &Catalog) -> Result<()> {
        if id.is_some_and(|id| !catalog.tailnets.iter().any(|t| t.id == id)) {
            return Err(Error::Missing);
        }
        std::fs::create_dir_all(cloud).map_err(|_| Error::Storage)?;
        write(
            &cloud.join("tailnet.json"),
            &Self {
                tailnet: id.map(str::to_owned),
            },
        )
    }
}
fn write(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().ok_or(Error::Storage)?;
    let mut pending = tempfile::NamedTempFile::new_in(parent).map_err(|_| Error::Storage)?;
    let bytes = Zeroizing::new(serde_json::to_vec_pretty(value).map_err(|_| Error::Storage)?);
    pending.write_all(&bytes).map_err(|_| Error::Storage)?;
    pending.as_file().sync_all().map_err(|_| Error::Storage)?;
    pending.persist(path).map_err(|_| Error::Storage)?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error::Storage)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_or_secret_bearing_metadata_is_refused_and_selection_is_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::new(temp.path().into());
        assert!(store.load().unwrap().tailnets.is_empty());
        std::fs::write(
            temp.path().join("tailnets.json"),
            br#"{"tailnets":[],"auth_key":"synthetic"}"#,
        )
        .unwrap();
        assert!(store.load().is_err());
        let catalog = Catalog {
            tailnets: vec![Tailnet {
                id: "work".into(),
                name: "Work".into(),
            }],
        };
        Selection::save(temp.path(), Some("work"), &catalog).unwrap();
        assert_eq!(Selection::load(temp.path()).unwrap().tailnet.as_deref(), Some("work"));
        assert!(Selection::save(temp.path(), Some("unknown"), &catalog).is_err());
        Selection::save(temp.path(), None, &catalog).unwrap();
        assert_eq!(Selection::load(temp.path()).unwrap(), Selection::default());
        assert!(valid_key("tskey-auth-synthetic12345678901234567890"));
        assert!(!valid_key(&format!("tskey-auth-{}", "a".repeat(19))));
        assert!(valid_key(&format!("tskey-auth-{}", "a".repeat(20))));
        assert!(valid_key(&format!("tskey-auth-{}", "a".repeat(245))));
        assert!(!valid_key(&format!("tskey-auth-{}", "a".repeat(246))));
        for invalid in [
            "tskey-api-synthetic12345678901234567890",
            "tskey-auth-short",
            "tskey-auth-synthetic12345678901234567890\n",
        ] {
            assert!(!valid_key(invalid));
        }
    }
}
