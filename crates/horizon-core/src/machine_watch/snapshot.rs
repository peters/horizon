//! The snapshot of the watch that a frame reads, and the shared state that the
//! readers of the machines write.
//!
//! A frame reads [`Shared::snapshot`], which hands out one [`std::sync::Arc`] of a
//! value that holds every machine, its link and its agents. The value changes only
//! when a row changed, so a frame that sees the same version does nothing and asks
//! for no repaint. The lock of the shared state is held for the microseconds of a
//! copy of that value and never across a read of a file, a wait or a process, so a
//! frame may take it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use super::agents::{AgentPrograms, observations};
use super::report::Report;
use super::state::{AgentState, MachineAgent, MachineModel};

/// The time of this computer in milliseconds of the Unix epoch, which a lost link
/// reports as the moment that it was lost.
#[must_use]
pub fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}

/// Why `ssh` refused a machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchRefusal {
    /// The host key of the machine is unknown or changed, and `ssh` refused it.
    HostKey,
    /// The machine refused the key of this computer, or the key is not there.
    Key,
    /// The record of the machine no longer passes the rule of an SSH terminal.
    Record,
    /// `ssh` refused the attempt in words that name none of the above.
    Other,
}

impl WatchRefusal {
    /// The stable code of this refusal, for a log line and for a later field.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::HostKey => "machine_watch_host_key",
            Self::Key => "machine_watch_key",
            Self::Record => "machine_watch_record",
            Self::Other => "machine_watch_other",
        }
    }

    /// The words of the words that this refusal names, as `ssh` writes them.
    #[must_use]
    pub const fn words(self) -> &'static str {
        match self {
            Self::HostKey => "host key",
            Self::Key => "key",
            Self::Record => "record",
            Self::Other => "refused",
        }
    }

    /// The refusal that the words of `ssh` name, or [`Self::Other`].
    #[must_use]
    pub fn of_ssh(stderr: &str) -> Self {
        let lower = stderr.to_ascii_lowercase();
        if lower.contains("host key verification failed")
            || lower.contains("host key is known")
            || lower.contains("remote host identification has changed")
        {
            return Self::HostKey;
        }
        if lower.contains("permission denied")
            || lower.contains("no such identity")
            || lower.contains("load key")
            || lower.contains("too many authentication failures")
        {
            return Self::Key;
        }
        Self::Other
    }
}

/// What this computer knows about its link to one machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchLink {
    /// The `ssh` child runs and no report of the machine has arrived yet.
    Connecting,
    /// The last report of the machine arrived.
    Watching,
    /// The machine has no `tmux` program.
    NoTmux,
    /// The machine has `tmux` and no `tmux` server.
    NoServer,
    /// The link ended, and the watch waits for its next attempt.
    Lost {
        /// The moment that the link was lost, in milliseconds of the Unix epoch.
        since_millis: u64,
        /// The number of the attempt that comes next, counted from 1.
        attempt: u32,
    },
    /// `ssh` refused this machine.
    Refused {
        /// What `ssh` refused.
        reason: WatchRefusal,
        /// The moment of the refusal, in milliseconds of the Unix epoch.
        since_millis: u64,
    },
    /// A machine of the list beyond the number of machines of one watch.
    OverTheLimit,
    /// The setting `machines.watch_agents` is off.
    Off,
}

impl WatchLink {
    /// Whether the link of this machine is up.
    #[must_use]
    pub const fn watching(self) -> bool {
        matches!(self, Self::Watching | Self::NoTmux | Self::NoServer)
    }
}

/// One machine of the snapshot: its link and the agents that are known there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineWatchState {
    /// The name of the machine in the machine list.
    pub machine: String,
    /// The state of the link to that machine.
    pub link: WatchLink,
    /// The agents of that machine, in session and pane order.
    pub agents: Vec<MachineAgent>,
}

impl MachineWatchState {
    /// The heaviest state of the agents of this machine, or [`AgentState::Idle`]
    /// when no agent is there.
    #[must_use]
    pub fn heaviest(&self) -> AgentState {
        self.agents.iter().map(|agent| agent.state).max().unwrap_or_default()
    }
}

/// The agents of every watched machine at one moment.
///
/// The version changes only when a row, a link or a machine changed, so a frame
/// compares the version that it holds with [`Self::version`] and does nothing when
/// they are equal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MachineWatchSnapshot {
    /// The version of this snapshot, which rises with every change.
    pub version: u64,
    /// The machines of the list, in name order.
    pub machines: Vec<MachineWatchState>,
    /// How many agents of every machine need the person.
    pub needs_you: usize,
    /// How many agents of every machine finished while the person did not look.
    pub done: usize,
}

impl MachineWatchSnapshot {
    /// The state of one machine of the list.
    #[must_use]
    pub fn machine(&self, machine: &str) -> Option<&MachineWatchState> {
        self.machines.iter().find(|held| held.machine == machine)
    }

    /// The agents of one machine.
    #[must_use]
    pub fn agents(&self, machine: &str) -> &[MachineAgent] {
        self.machine(machine).map_or(&[], |held| held.agents.as_slice())
    }

    /// The totals of the snapshot, from its machines.
    fn totals(machines: &[MachineWatchState]) -> (usize, usize) {
        let agents = machines.iter().flat_map(|machine| &machine.agents);
        let mut needs_you = 0;
        let mut done = 0;
        for agent in agents {
            match agent.state {
                AgentState::NeedsYou => needs_you += 1,
                AgentState::Done => done += 1,
                AgentState::Idle | AgentState::Working => {}
            }
        }
        (needs_you, done)
    }
}

/// The state that the readers of the machines write and that a frame reads.
#[derive(Debug)]
pub struct Shared {
    inner: Mutex<Inner>,
}

impl Shared {
    /// The shared state of a watch with no machine.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                version: 0,
                snapshot: Arc::new(MachineWatchSnapshot::default()),
                models: BTreeMap::new(),
                links: BTreeMap::new(),
                programs: AgentPrograms::builtin(),
            }),
        }
    }

    /// The snapshot of this moment. A frame holds the [`std::sync::Arc`] and reads
    /// it without the lock.
    #[must_use]
    pub fn snapshot(&self) -> Arc<MachineWatchSnapshot> {
        Arc::clone(&self.lock().snapshot)
    }

    /// This watch with the programs of the agent kinds of `programs`, which a
    /// preset of the settings may add to.
    pub fn set_programs(&self, programs: AgentPrograms) {
        let mut inner = self.lock();
        if inner.programs == programs {
            return;
        }
        inner.programs = programs;
        inner.publish();
    }

    /// the programs of the agent kinds of this watch.
    fn programs(&self) -> AgentPrograms {
        self.lock().programs.clone()
    }

    /// Records that the link of `machine` is `link`.
    pub fn set_link(&self, machine: &str, link: WatchLink) {
        let mut inner = self.lock();
        if inner.links.get(machine) == Some(&link) {
            return;
        }
        inner.link(machine, link);
        inner.publish();
    }

    /// Applies one report of `machine`: its link and its agents.
    pub fn apply_report(&self, machine: &str, report: &Report) {
        let programs = self.programs();
        let observations = observations(report, &programs);
        let link = match report.note {
            Some(super::MachineNote::NoTmux) => WatchLink::NoTmux,
            Some(super::MachineNote::NoServer) => WatchLink::NoServer,
            None => WatchLink::Watching,
        };
        let mut inner = self.lock();
        inner.link(machine, link);
        inner
            .models
            .entry(machine.to_owned())
            .or_insert_with(|| MachineModel::new(machine))
            .observe(&observations);
        inner.publish();
    }

    /// Records that the person looked at the agent of `session` and `pane` of
    /// `machine`. Answers whether any row changed.
    pub fn mark_seen(&self, machine: &str, session: &str, pane: &str) -> bool {
        let mut inner = self.lock();
        let changed = inner
            .models
            .get_mut(machine)
            .is_some_and(|model| model.mark_seen(session, pane));
        if changed {
            inner.publish();
        }
        changed
    }

    /// Forgets `machine`: its link and its rows leave the snapshot.
    pub fn remove(&self, machine: &str) {
        let mut inner = self.lock();
        let removed = inner.links.remove(machine).is_some();
        let cleared = inner.models.remove(machine).is_some();
        if removed || cleared {
            inner.publish();
        }
    }

    /// Records that the machine answered with its heartbeat, which a link that has
    /// not reported yet reads as watching.
    pub fn answered(&self, machine: &str) {
        let mut inner = self.lock();
        if inner.links.get(machine) != Some(&WatchLink::Connecting) {
            return;
        }
        inner.link(machine, WatchLink::Watching);
        inner.publish();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

/// The state behind the lock of [`Shared`].
#[derive(Debug)]
struct Inner {
    version: u64,
    snapshot: Arc<MachineWatchSnapshot>,
    models: BTreeMap<String, MachineModel>,
    links: BTreeMap<String, WatchLink>,
    programs: AgentPrograms,
}

impl Inner {
    fn link(&mut self, machine: &str, link: WatchLink) {
        self.links.insert(machine.to_owned(), link);
        if !link.watching()
            && let Some(model) = self.models.get_mut(machine)
        {
            // A machine whose link is down reports nothing, so its rows would be a
            // claim of an older report. They leave with the link.
            model.clear();
        }
    }

    /// Builds the snapshot of the state and swaps it in when it changed.
    fn publish(&mut self) {
        let mut machines = Vec::with_capacity(self.links.len());
        for (machine, link) in &self.links {
            let agents = self.models.get(machine).map(MachineModel::rows).unwrap_or_default();
            machines.push(MachineWatchState {
                machine: machine.clone(),
                link: *link,
                agents,
            });
        }
        let (needs_you, done) = MachineWatchSnapshot::totals(&machines);
        let candidate = MachineWatchSnapshot {
            version: self.version,
            machines,
            needs_you,
            done,
        };
        if *self.snapshot == candidate {
            return;
        }
        self.version = self.version.saturating_add(1);
        self.snapshot = Arc::new(MachineWatchSnapshot {
            version: self.version,
            ..candidate
        });
    }
}
