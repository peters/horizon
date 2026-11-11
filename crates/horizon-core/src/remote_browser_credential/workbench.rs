//! Credential entry state for the settings UI: synchronous session-only
//! values, and secrets-file operations run on a worker thread so a disk write
//! never stalls the render loop.

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use horizon_browser::remote::{CredentialReference, CredentialStoreKind, RemoteBrowserConfig, RemoteProviderProfile};

use zeroize::Zeroizing;

use super::{
    CredentialLocator, CredentialReadiness, CredentialState, CredentialStores, EnvironmentCredentialStore,
    RemoteCredentialError, RemoteCredentialStore, Sealed, SecretSink, SessionCredentialStore, readiness,
};
use crate::secret_store::{SecretStore, UnreadableStore};

/// Boxed store the worker thread owns; the allocation path borrows it later.
pub type SharedStore = Arc<Mutex<Box<dyn RemoteCredentialStore + Send>>>;

/// How the worker obtains the secrets file. Injectable so tests use their own store.
pub type StoreOpener = Box<dyn FnOnce() -> Result<Box<dyn RemoteCredentialStore + Send>, RemoteCredentialError> + Send>;

/// Whether the secrets file can be used right now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SecretsState {
    /// The worker is opening the file.
    Opening,
    /// The file answered, so a value can be read or written.
    Ready,
    /// The file cannot be used, with the typed reason and its path.
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

/// One file value that this computer stored or removed, for a link to a
/// server that holds the same values. It carries the reference and never the
/// value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialValueChange {
    /// The reference of the settings row whose value changed.
    pub reference: CredentialReference,
    /// Whether the value was stored (`true`) or removed.
    pub stored: bool,
}

/// The operation a notice reports on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeKind {
    SessionValueSet,
    SessionValueCleared,
    StoredInFile,
    DeletedFromFile,
}

/// Longest time that a cached presence stands before the file is asked again.
const REVALIDATE_AFTER: Duration = Duration::from_secs(10);

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
    Read(
        CredentialLocator,
        Sender<Result<Zeroizing<Vec<u8>>, RemoteCredentialError>>,
    ),
    Put(CredentialLocator, Zeroizing<Vec<u8>>, Row),
    Delete(CredentialLocator, Row),
}

enum Event {
    Opened(Result<(), RemoteCredentialError>),
    Presence(CredentialLocator, Result<bool, RemoteCredentialError>),
    Stored(CredentialLocator, Result<(), RemoteCredentialError>, Row),
    Deleted(CredentialLocator, Result<(), RemoteCredentialError>, Row),
}

struct StoreLink {
    commands: Sender<Command>,
    events: Receiver<Event>,
    store: SharedStore,
}

/// The worker's answer to one read of a value that only the secrets file holds.
/// The value is a zeroizing buffer, and it reaches nothing but the caller that
/// asked for it: polling never waits, so a disk read holds only the workbench
/// thread.
pub struct CredentialValueReply {
    answer: Receiver<Result<Zeroizing<Vec<u8>>, RemoteCredentialError>>,
    answered: bool,
}

impl CredentialValueReply {
    /// The answer, or `None` while the workbench has not answered yet. A
    /// worker that ended without answering reports an unavailable store, so a
    /// caller waiting for the value refuses its request instead of waiting
    /// for the create deadline. A read answers once.
    #[must_use]
    pub fn poll(&mut self) -> Option<Result<Zeroizing<Vec<u8>>, RemoteCredentialError>> {
        if self.answered {
            return None;
        }
        let answer = match self.answer.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(RemoteCredentialError::StoreUnavailable),
        };
        self.answered = true;
        Some(answer)
    }
}

/// Session store plus a store worker, with a presence cache the UI reads.
pub struct CredentialWorkbench {
    generation: u64,
    session: SessionCredentialStore,
    environment: EnvironmentCredentialStore,
    store: Option<StoreLink>,
    /// The secrets file of this workbench, when it has one, so a caller of this
    /// crate can open the same file on its own thread.
    secrets_file: Option<std::path::PathBuf>,
    secrets_state: SecretsState,
    presence: BTreeMap<OsAddress, CredentialState>,
    /// When the presence of each item was last asked for, so a file that another
    /// process changed is noticed without a restart. One entry per address,
    /// because Settings reads one profile at a time.
    probed_at: BTreeMap<OsAddress, Instant>,
    /// How long a cached presence may stand before the workbench asks again.
    revalidate_after: Duration,
    /// Rows that set a session value, with the origin their value lives
    /// under, so clearing every value can tell each of them.
    session_rows: BTreeMap<Row, String>,
    /// Commands sent to the worker whose answers have not arrived.
    in_flight: usize,
    notices: Vec<WorkbenchNotice>,
    /// The file values that changed and wait for the next call, so a server
    /// holds them too.
    value_changes: Vec<CredentialValueChange>,
}

impl CredentialWorkbench {
    /// Nonsecret invalidation version for cached account-bound discovery.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    fn credentials_changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Open the secrets file at `path` on a worker thread.
    #[must_use]
    pub fn spawn_file(path: std::path::PathBuf) -> Self {
        let mut bench = Self::with_opener(Box::new({
            let path = path.clone();
            move || SecretStore::open(path).map(|store| Box::new(store) as Box<dyn RemoteCredentialStore + Send>)
        }));
        bench.secrets_file = Some(path);
        bench
    }

    /// A handle that opens the secrets file of this workbench again, for a caller
    /// that must read a value on its own thread and never wait for the settings
    /// worker. `None` for a workbench that has no file of its own, which is what a
    /// test injects.
    #[must_use]
    pub fn file_opener(&self) -> Option<StoreOpener> {
        let path = self.secrets_file.clone()?;
        Some(Box::new(move || {
            SecretStore::open(path).map(|store| Box::new(store) as Box<dyn RemoteCredentialStore + Send>)
        }))
    }

    /// Open whatever `opener` returns on a worker thread.
    #[must_use]
    pub fn with_opener(opener: StoreOpener) -> Self {
        let (command_tx, command_rx) = channel::<Command>();
        let (event_tx, event_rx) = channel::<Event>();
        let store: SharedStore = Arc::new(Mutex::new(placeholder_store()));
        let worker_store = Arc::clone(&store);
        thread::Builder::new()
            .name("remote-browser-secrets".into())
            .spawn(move || worker(opener, &worker_store, &command_rx, &event_tx))
            .ok();
        Self {
            generation: 0,
            session: SessionCredentialStore::new(),
            environment: EnvironmentCredentialStore::new(),
            store: Some(StoreLink {
                commands: command_tx,
                events: event_rx,
                store,
            }),
            secrets_file: None,
            secrets_state: SecretsState::Opening,
            presence: BTreeMap::new(),
            probed_at: BTreeMap::new(),
            revalidate_after: REVALIDATE_AFTER,
            session_rows: BTreeMap::new(),
            in_flight: 0,
            notices: Vec::new(),
            value_changes: Vec::new(),
        }
    }

    /// The values that the secrets file stored or removed since the last call,
    /// oldest first. A link to a server queues each of them as one secret,
    /// so the other computers hold the same value; a computer without a server
    /// drops the list and needs no other change.
    #[must_use]
    pub fn take_value_changes(&mut self) -> Vec<CredentialValueChange> {
        std::mem::take(&mut self.value_changes)
    }

    /// Drain worker events. Call once per frame; cheap when idle.
    pub fn poll(&mut self) {
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(link) = &self.store {
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
                Event::Opened(Ok(())) => self.secrets_state = SecretsState::Ready,
                Event::Opened(Err(error)) => self.secrets_state = SecretsState::Unavailable(error),
                Event::Presence(locator, result) => {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    self.cache_presence(&locator, state_from(&result));
                }
                Event::Stored(locator, result, row) => {
                    self.credentials_changed();
                    self.in_flight = self.in_flight.saturating_sub(1);
                    self.cache_presence(&locator, state_from(&result.clone().map(|()| true)));
                    if result.is_ok() {
                        self.value_changes.push(CredentialValueChange {
                            reference: row.reference.clone(),
                            stored: true,
                        });
                    }
                    self.notices
                        .push(WorkbenchNotice::new(&row, NoticeKind::StoredInFile, result));
                }
                Event::Deleted(locator, result, row) => {
                    self.credentials_changed();
                    self.in_flight = self.in_flight.saturating_sub(1);
                    self.cache_presence(&locator, state_from(&result.clone().map(|()| false)));
                    if result.is_ok() {
                        self.value_changes.push(CredentialValueChange {
                            reference: row.reference.clone(),
                            stored: false,
                        });
                    }
                    self.notices
                        .push(WorkbenchNotice::new(&row, NoticeKind::DeletedFromFile, result));
                }
            }
        }
        if disconnected {
            // A worker that failed to open reports the specific reason and
            // then exits; that reason must outlive the disconnect.
            if !matches!(self.secrets_state, SecretsState::Unavailable(_)) {
                self.secrets_state = SecretsState::Unavailable(RemoteCredentialError::StoreUnavailable);
            }
            self.store = None;
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
        if let Some(link) = self.store.take() {
            let (_, events) = channel::<Event>();
            drop(link.commands);
            self.store = Some(StoreLink {
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
        self.secrets_state == SecretsState::Opening || self.in_flight > 0
    }

    fn cache_presence(&mut self, locator: &CredentialLocator, state: CredentialState) {
        if let Some(address) = os_address(locator) {
            self.presence.insert(address, state);
        }
    }

    #[must_use]
    pub fn secrets_state(&self) -> &SecretsState {
        &self.secrets_state
    }

    /// Readiness for display. Session values are answered immediately; file
    /// values come from the presence cache and show `Checking` until probed.
    pub fn readiness(&mut self, profile: &RemoteProviderProfile) -> Vec<CredentialReadiness> {
        // Another process of this home can write the file under a running
        // Horizon, and a person can copy another secrets file over it. Each address
        // is asked again on its own bound instead of once per start, and the cached
        // state stays visible until the answer arrives, so a list does not flicker
        // every interval.
        self.request_probe(profile);
        let cache = CachedStore {
            presence: &self.presence,
            state: &self.secrets_state,
        };
        let stores = CredentialStores {
            session: &self.session,
            secrets_file: Some(&cache),
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
        if result.is_ok() {
            self.credentials_changed();
        }
        self.notices
            .push(WorkbenchNotice::new(&row, NoticeKind::SessionValueSet, result.clone()));
        result
    }

    /// Persist a value in the secrets file on the worker thread. A request the
    /// worker never receives is reported as a notice, like a worker failure.
    ///
    /// # Errors
    /// The reference must be bound to the secrets file, which must be available,
    /// and the value must be non-empty and bounded.
    pub fn store_value(
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
                NoticeKind::StoredInFile,
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
        let locator = Self::locator(profile, reference, CredentialStoreKind::File)?;
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
        self.credentials_changed();
        Ok(())
    }

    /// Forget a value wherever its binding says it lives.
    ///
    /// # Errors
    /// Unbound references and an unavailable secrets file.
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
                if result.is_ok() {
                    self.credentials_changed();
                }
                self.notices.push(WorkbenchNotice::new(
                    &Row::new(provider, profile, reference),
                    NoticeKind::SessionValueCleared,
                    result.clone(),
                ));
                result
            }
            CredentialStoreKind::Environment => Err(RemoteCredentialError::StoreUnavailable),
            CredentialStoreKind::File => {
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
                    self.credentials_changed();
                }
                if let Err(error) = &result {
                    self.notices.push(WorkbenchNotice::new(
                        &Row::new(provider, profile, reference),
                        NoticeKind::DeletedFromFile,
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
        self.credentials_changed();
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

    /// Stores for allocation-time resolution. The file is only offered once opened.
    #[must_use]
    pub fn session_store(&self) -> &SessionCredentialStore {
        &self.session
    }

    #[must_use]
    pub fn environment_store(&self) -> &EnvironmentCredentialStore {
        &self.environment
    }

    /// Ask the workbench thread to read one value from the secrets file. The
    /// answer arrives through the reply, so the caller never takes the store
    /// mutex and never reads the file on its own thread. The value is read
    /// as the store holds it, and every check of it belongs to the caller.
    ///
    /// # Errors
    /// The reference must be bound to the secrets file, which must be available.
    pub fn request_value(
        &mut self,
        profile: &RemoteProviderProfile,
        reference: &CredentialReference,
    ) -> Result<CredentialValueReply, RemoteCredentialError> {
        let locator = Self::locator(profile, reference, CredentialStoreKind::File)?;
        let commands = self.available_link()?.commands.clone();
        let (answers, answer) = channel();
        commands
            .send(Command::Read(locator, answers))
            .map_err(|_| RemoteCredentialError::StoreUnavailable)?;
        Ok(CredentialValueReply {
            answer,
            answered: false,
        })
    }

    /// Whether the file answered and a value can be read from it: the same
    /// state [`Self::store_handle`] offers the store in.
    #[must_use]
    pub fn store_available(&self) -> bool {
        matches!((&self.store, &self.secrets_state), (Some(_), SecretsState::Ready))
    }

    #[must_use]
    pub fn store_handle(&self) -> Option<SharedStore> {
        if !self.store_available() {
            return None;
        }
        self.store.as_ref().map(|link| Arc::clone(&link.store))
    }

    /// How long until the next cached address is due for another probe, so a
    /// reader that only asks on a frame can schedule that frame. `None` when no
    /// address is cached and nothing is due.
    #[must_use]
    pub fn next_revalidation_in(&self) -> Option<Duration> {
        if self.secrets_state != SecretsState::Ready {
            return None;
        }
        self.probed_at
            .values()
            .map(|at| self.revalidate_after.saturating_sub(at.elapsed()))
            // An address that is already due is probed by the frame that reads
            // it, so asking for a frame now would spin: the worker answer brings
            // the next frame instead.
            .filter(|due| !due.is_zero())
            .min()
    }

    fn request_probe(&mut self, profile: &RemoteProviderProfile) {
        let Some(commands) = self.store.as_ref().map(|link| link.commands.clone()) else {
            return;
        };
        if self.secrets_state != SecretsState::Ready {
            return;
        }
        let mut wanted = Vec::new();
        for reference in profile.authentication.references() {
            let Some(binding) = profile.credential_bindings.get(reference) else {
                continue;
            };
            if binding.store != CredentialStoreKind::File {
                continue;
            }
            let locator = CredentialLocator::new(&profile.endpoint, reference, binding);
            let Some(address) = os_address(&locator) else {
                continue;
            };
            // A due address keeps the state that the person sees and is asked
            // again, so a relocked store replaces it with `Locked`.
            let due = self
                .probed_at
                .get(&address)
                .is_none_or(|at| at.elapsed() >= self.revalidate_after);
            match self.presence.entry(address.clone()) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(CredentialState::Checking);
                    wanted.push(locator);
                }
                std::collections::btree_map::Entry::Occupied(_) if due => wanted.push(locator),
                std::collections::btree_map::Entry::Occupied(_) => {}
            }
        }
        if !wanted.is_empty() {
            let count = wanted.len();
            if commands.send(Command::Probe(wanted.clone())).is_ok() {
                self.in_flight += count;
                let now = Instant::now();
                for locator in &wanted {
                    if let Some(address) = os_address(locator) {
                        self.probed_at.insert(address, now);
                    }
                }
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

    fn available_link(&self) -> Result<&StoreLink, RemoteCredentialError> {
        match (&self.store, &self.secrets_state) {
            (Some(link), SecretsState::Ready) => Ok(link),
            (_, SecretsState::Unavailable(error)) => Err(error.clone()),
            _ => Err(RemoteCredentialError::StoreUnavailable),
        }
    }
}

fn state_from(result: &Result<bool, RemoteCredentialError>) -> CredentialState {
    match result {
        Ok(true) => CredentialState::Present,
        Ok(false) | Err(RemoteCredentialError::Missing) => CredentialState::Missing,
        Err(_) => CredentialState::StoreUnavailable,
    }
}

/// The store that the worker holds until `opener` answers. It holds no value and
/// no caller reads it before the answer, so a value in memory is enough.
fn placeholder_store() -> Box<dyn RemoteCredentialStore + Send> {
    match SecretStore::in_memory() {
        Ok(store) => Box::new(store),
        Err(error) => Box::new(UnreadableStore::new(error)),
    }
}

/// Accepts one value from the secrets file into a zeroizing buffer, for a caller
/// on another thread. It checks nothing: the caller checks the value when it
/// builds the request, with the same checks the store's own path applies.
#[derive(Default)]
struct StoreValueSink {
    value: Option<Zeroizing<Vec<u8>>>,
}

impl Sealed for StoreValueSink {}

impl SecretSink for StoreValueSink {
    fn accept(&mut self, bytes: &[u8]) -> Result<(), RemoteCredentialError> {
        self.value = Some(Zeroizing::new(bytes.to_vec()));
        Ok(())
    }
}

/// Read-only view of the presence cache in store form, for [`readiness`].
struct CachedStore<'a> {
    presence: &'a BTreeMap<OsAddress, CredentialState>,
    state: &'a SecretsState,
}

impl Sealed for CachedStore<'_> {}

impl RemoteCredentialStore for CachedStore<'_> {
    fn kind(&self) -> CredentialStoreKind {
        CredentialStoreKind::File
    }

    fn put(&mut self, _: &CredentialLocator, _: &[u8]) -> Result<(), RemoteCredentialError> {
        Err(RemoteCredentialError::StoreUnavailable)
    }

    fn delete(&mut self, _: &CredentialLocator) -> Result<(), RemoteCredentialError> {
        Err(RemoteCredentialError::StoreUnavailable)
    }

    fn contains(&self, locator: &CredentialLocator) -> Result<bool, RemoteCredentialError> {
        match self.state {
            SecretsState::Unavailable(error) => return Err(error.clone()),
            SecretsState::Opening => return Err(RemoteCredentialError::Checking),
            SecretsState::Ready => {}
        }
        match os_address(locator).and_then(|address| self.presence.get(&address)) {
            Some(CredentialState::Present) => Ok(true),
            Some(CredentialState::Missing) => Ok(false),
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
            // A read answers only the caller that asked for it, so an answer
            // nobody waits for any more must not end the worker.
            Command::Read(locator, answers) => {
                let mut sink = StoreValueSink::default();
                let result = store
                    .with_secret(&locator, &mut sink)
                    .and_then(|()| sink.value.take().ok_or(RemoteCredentialError::Missing));
                let _ = answers.send(result);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_answers_once_and_a_worker_that_ended_answers_an_unavailable_store() {
        let (answers, answer) = channel();
        let mut reply = CredentialValueReply {
            answer,
            answered: false,
        };
        assert!(reply.poll().is_none(), "the worker has not answered yet");
        drop(answers);
        assert_eq!(
            reply.poll().map(|answer| answer.map(|value| value.to_vec())),
            Some(Err(RemoteCredentialError::StoreUnavailable))
        );
        assert!(reply.poll().is_none(), "a read answers once");
    }
}
