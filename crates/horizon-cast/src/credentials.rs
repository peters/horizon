use crate::{Error, Result, crypto};
use plist::{Dictionary, Value};
use ring::{digest, signature};
use std::{fs, io::Write, path::PathBuf};
use zeroize::Zeroizing;

/// Long-term pairing identity. Never contains the onscreen PIN or session keys.
#[derive(Clone)]
pub struct PairingCredentials {
    pub(crate) client_id: String,
    pub(crate) client_seed: Zeroizing<[u8; 32]>,
    pub(crate) receiver_id: Vec<u8>,
    pub(crate) receiver_key: Vec<u8>,
}
impl PairingCredentials {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            client_id: uuid::Uuid::new_v4().to_string().to_uppercase(),
            client_seed: crypto::random()?,
            receiver_id: Vec::new(),
            receiver_key: Vec::new(),
        })
    }
    pub(crate) fn signing(&self) -> Result<signature::Ed25519KeyPair> {
        signature::Ed25519KeyPair::from_seed_unchecked(self.client_seed.as_ref()).map_err(|_| Error::Authentication)
    }
}

/// Linux private pairing file for one stable discovered receiver ID.
/// The host supplies its profile directory; nothing is written by default.
#[derive(Clone, Debug)]
pub struct PairingStore {
    directory: PathBuf,
    device_id: String,
    name: String,
}
/// Remembered receiver metadata. Contains no pairing keys.
#[derive(Clone, Debug)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
}
pub(crate) struct ReceiverLease(fs::File);
impl Drop for ReceiverLease {
    fn drop(&mut self) {
        // A concurrent process spawn may briefly inherit this descriptor before exec.
        // Release ownership now rather than waiting for every inherited copy to close.
        let _ = self.0.unlock();
    }
}

struct SavedData(Value);
impl Drop for SavedData {
    fn drop(&mut self) {
        if let Some(Value::Data(seed)) = self.0.as_dictionary_mut().and_then(|data| data.get_mut("clientSeed")) {
            zeroize::Zeroize::zeroize(seed);
        }
    }
}
impl PairingStore {
    #[must_use]
    pub fn new(directory: PathBuf, device_id: String, name: String) -> Self {
        Self {
            directory,
            device_id,
            name,
        }
    }
    fn path(&self) -> PathBuf {
        let hash = digest::digest(&digest::SHA256, self.device_id.as_bytes());
        let name = hash.as_ref().iter().fold(String::with_capacity(64), |mut name, byte| {
            use std::fmt::Write;
            let _ = write!(name, "{byte:02x}");
            name
        });
        self.directory.join(format!("{name}.plist"))
    }
    pub(crate) fn reserve(&self) -> Result<ReceiverLease> {
        prepare_directory(&self.directory)?;
        let path = self.path().with_extension("lock");
        let file = private_lock_file(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(ReceiverLease(file)),
            Err(std::fs::TryLockError::WouldBlock) => Err(Error::AlreadyCasting),
            Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }
    /// Read a saved identity, without connecting to the TV.
    /// # Errors
    /// Rejects insecure permissions, malformed files and unknown versions.
    pub fn load(&self) -> Result<Option<PairingCredentials>> {
        let path = self.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        private_file(&metadata)?;
        let bytes = Zeroizing::new(fs::read(path)?);
        let value = SavedData(Value::from_reader(std::io::Cursor::new(bytes.as_slice()))?);
        let data = value
            .0
            .as_dictionary()
            .ok_or(Error::Protocol("invalid saved pairing"))?;
        let text = |name| {
            data.get(name)
                .and_then(Value::as_string)
                .ok_or(Error::Protocol("invalid saved pairing"))
        };
        let bytes = |name| {
            data.get(name)
                .and_then(Value::as_data)
                .ok_or(Error::Protocol("invalid saved pairing"))
        };
        if data.get("version").and_then(Value::as_unsigned_integer) != Some(1)
            || text("deviceID")? != self.device_id
            || text("clientID")?.is_empty()
            || bytes("receiverID")?.is_empty()
            || bytes("receiverKey")?.len() != 32
        {
            return Err(Error::Protocol("invalid saved pairing"));
        }
        let seed = bytes("clientSeed")?
            .try_into()
            .map_err(|_| Error::Protocol("invalid saved pairing"))?;
        Ok(Some(PairingCredentials {
            client_id: text("clientID")?.to_owned(),
            client_seed: Zeroizing::new(seed),
            receiver_id: bytes("receiverID")?.to_vec(),
            receiver_key: bytes("receiverKey")?.to_vec(),
        }))
    }
    pub(crate) fn save(&self, credentials: &PairingCredentials) -> Result<()> {
        prepare_directory(&self.directory)?;
        let mut data = Dictionary::new();
        data.insert("version".into(), 1u64.into());
        data.insert("deviceID".into(), self.device_id.clone().into());
        data.insert("name".into(), self.name.clone().into());
        data.insert("clientID".into(), credentials.client_id.clone().into());
        data.insert("clientSeed".into(), Value::Data(credentials.client_seed.to_vec()));
        data.insert("receiverID".into(), Value::Data(credentials.receiver_id.clone()));
        data.insert("receiverKey".into(), Value::Data(credentials.receiver_key.clone()));
        let value = SavedData(Value::Dictionary(data));
        let mut bytes = Zeroizing::new(Vec::new());
        value.0.to_writer_binary(&mut *bytes)?;
        let temporary = self.directory.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = private_create(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, self.path())?;
            Ok(())
        })();
        let _ = fs::remove_file(temporary);
        result
    }
    /// List stored TVs, including offline devices, without exposing credentials.
    /// # Errors
    /// Rejects malformed or insecure entries rather than returning their content.
    pub fn list(directory: &std::path::Path) -> Result<Vec<PairedDevice>> {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut devices = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "plist") {
                continue;
            }
            if devices.len() >= 256 {
                return Err(Error::Protocol("too many saved pairings"));
            }
            private_file(&fs::symlink_metadata(&path)?)?;
            let bytes = Zeroizing::new(fs::read(&path)?);
            let value = SavedData(Value::from_reader(std::io::Cursor::new(bytes.as_slice()))?);
            let data = value
                .0
                .as_dictionary()
                .ok_or(Error::Protocol("invalid saved pairing"))?;
            let text = |key| {
                data.get(key)
                    .and_then(Value::as_string)
                    .ok_or(Error::Protocol("invalid saved pairing"))
            };
            let id = text("deviceID")?.to_owned();
            let name = text("name")?.to_owned();
            let store = Self::new(directory.to_path_buf(), id.clone(), name.clone());
            if store.path() != path || store.load()?.is_none() {
                return Err(Error::Protocol("invalid saved pairing"));
            }
            devices.push(PairedDevice { id, name });
        }
        devices.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(devices)
    }
    /// Forget only this TV's pairing. Does not connect or start another session.
    /// # Errors
    /// Returns filesystem errors; a missing pairing is already forgotten.
    pub fn forget(&self) -> Result<()> {
        let _reservation = self.reserve()?;
        match fs::remove_file(self.path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(unix)]
fn private_lock_file(path: &std::path::Path) -> Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        private_file(&metadata)?;
    }
    Ok(fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?)
}
#[cfg(not(unix))]
fn private_lock_file(_: &std::path::Path) -> Result<fs::File> {
    Err(Error::Protocol("persistent pairing requires Linux"))
}

#[cfg(unix)]
fn private_file(metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 || metadata.len() > 4096 {
        return Err(Error::Protocol("saved pairing must be a private regular file"));
    }
    Ok(())
}
#[cfg(unix)]
fn prepare_directory(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    fs::DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(Error::Protocol("pairing directory must be private"));
    }
    Ok(())
}
#[cfg(unix)]
fn private_create(path: &std::path::Path) -> Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    Ok(fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}
#[cfg(not(unix))]
fn private_file(_: &fs::Metadata) -> Result<()> {
    Err(Error::Protocol("persistent pairing requires Linux"))
}
#[cfg(not(unix))]
fn prepare_directory(_: &std::path::Path) -> Result<()> {
    Err(Error::Protocol("persistent pairing requires Linux"))
}
#[cfg(not(unix))]
fn private_create(_: &std::path::Path) -> Result<fs::File> {
    Err(Error::Protocol("persistent pairing requires Linux"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn fixture() -> PairingCredentials {
        PairingCredentials {
            client_id: "synthetic-client".into(),
            client_seed: Zeroizing::new([7; 32]),
            receiver_id: b"synthetic-receiver".to_vec(),
            receiver_key: vec![9; 32],
        }
    }
    #[test]
    fn remembers_two_offline_tvs_and_forgets_only_the_selected_one() {
        let home = tempfile::tempdir().expect("isolated home");
        let directory = home.path().join("pairings");
        let first = PairingStore::new(directory.clone(), "tv/one".into(), "First TV".into());
        let second = PairingStore::new(directory.clone(), "tv/two".into(), "Second TV".into());
        assert!(first.load().expect("missing").is_none());
        assert!(PairingStore::list(&directory).expect("empty").is_empty());
        first.save(&fixture()).expect("first pairing");
        second.save(&fixture()).expect("second pairing");
        assert_eq!(
            fs::metadata(first.path()).expect("file").permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&directory).expect("directory").permissions().mode() & 0o777,
            0o700
        );
        let loaded = first.load().expect("read saved pairing").expect("pairing");
        assert_eq!(loaded.client_id, "synthetic-client");
        assert_eq!(*loaded.client_seed, [7; 32]);
        assert_eq!(loaded.receiver_key, [9; 32]);
        let devices = PairingStore::list(&directory).expect("metadata");
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].name, "First TV");
        first.forget().expect("forget first");
        first.forget().expect("repeat forget");
        assert!(first.load().expect("forgotten").is_none());
        assert!(second.load().expect("other TV retained").is_some());
        assert_eq!(PairingStore::list(&directory).expect("remaining")[0].name, "Second TV");
    }
    #[test]
    fn rejects_exposed_corrupt_symlinked_and_misbound_pairings() {
        let home = tempfile::tempdir().expect("isolated home");
        let directory = home.path().join("pairings");
        let store = PairingStore::new(directory.clone(), "first".into(), "TV".into());
        store.save(&fixture()).expect("save");
        fs::set_permissions(store.path(), fs::Permissions::from_mode(0o644)).expect("expose fixture");
        assert!(store.load().is_err());
        store.save(&fixture()).expect("atomic replace");
        let other = PairingStore::new(directory.clone(), "other".into(), "Other".into());
        fs::copy(store.path(), other.path()).expect("misbind fixture");
        assert!(other.load().is_err());
        other.forget().expect("remove copy");
        symlink(store.path(), other.path()).expect("link fixture");
        assert!(other.load().is_err());
        other.forget().expect("remove link only");
        assert!(store.load().expect("original retained").is_some());
        fs::write(store.path(), b"truncated").expect("corrupt fixture");
        assert!(store.load().is_err());
    }
    #[test]
    fn descriptor_copies_cannot_extend_the_receiver_lease_lifetime() {
        let home = tempfile::tempdir().expect("private home");
        let store = PairingStore::new(home.path().join("pairings"), "first".into(), "First".into());
        store.save(&fixture()).expect("save");
        let lease = store.reserve().expect("owner");
        let inherited = lease.0.try_clone().expect("descriptor copied during process spawn");
        assert!(matches!(store.reserve(), Err(Error::AlreadyCasting)));
        drop(lease);
        store.forget().expect("owner ended despite descriptor copy");
        drop(inherited);
    }

    #[test]
    fn receiver_file_lease_blocks_competing_start_and_forget_until_release() {
        let home = tempfile::tempdir().expect("private home");
        let first = PairingStore::new(home.path().join("pairings"), "first".into(), "First".into());
        let second = PairingStore::new(home.path().join("pairings"), "second".into(), "Second".into());
        first.save(&fixture()).expect("save");
        let copy = first.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _lease = copy.reserve().expect("first caller");
            ready_tx.send(()).expect("ready");
            stop_rx.recv().expect("release");
        });
        ready_rx.recv().expect("first owns lease");
        assert!(matches!(first.reserve(), Err(Error::AlreadyCasting)));
        assert!(matches!(first.forget(), Err(Error::AlreadyCasting)));
        let independent = second.reserve().expect("other TV remains usable");
        stop_tx.send(()).expect("stop");
        worker.join().expect("worker stopped");
        first.forget().expect("released TV can be forgotten");
        drop(independent);
        assert!(second.reserve().is_ok());
    }
}
