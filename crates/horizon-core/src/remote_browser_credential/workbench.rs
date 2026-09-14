//! Credential entry state for the settings UI: synchronous session-only
//! values, and OS-store operations run on a worker thread so a locked store
//! or an unlock prompt never stalls the render loop.

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use horizon_browser::remote::{CredentialReference, CredentialStoreKind, RemoteProviderProfile};

use zeroize::Zeroizing;

use super::{
    CredentialLocator, CredentialReadiness, CredentialState, CredentialStores, KeyringCredentialStore,
    RemoteCredentialError, RemoteCredentialStore, Sealed, SessionCredentialStore, readiness,
};

/// Boxed store the worker thread owns; the allocation path borrows it later.
pub type SharedStore = Arc<Mutex<Box<dyn RemoteCredentialStore + Send>>>;

/// How the worker obtains the OS store. Injectable so tests use the fake.
pub type StoreOpener = Box<dyn FnOnce() -> Result<Box<dyn RemoteCredentialStore + Send>, RemoteCredentialError> + Send>;

/// Whether the OS store can be used right now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeychainState {
    Opening,
    Available,
    Unavailable(RemoteCredentialError),
}

/// Value-free outcome of one store operation, for the UI to show once.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkbenchNotice {
    pub provider: String,
    pub reference: CredentialReference,
    pub outcome: Result<NoticeKind, RemoteCredentialError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeKind {
    SessionValueSet,
    SessionValueCleared,
    StoredInKeychain,
    DeletedFromKeychain,
}

enum Command {
    Probe(Vec<CredentialLocator>),
    Put(CredentialLocator, Zeroizing<Vec<u8>>),
    Delete(CredentialLocator),
}

enum Event {
    Opened(Result<(), RemoteCredentialError>),
    Presence(CredentialLocator, Result<bool, RemoteCredentialError>),
    Stored(CredentialLocator, Result<(), RemoteCredentialError>),
    Deleted(CredentialLocator, Result<(), RemoteCredentialError>),
}

struct KeychainLink {
    commands: Sender<Command>,
    events: Receiver<Event>,
    store: SharedStore,
}

/// Session store plus a keychain worker, with a presence cache the UI reads.
pub struct CredentialWorkbench {
    session: SessionCredentialStore,
    keychain: Option<KeychainLink>,
    keychain_state: KeychainState,
    presence: BTreeMap<CredentialLocator, CredentialState>,
    labels: BTreeMap<CredentialLocator, (String, CredentialReference)>,
    notices: Vec<WorkbenchNotice>,
}

impl CredentialWorkbench {
    /// Open the platform store on a worker thread.
    #[must_use]
    pub fn spawn_platform() -> Self {
        Self::with_opener(Box::new(|| {
            KeyringCredentialStore::open().map(|store| Box::new(store) as Box<dyn RemoteCredentialStore + Send>)
        }))
    }

    /// Open whatever `opener` returns on a worker thread.
    #[must_use]
    pub fn with_opener(opener: StoreOpener) -> Self {
        let (command_tx, command_rx) = channel::<Command>();
        let (event_tx, event_rx) = channel::<Event>();
        let store: SharedStore = Arc::new(Mutex::new(Box::new(super::FakeCredentialStore::new())));
        let worker_store = Arc::clone(&store);
        thread::Builder::new()
            .name("remote-browser-keychain".into())
            .spawn(move || worker(opener, &worker_store, &command_rx, &event_tx))
            .ok();
        Self {
            session: SessionCredentialStore::new(),
            keychain: Some(KeychainLink {
                commands: command_tx,
                events: event_rx,
                store,
            }),
            keychain_state: KeychainState::Opening,
            presence: BTreeMap::new(),
            labels: BTreeMap::new(),
            notices: Vec::new(),
        }
    }

    /// Drain worker events. Call once per frame; cheap when idle.
    pub fn poll(&mut self) {
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(link) = &self.keychain {
            loop {
                match link.events.try_recv() {
                    Ok(event) => events.push(event),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        for event in events {
            match event {
                Event::Opened(Ok(())) => self.keychain_state = KeychainState::Available,
                Event::Opened(Err(error)) => self.keychain_state = KeychainState::Unavailable(error),
                Event::Presence(locator, result) => {
                    self.presence.insert(locator, state_from(&result));
                }
                Event::Stored(locator, result) => {
                    self.presence
                        .insert(locator.clone(), state_from(&result.clone().map(|()| true)));
                    self.notify(&locator, result.map(|()| NoticeKind::StoredInKeychain));
                }
                Event::Deleted(locator, result) => {
                    self.presence
                        .insert(locator.clone(), state_from(&result.clone().map(|()| false)));
                    self.notify(&locator, result.map(|()| NoticeKind::DeletedFromKeychain));
                }
            }
        }
        if disconnected {
            self.keychain_state = KeychainState::Unavailable(RemoteCredentialError::StoreUnavailable);
            self.keychain = None;
        }
    }

    #[must_use]
    pub fn keychain_state(&self) -> &KeychainState {
        &self.keychain_state
    }

    /// Readiness for display. Session values are answered immediately; OS-store
    /// values come from the presence cache and show `Checking` until probed.
    pub fn readiness(&mut self, provider: &str, profile: &RemoteProviderProfile) -> Vec<CredentialReadiness> {
        self.request_probe(provider, profile);
        let cache = CachedKeychain {
            presence: &self.presence,
            state: &self.keychain_state,
        };
        let stores = CredentialStores {
            session: &self.session,
            os_keychain: Some(&cache),
        };
        readiness(profile, &stores)
    }

    /// Hold a value for this process only.
    ///
    /// # Errors
    /// The reference must be bound to the session store and the value must be
    /// non-empty and bounded.
    pub fn set_session_value(
        &mut self,
        provider: &str,
        profile: &RemoteProviderProfile,
        reference: &CredentialReference,
        value: &[u8],
    ) -> Result<(), RemoteCredentialError> {
        let locator = self.locator(provider, profile, reference, CredentialStoreKind::Session)?;
        let result = self.session.put(&locator, value);
        self.notices.push(WorkbenchNotice {
            provider: provider.to_string(),
            reference: reference.clone(),
            outcome: result.clone().map(|()| NoticeKind::SessionValueSet),
        });
        result
    }

    /// Persist a value in the OS store on the worker thread.
    ///
    /// # Errors
    /// The reference must be bound to the OS store, which must be available.
    pub fn store_in_keychain(
        &mut self,
        provider: &str,
        profile: &RemoteProviderProfile,
        reference: &CredentialReference,
        value: &[u8],
    ) -> Result<(), RemoteCredentialError> {
        super::validate_secret(value)?;
        let locator = self.locator(provider, profile, reference, CredentialStoreKind::OsKeychain)?;
        let commands = self.available_link()?.commands.clone();
        self.presence.insert(locator.clone(), CredentialState::Checking);
        commands
            .send(Command::Put(locator, Zeroizing::new(value.to_vec())))
            .map_err(|_| RemoteCredentialError::StoreUnavailable)
    }

    /// Forget a value wherever its binding says it lives.
    ///
    /// # Errors
    /// Unbound references and an unavailable OS store.
    pub fn delete(
        &mut self,
        provider: &str,
        profile: &RemoteProviderProfile,
        reference: &CredentialReference,
    ) -> Result<(), RemoteCredentialError> {
        let binding = profile
            .credential_bindings
            .get(reference)
            .ok_or(RemoteCredentialError::Missing)?;
        let locator = CredentialLocator::new(&profile.endpoint, reference, binding);
        self.labels
            .insert(locator.clone(), (provider.to_string(), reference.clone()));
        match binding.store {
            CredentialStoreKind::Session => {
                let result = self.session.delete(&locator);
                self.notices.push(WorkbenchNotice {
                    provider: provider.to_string(),
                    reference: reference.clone(),
                    outcome: result.clone().map(|()| NoticeKind::SessionValueCleared),
                });
                result
            }
            CredentialStoreKind::OsKeychain => {
                let commands = self.available_link()?.commands.clone();
                self.presence.insert(locator.clone(), CredentialState::Checking);
                commands
                    .send(Command::Delete(locator))
                    .map_err(|_| RemoteCredentialError::StoreUnavailable)
            }
        }
    }

    /// Drop every session-only value, overwriting buffers first.
    pub fn clear_session(&mut self) {
        self.session.clear();
    }

    #[must_use]
    pub fn session_value_count(&self) -> usize {
        self.session.len()
    }

    /// Outcomes accumulated since the last call, oldest first.
    pub fn take_notices(&mut self) -> Vec<WorkbenchNotice> {
        std::mem::take(&mut self.notices)
    }

    /// Stores for allocation-time resolution. The OS store is only offered once opened.
    #[must_use]
    pub fn session_store(&self) -> &SessionCredentialStore {
        &self.session
    }

    #[must_use]
    pub fn keychain_store(&self) -> Option<SharedStore> {
        match (&self.keychain, &self.keychain_state) {
            (Some(link), KeychainState::Available) => Some(Arc::clone(&link.store)),
            _ => None,
        }
    }

    fn request_probe(&mut self, provider: &str, profile: &RemoteProviderProfile) {
        let Some(commands) = self.keychain.as_ref().map(|link| link.commands.clone()) else {
            return;
        };
        if self.keychain_state != KeychainState::Available {
            return;
        }
        let mut wanted = Vec::new();
        for reference in profile.authentication.references() {
            let Some(binding) = profile.credential_bindings.get(reference) else {
                continue;
            };
            if binding.store != CredentialStoreKind::OsKeychain {
                continue;
            }
            let locator = CredentialLocator::new(&profile.endpoint, reference, binding);
            if !self.presence.contains_key(&locator) {
                self.presence.insert(locator.clone(), CredentialState::Checking);
                self.labels
                    .insert(locator.clone(), (provider.to_string(), reference.clone()));
                wanted.push(locator);
            }
        }
        if !wanted.is_empty() {
            commands.send(Command::Probe(wanted)).ok();
        }
    }

    fn locator(
        &mut self,
        provider: &str,
        profile: &RemoteProviderProfile,
        reference: &CredentialReference,
        expected: CredentialStoreKind,
    ) -> Result<CredentialLocator, RemoteCredentialError> {
        let binding = profile
            .credential_bindings
            .get(reference)
            .ok_or(RemoteCredentialError::Missing)?;
        if binding.store != expected {
            return Err(RemoteCredentialError::StoreUnavailable);
        }
        let locator = CredentialLocator::new(&profile.endpoint, reference, binding);
        self.labels
            .insert(locator.clone(), (provider.to_string(), reference.clone()));
        Ok(locator)
    }

    fn available_link(&self) -> Result<&KeychainLink, RemoteCredentialError> {
        match (&self.keychain, &self.keychain_state) {
            (Some(link), KeychainState::Available) => Ok(link),
            (_, KeychainState::Unavailable(error)) => Err(error.clone()),
            _ => Err(RemoteCredentialError::StoreUnavailable),
        }
    }

    fn notify(&mut self, locator: &CredentialLocator, outcome: Result<NoticeKind, RemoteCredentialError>) {
        if let Some((provider, reference)) = self.labels.get(locator) {
            self.notices.push(WorkbenchNotice {
                provider: provider.clone(),
                reference: reference.clone(),
                outcome,
            });
        }
    }
}

fn state_from(result: &Result<bool, RemoteCredentialError>) -> CredentialState {
    match result {
        Ok(true) => CredentialState::Present,
        Ok(false) | Err(RemoteCredentialError::Missing) => CredentialState::Missing,
        Err(RemoteCredentialError::Locked) => CredentialState::Locked,
        Err(_) => CredentialState::StoreUnavailable,
    }
}

/// Read-only view of the presence cache in store form, for [`readiness`].
struct CachedKeychain<'a> {
    presence: &'a BTreeMap<CredentialLocator, CredentialState>,
    state: &'a KeychainState,
}

impl Sealed for CachedKeychain<'_> {}

impl RemoteCredentialStore for CachedKeychain<'_> {
    fn kind(&self) -> CredentialStoreKind {
        CredentialStoreKind::OsKeychain
    }

    fn put(&mut self, _: &CredentialLocator, _: &[u8]) -> Result<(), RemoteCredentialError> {
        Err(RemoteCredentialError::StoreUnavailable)
    }

    fn delete(&mut self, _: &CredentialLocator) -> Result<(), RemoteCredentialError> {
        Err(RemoteCredentialError::StoreUnavailable)
    }

    fn contains(&self, locator: &CredentialLocator) -> Result<bool, RemoteCredentialError> {
        match self.state {
            KeychainState::Unavailable(error) => return Err(error.clone()),
            KeychainState::Opening => return Err(RemoteCredentialError::Checking),
            KeychainState::Available => {}
        }
        match self.presence.get(locator) {
            Some(CredentialState::Present) => Ok(true),
            Some(CredentialState::Missing) => Ok(false),
            Some(CredentialState::Locked) => Err(RemoteCredentialError::Locked),
            Some(CredentialState::StoreUnavailable) => Err(RemoteCredentialError::StoreUnavailable),
            Some(CredentialState::Checking) | None => Err(RemoteCredentialError::Checking),
        }
    }

    fn with_secret(&self, _: &CredentialLocator, _: &mut dyn super::SecretSink) -> Result<(), RemoteCredentialError> {
        Err(RemoteCredentialError::StoreUnavailable)
    }
}

fn worker(opener: StoreOpener, shared: &SharedStore, commands: &Receiver<Command>, events: &Sender<Event>) {
    match opener() {
        Ok(store) => {
            if let Ok(mut slot) = shared.lock() {
                *slot = store;
            }
            events.send(Event::Opened(Ok(()))).ok();
        }
        Err(error) => {
            events.send(Event::Opened(Err(error))).ok();
            return;
        }
    }
    while let Ok(command) = commands.recv() {
        let Ok(mut store) = shared.lock() else { return };
        let outcome = match command {
            Command::Probe(locators) => {
                for locator in locators {
                    let result = store.contains(&locator);
                    if events.send(Event::Presence(locator, result)).is_err() {
                        return;
                    }
                }
                continue;
            }
            Command::Put(locator, value) => {
                let result = store.put(&locator, &value);
                Event::Stored(locator, result)
            }
            Command::Delete(locator) => {
                let result = store.delete(&locator);
                Event::Deleted(locator, result)
            }
        };
        if events.send(outcome).is_err() {
            return;
        }
    }
}
