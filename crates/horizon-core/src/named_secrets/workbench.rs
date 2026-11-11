//! The named secrets of the settings window, and the worker thread that reads them.
//!
//! The Settings window draws a list of names and one hidden field per value. The
//! credential store is a wait: a locked keychain asks for an unlock, and the Secret
//! Service of Linux answers over a socket. Both run on the worker thread here, and
//! the window reads a cached state that [`NamedSecretWorkbench::poll`] keeps.
//!
//! A value is never part of a state that the window reads: the window learns whether
//! this computer holds a value for a name, and it hands one in when the person types
//! it. The value goes to the worker and then to the store, and it comes back only as
//! the environment of the next agent panel.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use super::{NamedSecretChange, NamedSecretError, NamedSecretState, NamedSecretVault, SecretName};

/// How long a state that the worker answered stands before it is asked again. A
/// keychain can lock under a running Horizon, and a value can arrive from a server.
const REVALIDATE_AFTER: Duration = Duration::from_secs(10);

enum Command {
    Refresh,
    Store(SecretName, Zeroizing<Vec<u8>>),
    Delete(SecretName),
}

enum Event {
    Refreshed(Result<Vec<SecretName>, NamedSecretError>),
    Stored(SecretName, Result<NamedSecretChange, NamedSecretError>),
    Deleted(SecretName, Result<NamedSecretChange, NamedSecretError>),
}

/// The named secrets of this computer as the Settings window sees them.
pub struct NamedSecretWorkbench {
    commands: Option<Sender<Command>>,
    events: Option<Receiver<Event>>,
    /// The vault of this computer, for the list of names and the number of its
    /// changes. Reading them touches no store.
    vault: Option<Arc<NamedSecretVault>>,
    /// One state per name text, as the last answer of the worker described it, so a
    /// row of the settings list costs one lookup and no parse.
    states: BTreeMap<String, NamedSecretState>,
    /// The number of changes of the name list that the last request described.
    seen_names: u64,
    /// The names that the last request described, so an answer that holds a value
    /// for some of them says `NoValue` for the others.
    asked: Vec<SecretName>,
    /// When the list was last asked for, so a locked store is noticed without a
    /// restart and an idle frame asks nothing.
    asked_at: Option<Instant>,
    /// Commands whose answers have not arrived.
    in_flight: usize,
    /// The values that this computer stored or removed since the last call, for the
    /// link to a server that holds the same values.
    changes: Vec<NamedSecretChange>,
    /// Why the store refused the last operation, for the line of the window.
    failure: Option<NamedSecretError>,
}

impl std::fmt::Debug for NamedSecretWorkbench {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NamedSecretWorkbench")
            .field("states", &self.states.len())
            .field("in_flight", &self.in_flight)
            .finish_non_exhaustive()
    }
}

impl NamedSecretWorkbench {
    /// Starts the worker of `vault` and asks for the states of its names. The worker
    /// opens the store, reads every value and publishes the ones that the next agent
    /// panel inherits, so the window that a person opens shows the truth of the store
    /// as its first answer.
    #[must_use]
    pub fn new(vault: Option<Arc<NamedSecretVault>>, names: &[SecretName]) -> Self {
        let Some(vault) = vault else {
            return Self {
                commands: None,
                events: None,
                vault: None,
                states: BTreeMap::new(),
                seen_names: 0,
                asked: names.to_vec(),
                asked_at: None,
                in_flight: 0,
                changes: Vec::new(),
                failure: Some(NamedSecretError::StoreUnavailable),
            };
        };
        let (commands, command_rx) = channel::<Command>();
        let (event_tx, events) = channel::<Event>();
        let worker_vault = Arc::clone(&vault);
        let spawned = std::thread::Builder::new()
            .name("horizon-named-secrets".to_string())
            .spawn(move || worker(&worker_vault, &command_rx, &event_tx));
        if let Err(error) = spawned {
            tracing::error!("cannot start the named secrets thread: {error}");
            return Self {
                commands: None,
                events: None,
                vault: None,
                states: BTreeMap::new(),
                seen_names: 0,
                asked: names.to_vec(),
                asked_at: None,
                in_flight: 0,
                changes: Vec::new(),
                failure: Some(NamedSecretError::StoreUnavailable),
            };
        }
        let mut bench = Self {
            commands: Some(commands),
            events: Some(events),
            vault: Some(Arc::clone(&vault)),
            states: BTreeMap::new(),
            seen_names: vault.names_generation(),
            asked: names.to_vec(),
            asked_at: None,
            in_flight: 0,
            changes: Vec::new(),
            failure: None,
        };
        bench.ask(names);
        bench
    }

    /// Asks the worker to read the store again: the values that the next agent panel
    /// inherits, and the state of every name.
    pub fn refresh(&mut self, names: &[SecretName]) {
        self.ask(names);
    }

    /// Marks every name as being read, and sends one read of the store.
    fn ask(&mut self, names: &[SecretName]) {
        self.asked = names.to_vec();
        for name in names {
            self.states.insert(name.as_str().to_owned(), NamedSecretState::Checking);
        }
        self.in_flight += 1;
        self.asked_at = Some(Instant::now());
        self.send(Command::Refresh);
    }

    /// Reads the store again when the answer that the window shows is old, or when
    /// a name appeared or went. It builds no list of names unless it asks, so an
    /// idle frame allocates nothing.
    pub fn refresh_if_stale(&mut self) {
        let generation = self.vault.as_ref().map_or(0, |vault| vault.names_generation());
        let stale = match self.asked_at {
            Some(asked_at) => asked_at.elapsed() >= REVALIDATE_AFTER,
            None => true,
        };
        if !stale && generation == self.seen_names {
            return;
        }
        let names = self.vault.as_ref().map(|vault| vault.names()).unwrap_or_default();
        self.seen_names = generation;
        self.ask(&names);
    }

    /// Stores `value` under `name` on the worker thread.
    pub fn store(&mut self, name: SecretName, value: Zeroizing<Vec<u8>>) {
        self.in_flight += 1;
        self.states.insert(name.as_str().to_owned(), NamedSecretState::Checking);
        self.send(Command::Store(name, value));
    }

    /// Forgets the value and the name on the worker thread.
    pub fn delete(&mut self, name: SecretName) {
        self.in_flight += 1;
        self.states.insert(name.as_str().to_owned(), NamedSecretState::Checking);
        self.send(Command::Delete(name));
    }

    /// The state of one name, as the last answer described it. The settings window
    /// passes the text of the name, so a row of the list costs one lookup and no
    /// parse.
    #[must_use]
    pub fn state_of(&self, name: &str) -> NamedSecretState {
        self.states.get(name).copied().unwrap_or(NamedSecretState::Checking)
    }

    /// The state of one name, as the last answer described it.
    #[must_use]
    pub fn state(&self, name: &SecretName) -> NamedSecretState {
        self.state_of(name.as_str())
    }

    /// Why the last operation failed, for one line of the window.
    #[must_use]
    pub fn failure(&self) -> Option<&NamedSecretError> {
        self.failure.as_ref()
    }

    /// Takes the failure, so the line is shown once.
    pub fn take_failure(&mut self) -> Option<NamedSecretError> {
        self.failure.take()
    }

    /// The values that this computer stored or removed since the last call, so the
    /// link to a server queues each of them as one secret.
    #[must_use]
    pub fn take_changes(&mut self) -> Vec<NamedSecretChange> {
        std::mem::take(&mut self.changes)
    }

    /// Whether an answer is still due, so the window keeps drawing frames.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.in_flight > 0
    }

    /// Drains the answers of the worker. Cheap when the worker is idle.
    pub fn poll(&mut self) {
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(events_rx) = &self.events {
            loop {
                match events_rx.try_recv() {
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
                Event::Refreshed(Ok(present)) => {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    self.failure = None;
                    let present: std::collections::BTreeSet<SecretName> = present.into_iter().collect();
                    for name in &self.asked {
                        let state = if present.contains(name) {
                            NamedSecretState::Present
                        } else {
                            NamedSecretState::NoValue
                        };
                        self.states.insert(name.as_str().to_owned(), state);
                    }
                }
                Event::Refreshed(Err(error)) => {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    set_every_state(&mut self.states, &error);
                    self.failure = Some(error);
                }
                Event::Stored(name, answer) => {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    self.settle(name, answer.map(|change| change.stored));
                }
                Event::Deleted(name, answer) => {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    self.settle(name, answer.map(|change| change.stored));
                }
            }
        }
        if disconnected {
            self.in_flight = 0;
            for state in self.states.values_mut() {
                if *state == NamedSecretState::Checking {
                    *state = NamedSecretState::StoreUnavailable;
                }
            }
            self.failure = Some(NamedSecretError::StoreUnavailable);
            self.commands = None;
            self.events = None;
        }
    }

    fn settle(&mut self, name: SecretName, stored: Result<bool, NamedSecretError>) {
        match stored {
            Ok(stored) => {
                self.failure = None;
                let state = if stored {
                    NamedSecretState::Present
                } else {
                    NamedSecretState::NoValue
                };
                self.states.insert(name.as_str().to_owned(), state);
                self.changes.push(NamedSecretChange { name, stored });
            }
            Err(error) => {
                self.states.insert(name.as_str().to_owned(), state_of_failure(&error));
                self.failure = Some(error);
            }
        }
    }

    fn send(&mut self, command: Command) {
        let Some(commands) = &self.commands else {
            self.in_flight = self.in_flight.saturating_sub(1);
            self.failure = Some(NamedSecretError::StoreUnavailable);
            return;
        };
        if commands.send(command).is_err() {
            self.in_flight = self.in_flight.saturating_sub(1);
            self.failure = Some(NamedSecretError::StoreUnavailable);
        }
    }
}

/// The state of every name after a failure of the store.
fn state_of_failure(error: &NamedSecretError) -> NamedSecretState {
    match error {
        NamedSecretError::Locked => NamedSecretState::Locked,
        NamedSecretError::InvalidName { .. }
        | NamedSecretError::ReservedName { .. }
        | NamedSecretError::InvalidValue { .. }
        | NamedSecretError::ValueNotText { .. } => NamedSecretState::NoValue,
        NamedSecretError::StoreUnavailable | NamedSecretError::Platform { .. } => NamedSecretState::StoreUnavailable,
    }
}

/// Describes every name by the failure of the store, so no row of the window keeps
/// a state that the store cannot confirm.
fn set_every_state(states: &mut BTreeMap<String, NamedSecretState>, error: &NamedSecretError) {
    let state = state_of_failure(error);
    for value in states.values_mut() {
        *value = state;
    }
}

fn worker(vault: &NamedSecretVault, commands: &Receiver<Command>, events: &Sender<Event>) {
    while let Ok(command) = commands.recv() {
        let event = match command {
            Command::Refresh => Event::Refreshed(vault.refresh()),
            Command::Store(name, value) => {
                let answer = vault.put(&name, &value);
                Event::Stored(name, answer)
            }
            Command::Delete(name) => {
                let answer = vault.remove(&name);
                Event::Deleted(name, answer)
            }
        };
        if events.send(event).is_err() {
            return;
        }
    }
}
