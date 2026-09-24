//! Owning-host registration and anchored provider journal for the shared protocol.
//! This API performs no provider I/O and is not connected to runtime entry points.
mod directory;
mod journal;
mod machine;
mod registration;
mod vault;

use directory::Directory;
use ed25519_dalek::{Signer, SigningKey};
use horizon_cloud_protocol::{
    AllocationId, ControllerId,
    signed::{ControllerBinding, Intent, SignedIntent},
};
use journal::{Anchor, CANDIDATE, JOURNAL, Journal, Lock, MARKER, Marker};
use machine::MachineId;
use registration::{Registration, Transition};
use ring::rand::{SecureRandom, SystemRandom};
use std::path::{Path, PathBuf};
use vault::{NativeVault, Vault};

type Result<T> = std::result::Result<T, Error>;
type MachineReader = Box<dyn Fn() -> Result<MachineId>>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Native machine identity is unavailable")]
    Machine,
    #[error("Cloud controller OS credential store is unavailable")]
    Store,
    #[error("Cloud controller registration is missing")]
    MissingRegistration,
    #[error("Cloud controller registration is invalid or uncertain")]
    Registration,
    #[error("Cloud journal is not owned by this host and location")]
    Ownership,
    #[error("Cloud journal is missing, changed or rolled back")]
    Journal,
    #[error("Another process owns this allocation lock")]
    Busy,
    #[error("Cloud journal I/O failed")]
    Io(#[from] std::io::Error),
    #[error("Cloud management request cannot be signed")]
    Signature,
}

/// Holds the canonical allocation lock. Keep it through provider intent, I/O and
/// verified completion. UI/CLI/MCP must validate caller/project ownership first.
/// There is no key import, ownership transfer or file-store fallback.
pub struct Owner {
    root: PathBuf,
    directory: Directory,
    lock_path: PathBuf,
    marker: Marker,
    machine: MachineReader,
    vault: Box<dyn Vault>,
    ready: bool,
    lock: Lock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Boundary {
    Candidate,
    Pending,
    Published,
    Committed,
}

impl Owner {
    /// Create a new allocation identity and register a new, absent journal directory.
    /// Existing directories are never adopted, even when empty or missing state.
    ///
    /// # Errors
    /// Blocks unsupported hosts, unavailable stores and uncertain durable writes.
    pub fn create(root: &Path, payload: serde_json::Value) -> Result<Self> {
        Self::create_with(
            root,
            &crate::horizon_home::HorizonHome::resolve()
                .root()
                .join("cloud-controller-locks"),
            payload,
            Box::new(NativeVault::open()?),
            Box::new(MachineId::read),
            &mut |_| Ok(()),
        )
    }

    /// Verify the native host, registration, canonical path and anchored journal.
    /// Recovery can complete only a pending transition already in the OS store.
    ///
    /// # Errors
    /// Refuses copies, rollbacks, missing registrations and conflicting recovery state.
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_with(root, Box::new(NativeVault::open()?), Box::new(MachineId::read))
    }

    /// # Errors
    /// Rechecks the native registration and anchored journal before returning public identity.
    pub fn binding(&self) -> Result<ControllerBinding> {
        let (registration, _) = self.current()?;
        let key = registration.verify(&self.root, &self.marker, &(self.machine)()?)?;
        let public = key.verifying_key().to_bytes();
        Ok(ControllerBinding::new(
            self.marker.allocation,
            self.marker.controller,
            public,
        ))
    }

    /// # Errors
    /// Refuses an unanchored, missing or rolled-back journal, including after a failed save.
    pub fn load(&self) -> Result<serde_json::Value> {
        self.current().map(|(_, journal)| journal.payload)
    }

    /// # Errors
    /// Any uncertain boundary poisons this handle; reopen to reconcile before further actions.
    pub fn save(&mut self, payload: serde_json::Value) -> Result<()> {
        self.save_with(payload, &mut |_| Ok(()))
    }

    /// Sign only after local caller ownership and typed action validation. This
    /// method does not grant project membership or permission for provider I/O.
    /// The expected revision belongs to the worker's membership manifest, not
    /// this host journal's generation; the worker must check it under its lock.
    ///
    /// # Errors
    /// Rechecks native registration and journal freshness and rejects mismatched intents.
    pub fn sign(&self, intent: Intent) -> Result<SignedIntent> {
        let (registration, _) = self.current()?;
        let key = registration.verify(&self.root, &self.marker, &(self.machine)()?)?;
        SignedIntent::sign_with(intent, &self.binding()?, |bytes| key.sign(bytes).to_bytes().to_vec())
            .map_err(|_| Error::Signature)
    }

    fn create_with(
        root: &Path,
        lock_root: &Path,
        payload: serde_json::Value,
        vault: Box<dyn Vault>,
        machine: MachineReader,
        checkpoint: &mut impl FnMut(Boundary) -> Result<()>,
    ) -> Result<Self> {
        let machine_id = machine()?;
        crate::session_store::require_directory_durability()?;
        let mut key = zeroize::Zeroizing::new([0_u8; 32]);
        SystemRandom::new()
            .fill(key.as_mut())
            .map_err(|_| Error::Registration)?;
        let decoded = SigningKey::from_bytes(&key);
        let marker = Marker {
            version: 1,
            allocation: AllocationId::generate(),
            controller: ControllerId::generate(),
            registration: uuid::Uuid::new_v4(),
            public_key_hash: journal::hash(decoded.verifying_key().as_bytes()),
        };
        match vault.read(&marker.registration.to_string()) {
            Err(Error::MissingRegistration) => {}
            Err(error) => return Err(error),
            Ok(_) => return Err(Error::Registration),
        }
        let directory = Directory::create(root, || {})?;
        let root = directory.path().to_owned();
        let lock_root = journal::create_lock_root(lock_root)?;
        if lock_root.starts_with(&root) {
            return Err(Error::Ownership);
        }
        let lock_path = lock_root.join(format!("{}.lock", marker.registration));
        let lock = Lock::acquire(&lock_path, true)?;
        directory.write(MARKER, &serde_json::to_vec(&marker).map_err(|_| Error::Journal)?)?;
        let mut registration = Registration {
            version: 1,
            machine: machine_id,
            root: root.clone(),
            lock_path: lock_path.clone(),
            lock_identity: lock.identity()?,
            marker: marker.clone(),
            key: zeroize::Zeroizing::new(key.as_ref().to_vec()),
            committed: None,
            pending: None,
        };
        let mut owner = Self {
            root,
            directory,
            lock_path,
            marker,
            machine,
            vault,
            ready: false,
            lock,
        };
        owner.advance(&mut registration, payload, checkpoint)?;
        owner.ready = true;
        Ok(owner)
    }

    fn open_with(root: &Path, vault: Box<dyn Vault>, machine: MachineReader) -> Result<Self> {
        crate::session_store::require_directory_durability()?;
        let root = root.canonicalize()?;
        let directory = Directory::open(&root)?;
        let marker: Marker = serde_json::from_slice(&directory.read(MARKER)?).map_err(|_| Error::Journal)?;
        if marker.version != 1 || marker.registration.is_nil() {
            return Err(Error::Journal);
        }
        let registration = Registration::read(vault.as_ref(), &marker)?;
        registration.verify(&root, &marker, &machine()?)?;
        let lock = Lock::acquire(&registration.lock_path, false)?;
        lock.verify(&registration.lock_path, registration.lock_identity)?;
        let mut owner = Self {
            root,
            directory,
            lock_path: registration.lock_path.clone(),
            marker,
            machine,
            vault,
            ready: false,
            lock,
        };
        let mut registration = Registration::read(owner.vault.as_ref(), &owner.marker)?;
        registration.verify(&owner.root, &owner.marker, &(owner.machine)()?)?;
        if registration.lock_path != owner.lock_path {
            return Err(Error::Ownership);
        }
        owner.lock.verify(&owner.lock_path, registration.lock_identity)?;
        owner.recover(&mut registration, &mut |_| Ok(()))?;
        owner.ready = true;
        owner.current()?;
        Ok(owner)
    }

    fn current(&self) -> Result<(Registration, Journal)> {
        if !self.ready {
            return Err(Error::Registration);
        }
        let marker: Marker = serde_json::from_slice(&self.directory.read(MARKER)?).map_err(|_| Error::Journal)?;
        if marker != self.marker {
            return Err(Error::Ownership);
        }
        let registration = Registration::read(self.vault.as_ref(), &self.marker)?;
        registration.verify(&self.root, &self.marker, &(self.machine)()?)?;
        if registration.lock_path != self.lock_path {
            return Err(Error::Ownership);
        }
        self.lock.verify(&self.lock_path, registration.lock_identity)?;
        if registration.pending.is_some() {
            return Err(Error::Registration);
        }
        let anchor = registration.committed.as_ref().ok_or(Error::Registration)?;
        let journal = journal::decode(&self.directory.read(JOURNAL)?, &self.marker, anchor)?;
        Ok((registration, journal))
    }

    fn save_with(
        &mut self,
        payload: serde_json::Value,
        checkpoint: &mut impl FnMut(Boundary) -> Result<()>,
    ) -> Result<()> {
        let (mut registration, _) = self.current()?;
        self.ready = false;
        self.advance(&mut registration, payload, checkpoint)?;
        self.ready = true;
        Ok(())
    }

    fn advance(
        &self,
        registration: &mut Registration,
        payload: serde_json::Value,
        checkpoint: &mut impl FnMut(Boundary) -> Result<()>,
    ) -> Result<()> {
        let previous = registration.committed.as_ref().map(|_| registration.clone());
        let generation = match &registration.committed {
            Some(anchor) => anchor.generation.checked_add(1).ok_or(Error::Registration)?,
            None => 0,
        };
        let journal = Journal {
            owner: self.marker.clone(),
            generation,
            payload,
        };
        let bytes = serde_json::to_vec(&journal).map_err(|_| Error::Journal)?;
        let next = Anchor {
            generation,
            hash: journal::hash(&bytes),
        };
        self.directory.write(CANDIDATE, &bytes)?;
        checkpoint(Boundary::Candidate)?;
        registration.pending = Some(Transition {
            previous: registration.committed.clone(),
            next: next.clone(),
        });
        self.register(registration, previous.as_ref())?;
        let pending = registration.clone();
        checkpoint(Boundary::Pending)?;
        self.directory.write(JOURNAL, &bytes)?;
        checkpoint(Boundary::Published)?;
        registration.committed = Some(next);
        registration.pending = None;
        self.register(registration, Some(&pending))?;
        checkpoint(Boundary::Committed)
    }

    fn register(&self, next: &Registration, previous: Option<&Registration>) -> Result<()> {
        next.verify(&self.root, &self.marker, &(self.machine)()?)?;
        self.lock.verify(&self.lock_path, next.lock_identity)?;
        let marker: Marker = serde_json::from_slice(&self.directory.read(MARKER)?).map_err(|_| Error::Journal)?;
        if marker != self.marker || next.lock_path != self.lock_path {
            return Err(Error::Ownership);
        }
        if let Some(pending) = &next.pending {
            journal::decode(&self.directory.read(CANDIDATE)?, &self.marker, &pending.next)?;
        }
        match &next.committed {
            Some(anchor) => {
                journal::decode(&self.directory.read(JOURNAL)?, &self.marker, anchor)?;
            }
            None => match self.directory.read(JOURNAL) {
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(Error::Journal),
            },
        }
        // The canonical lock serializes cooperative writers; never recreate a
        // missing native predecessor from cached signing material.
        match (previous, Registration::read(self.vault.as_ref(), &self.marker)) {
            (Some(expected), Ok(current)) if current == *expected => {}
            (None, Err(Error::MissingRegistration)) if next.committed.is_none() && next.pending.is_some() => {}
            (_, Err(error)) => return Err(error),
            _ => return Err(Error::Registration),
        }
        next.write(self.vault.as_ref())
    }

    fn recover(
        &self,
        registration: &mut Registration,
        checkpoint: &mut impl FnMut(Boundary) -> Result<()>,
    ) -> Result<()> {
        let Some(pending) = &registration.pending else {
            return Ok(());
        };
        let previous = registration.clone();
        let candidate = self.directory.read(CANDIDATE)?;
        journal::decode(&candidate, &self.marker, &pending.next)?;
        match self.directory.read(JOURNAL) {
            Ok(bytes) if journal::hash(&bytes) == pending.next.hash => {
                journal::decode(&bytes, &self.marker, &pending.next)?;
            }
            Ok(bytes) => {
                let previous = pending.previous.as_ref().ok_or(Error::Journal)?;
                journal::decode(&bytes, &self.marker, previous)?;
            }
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound && pending.previous.is_none() => {}
            _ => return Err(Error::Journal),
        }
        // A visible rename may still need its durability barrier after a crash.
        self.directory.write(JOURNAL, &candidate)?;
        checkpoint(Boundary::Published)?;
        registration.committed = Some(pending.next.clone());
        registration.pending = None;
        self.register(registration, Some(&previous))
    }
}

#[cfg(all(test, unix))]
mod tests;
