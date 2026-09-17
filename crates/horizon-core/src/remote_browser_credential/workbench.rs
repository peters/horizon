//! Credential entry state for the settings UI: synchronous session-only
//! values, and OS-store operations run on a worker thread so a locked store
//! or an unlock prompt never stalls the render loop.

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use horizon_browser::remote::{CredentialReference, CredentialStoreKind, RemoteBrowserConfig, RemoteProviderProfile};

use zeroize::Zeroizing;

use super::{
    CredentialLocator, CredentialReadiness, CredentialState, CredentialStores, EnvironmentCredentialStore,
    KeyringCredentialStore, RemoteCredentialError, RemoteCredentialStore, Sealed, SessionCredentialStore, readiness,
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

/// Value-free outcome of one store operation, for the UI to show once. The
/// attempted operation is kept on failure too, so a failed delete is never
/// described as a failed save.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkbenchNotice {
    pub provider: String,
    pub reference: CredentialReference,
    /// The destination the operation went to (see [`credential_destination`]),
    /// so a row whose binding changed while an operation was pending does
    /// not show that operation's outcome as its own.
    pub destination: String,
    pub kind: NoticeKind,
    pub error: Option<RemoteCredentialError>,
}

impl WorkbenchNotice {
    fn new(row: &Row, kind: NoticeKind, result: Result<(), RemoteCredentialError>) -> Self {
        Self {
            provider: row.provider.clone(),
            reference: row.reference.clone(),
            destination: row.destination.clone(),
            kind,
            error: result.err(),
        }
    }
}

/// Where a value for `reference` would go: the endpoint origin plus the
/// binding's store and slot (`origin|store|slot`), or `origin|unbound`. The
/// settings UI keys its drafts and matches notices by this string.
#[must_use]
pub fn credential_destination(profile: &RemoteProviderProfile, reference: &CredentialReference) -> String {
    profile.credential_bindings.get(reference).map_or_else(
        || format!("{}|unbound", profile.endpoint.origin()),
        |binding| {
            format!(
                "{}|{:?}|{}",
                profile.endpoint.origin(),
                binding.store,
                binding.slot.as_deref().unwrap_or_default()
            )
        },
    )
}

/// The operation a notice reports on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeKind {
    SessionValueSet,
    SessionValueCleared,
    StoredInKeychain,
    DeletedFromKeychain,
}

/// How the OS store addresses an item: endpoint origin plus slot. Two
/// references bound to one slot are one item, so presence is cached per
/// address rather than per reference.
type OsAddress = (String, String);

fn os_address(locator: &CredentialLocator) -> Option<OsAddress> {
    locator.slot.as_ref().map(|slot| (locator.origin.clone(), slot.clone()))
}

/// The settings row that started an operation. Carried with every command
/// and answer so a notice always lands on the row that asked, even when
/// several providers share an endpoint origin, reference name and slot.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Row {
    provider: String,
    reference: CredentialReference,
    destination: String,
}

impl Row {
    fn new(provider: &str, profile: &RemoteProviderProfile, reference: &CredentialReference) -> Self {
        Self {
            provider: provider.to_string(),
            reference: reference.clone(),
            destination: credential_destination(profile, reference),
        }
    }
}

enum Command {
    Probe(Vec<CredentialLocator>),
    Put(CredentialLocator, Zeroizing<Vec<u8>>, Row),
    Delete(CredentialLocator, Row),
}

enum Event {
    Opened(Result<(), RemoteCredentialError>),
    Presence(CredentialLocator, Result<bool, RemoteCredentialError>),
    Stored(CredentialLocator, Result<(), RemoteCredentialError>, Row),
    Deleted(CredentialLocator, Result<(), RemoteCredentialError>, Row),
}

struct KeychainLink {
    commands: Sender<Command>,
    events: Receiver<Event>,
    store: SharedStore,
}

/// Session store plus a keychain worker, with a presence cache the UI reads.
pub struct CredentialWorkbench {
    session: SessionCredentialStore,
    environment: EnvironmentCredentialStore,
    keychain: Option<KeychainLink>,
    keychain_state: KeychainState,
    presence: BTreeMap<OsAddress, CredentialState>,
    /// Rows that set a session value, with the origin their value lives
    /// under, so clearing every value can tell each of them.
    session_rows: BTreeMap<Row, String>,
    /// Commands sent to the worker whose answers have not arrived.
    in_flight: usize,
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
            environment: EnvironmentCredentialStore::new(),
            keychain: Some(KeychainLink {
                commands: command_tx,
                events: event_rx,
                store,
            }),
            keychain_state: KeychainState::Opening,
            presence: BTreeMap::new(),
            session_rows: BTreeMap::new(),
            in_flight: 0,
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
                    self.in_flight = self.in_flight.saturating_sub(1);
                    self.cache_presence(&locator, state_from(&result));
                }
                Event::Stored(locator, result, row) => {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    self.cache_presence(&locator, state_from(&result.clone().map(|()| true)));
                    self.notices
                        .push(WorkbenchNotice::new(&row, NoticeKind::StoredInKeychain, result));
                }
                Event::Deleted(locator, result, row) => {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    self.cache_presence(&locator, state_from(&result.clone().map(|()| false)));
                    self.notices
                        .push(WorkbenchNotice::new(&row, NoticeKind::DeletedFromKeychain, result));
                }
            }
        }
        if disconnected {
            // A worker that failed to open reports the specific reason and
            // then exits; that reason must outlive the disconnect.
            if !matches!(self.keychain_state, KeychainState::Unavailable(_)) {
                self.keychain_state = KeychainState::Unavailable(RemoteCredentialError::StoreUnavailable);
            }
            self.keychain = None;
            // Nothing will answer an in-flight probe or write now; settle
            // every pending entry so the UI stops waiting for it.
            self.in_flight = 0;
            for state in self.presence.values_mut() {
                if *state == CredentialState::Checking {
                    *state = CredentialState::StoreUnavailable;
                }
            }
        }
    }

    /// Pretend the worker thread died mid-flight: replace its channels with
    /// ones nobody serves, so the next poll observes the disconnect while
    /// entries are still checking.
    #[cfg(test)]
    pub(crate) fn simulate_worker_loss_for_tests(&mut self) {
        if let Some(link) = self.keychain.take() {
            let (_, events) = channel::<Event>();
            drop(link.commands);
            self.keychain = Some(KeychainLink {
                commands: channel::<Command>().0,
                events,
                store: link.store,
            });
        }
    }

    /// Whether worker results are still expected, so the UI keeps polling
    /// until the store has opened and every probe or write has answered.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.keychain_state == KeychainState::Opening || self.in_flight > 0
    }

    fn cache_presence(&mut self, locator: &CredentialLocator, state: CredentialState) {
        if let Some(address) = os_address(locator) {
            self.presence.insert(address, state);
        }
    }

    #[must_use]
    pub fn keychain_state(&self) -> &KeychainState {
        &self.keychain_state
    }

    /// Readiness for display. Session values are answered immediately; OS-store
    /// values come from the presence cache and show `Checking` until probed.
    pub fn readiness(&mut self, profile: &RemoteProviderProfile) -> Vec<CredentialReadiness> {
        self.request_probe(profile);
        let cache = CachedKeychain {
            presence: &self.presence,
            state: &self.keychain_state,
        };
        let stores = CredentialStores {
            session: &self.session,
            os_keychain: Some(&cache),
            environment: Some(&self.environment),
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
        let row = Row::new(provider, profile, reference);
        let result = Self::locator(profile, reference, CredentialStoreKind::Session).and_then(|locator| {
            self.session_rows.insert(row.clone(), locator.origin.clone());
            self.session.put(&locator, value)
        });
        self.notices
            .push(WorkbenchNotice::new(&row, NoticeKind::SessionValueSet, result.clone()));
        result
    }

    /// Persist a value in the OS store on the worker thread. A request the
    /// worker never receives is reported as a notice, like a worker failure.
    ///
    /// # Errors
    /// The reference must be bound to the OS store, which must be available,
    /// and the value must be non-empty and bounded.
    pub fn store_in_keychain(
        &mut self,
        provider: &str,
        profile: &RemoteProviderProfile,
        reference: &CredentialReference,
        value: &[u8],
    ) -> Result<(), RemoteCredentialError> {
        let result = self.queue_store(provider, profile, reference, value);
        if let Err(error) = &result {
            self.notices.push(WorkbenchNotice::new(
                &Row::new(provider, profile, reference),
                NoticeKind::StoredInKeychain,
                Err(error.clone()),
            ));
        }
        result
    }

    fn queue_store(
        &mut self,
        provider: &str,
        profile: &RemoteProviderProfile,
        reference: &CredentialReference,
        value: &[u8],
    ) -> Result<(), RemoteCredentialError> {
        super::validate_secret(value)?;
        let locator = Self::locator(profile, reference, CredentialStoreKind::OsKeychain)?;
        let commands = self.available_link()?.commands.clone();
        self.cache_presence(&locator, CredentialState::Checking);
        commands
            .send(Command::Put(
                locator,
                Zeroizing::new(value.to_vec()),
                Row::new(provider, profile, reference),
            ))
            .map_err(|_| RemoteCredentialError::StoreUnavailable)?;
        self.in_flight += 1;
        Ok(())
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
        match binding.store {
            CredentialStoreKind::Session => {
                let result = self.session.delete(&locator);
                self.notices.push(WorkbenchNotice::new(
                    &Row::new(provider, profile, reference),
                    NoticeKind::SessionValueCleared,
                    result.clone(),
                ));
                result
            }
            CredentialStoreKind::Environment => Err(RemoteCredentialError::StoreUnavailable),
            CredentialStoreKind::OsKeychain => {
                let result = self
                    .available_link()
                    .map(|link| link.commands.clone())
                    .and_then(|commands| {
                        self.cache_presence(&locator, CredentialState::Checking);
                        commands
                            .send(Command::Delete(locator.clone(), Row::new(provider, profile, reference)))
                            .map_err(|_| RemoteCredentialError::StoreUnavailable)
                    });
                if result.is_ok() {
                    self.in_flight += 1;
                }
                if let Err(error) = &result {
                    self.notices.push(WorkbenchNotice::new(
                        &Row::new(provider, profile, reference),
                        NoticeKind::DeletedFromKeychain,
                        Err(error.clone()),
                    ));
                }
                result
            }
        }
    }

    /// Drop every session-only value, overwriting buffers first. Each
    /// reference that held one gets a cleared notice so its row agrees.
    pub fn clear_session(&mut self) {
        let cleared: Vec<Row> = self
            .session_rows
            .iter()
            .filter(|(row, origin)| {
                let locator = CredentialLocator {
                    origin: (*origin).clone(),
                    reference: row.reference.clone(),
                    slot: None,
                };
                self.session.contains(&locator).unwrap_or(false)
            })
            .map(|(row, _)| row.clone())
            .collect();
        self.session.clear();
        self.session_rows.clear();
        for row in cleared {
            self.notices
                .push(WorkbenchNotice::new(&row, NoticeKind::SessionValueCleared, Ok(())));
        }
    }

    #[must_use]
    pub fn session_value_count(&self) -> usize {
        self.session.len()
    }

    /// Outcomes accumulated since the last call, oldest first.
    pub fn take_notices(&mut self) -> Vec<WorkbenchNotice> {
        std::mem::take(&mut self.notices)
    }

    /// Copy environment-backed bindings from the launching process into the
    /// in-memory snapshot. Missing names stay missing; captured names are kept.
    pub fn load_environment_bindings(&mut self, remote: &RemoteBrowserConfig) {
        let names = remote.environment_variable_names();
        self.environment.capture_from_process(&names);
    }

    /// Stores for allocation-time resolution. The OS store is only offered once opened.
    #[must_use]
    pub fn session_store(&self) -> &SessionCredentialStore {
        &self.session
    }

    #[must_use]
    pub fn environment_store(&self) -> &EnvironmentCredentialStore {
        &self.environment
    }

    #[must_use]
    pub fn keychain_store(&self) -> Option<SharedStore> {
        match (&self.keychain, &self.keychain_state) {
            (Some(link), KeychainState::Available) => Some(Arc::clone(&link.store)),
            _ => None,
        }
    }

    fn request_probe(&mut self, profile: &RemoteProviderProfile) {
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
            let Some(address) = os_address(&locator) else {
                continue;
            };
            if let std::collections::btree_map::Entry::Vacant(entry) = self.presence.entry(address) {
                entry.insert(CredentialState::Checking);
                wanted.push(locator);
            }
        }
        if !wanted.is_empty() {
            let count = wanted.len();
            if commands.send(Command::Probe(wanted)).is_ok() {
                self.in_flight += count;
            }
        }
    }

    fn locator(
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
        Ok(CredentialLocator::new(&profile.endpoint, reference, binding))
    }

    fn available_link(&self) -> Result<&KeychainLink, RemoteCredentialError> {
        match (&self.keychain, &self.keychain_state) {
            (Some(link), KeychainState::Available) => Ok(link),
            (_, KeychainState::Unavailable(error)) => Err(error.clone()),
            _ => Err(RemoteCredentialError::StoreUnavailable),
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
    presence: &'a BTreeMap<OsAddress, CredentialState>,
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
        match os_address(locator).and_then(|address| self.presence.get(&address)) {
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
            Command::Put(locator, value, row) => {
                let result = store.put(&locator, &value);
                Event::Stored(locator, result, row)
            }
            Command::Delete(locator, row) => {
                let result = store.delete(&locator);
                Event::Deleted(locator, result, row)
            }
        };
        if events.send(outcome).is_err() {
            return;
        }
    }
}
