//! The supervisor of the watch: one reader per machine of the list, started when the
//! machine is in the list and stopped when it leaves it or when the setting goes off.
//!
//! The supervisor holds the `ssh` child of every reader, so a stop kills the child by
//! the process id that it holds and joins the reader within its bound. A stopped
//! watch leaves nothing: the machine leaves the snapshot with its rows. This computer
//! watches at most [`MAX_WATCHED_MACHINES`] machines, and a machine that it stops for
//! the setting keeps its row with the link [`WatchLink::Off`].
//!
//! The supervisor starts and joins threads, so its calls mark them with
//! [`crate::ui_thread::blocking`]: a frame reads [`MachineWatch::snapshot`] and never
//! reconciles.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Child;
use std::sync::mpsc::{self};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{JoinHandle, sleep};
use std::time::{Duration, Instant};

use crate::machines::Machines;
use crate::ssh::MachineKeys;

use super::remote::WatchTarget;
use super::snapshot::{Shared, WatchLink, WatchRefusal, unix_millis};
use super::{MAX_WATCHED_MACHINES, STOP_TIMEOUT};

/// The program of the watch of this computer: the `ssh` of its `PATH`.
const SSH_PROGRAM: &str = "ssh";

/// How long a stop waits for its reader before it kills the `ssh` child.
const STOP_GRACE: Duration = Duration::from_millis(150);

/// How often a stop looks for the end of its reader.
const STOP_POLL: Duration = Duration::from_millis(10);

/// The watch of the machines of one Horizon.
#[derive(Debug)]
pub struct MachineWatch {
    shared: Arc<Shared>,
    program: String,
    readers: Mutex<BTreeMap<String, Watch>>,
}

/// One running reader of one machine.
#[derive(Debug)]
struct Watch {
    target: WatchTarget,
    /// The stop of this reader, which ends its wait and its loop.
    stop: mpsc::Sender<()>,
    reader: JoinHandle<()>,
    /// The `ssh` child while it runs, so the supervisor can kill it.
    child: Arc<Mutex<Option<Child>>>,
}

impl MachineWatch {
    /// The watch of this computer, with the `ssh` of its `PATH`.
    #[must_use]
    pub fn new() -> Self {
        Self::with_program(SSH_PROGRAM)
    }

    /// The watch of this computer with another program for its `ssh` children, which
    /// a test of this module names.
    #[must_use]
    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            shared: Arc::new(Shared::new()),
            program: program.into(),
            readers: Mutex::new(BTreeMap::new()),
        }
    }

    /// The snapshot of this moment, which a frame reads and holds.
    #[must_use]
    pub fn snapshot(&self) -> Arc<super::MachineWatchSnapshot> {
        self.shared.snapshot()
    }

    /// The programs of the agent kinds of this watch, which a preset of the settings
    /// may add to.
    pub fn set_programs(&self, programs: super::AgentPrograms) {
        self.shared.set_programs(programs);
    }

    /// Records that the person looked at the agent of `session` and `pane` of
    /// `machine`. Answers whether any row changed.
    pub fn mark_seen(&self, machine: &str, session: &str, pane: &str) -> bool {
        self.shared.mark_seen(machine, session, pane)
    }

    /// Makes the readers of the watch match the machine list of this computer: a machine
    /// that the list holds and the limit allows gets one reader, a machine whose record
    /// changed gets a new one, and every other reader stops. `enabled` is the setting
    /// `machines.watch_agents`: with it off, every reader stops and every machine of the
    /// list says that the watch of this computer is off.
    pub fn reconcile(&self, machines: &Machines, keys: &MachineKeys, enabled: bool) {
        crate::ui_thread::blocking("the watch of the machines starts and stops its readers");
        let mut readers = self.lock();
        let mut wanted: Vec<WatchTarget> = Vec::new();
        let mut unwanted: Vec<String> = Vec::new();
        for (index, record) in machines.records().iter().enumerate() {
            if !enabled {
                unwanted.push(record.name.clone());
                self.shared.set_link(&record.name, WatchLink::Off);
                continue;
            }
            if index >= MAX_WATCHED_MACHINES {
                unwanted.push(record.name.clone());
                self.shared.set_link(&record.name, WatchLink::OverTheLimit);
                continue;
            }
            match WatchTarget::of(machines, keys, &record.name) {
                Ok(target) => wanted.push(target),
                Err(error) => {
                    unwanted.push(record.name.clone());
                    let link = WatchLink::Refused {
                        reason: WatchRefusal::Record,
                        since_millis: unix_millis(),
                    };
                    self.shared.set_link(&record.name, link);
                    tracing::warn!(machine = %record.name, %error, "the record of a machine cannot be watched");
                }
            }
        }
        for name in &unwanted {
            if let Some(watch) = readers.remove(name) {
                stop_watch(name, watch);
            }
        }
        let keep: BTreeSet<String> = wanted.iter().map(|target| target.machine.clone()).collect();
        let left: Vec<String> = readers.keys().filter(|name| !keep.contains(*name)).cloned().collect();
        for name in left {
            if let Some(watch) = readers.remove(&name) {
                stop_watch(&name, watch);
            }
            self.shared.remove(&name);
        }
        for target in wanted {
            let restart = readers.get(&target.machine).is_some_and(|watch| watch.target != target);
            if restart && let Some(watch) = readers.remove(&target.machine) {
                stop_watch(&target.machine, watch);
            }
            if !readers.contains_key(&target.machine) {
                self.start(&mut readers, target);
            }
        }
    }

    /// Stops every reader of this watch and forgets every machine of its snapshot. A
    /// watch without a reader waits for nothing, so the drop of one inside a frame is
    /// no call that can wait.
    pub fn stop_all(&self) {
        let mut readers = self.lock();
        if !readers.is_empty() {
            crate::ui_thread::blocking("the watch of the machines stops its readers");
        }
        let names: Vec<String> = readers.keys().cloned().collect();
        for name in names {
            if let Some(watch) = readers.remove(&name) {
                stop_watch(&name, watch);
            }
            self.shared.remove(&name);
        }
    }

    /// Starts one reader of `target`, with the link of a watch that has not reported
    /// yet.
    fn start(&self, readers: &mut BTreeMap<String, Watch>, target: WatchTarget) {
        self.shared.set_link(&target.machine, WatchLink::Connecting);
        let (stop, stopped) = mpsc::channel();
        let child = Arc::new(Mutex::new(None));
        let shared = Arc::clone(&self.shared);
        let program = self.program.clone();
        let name = target.machine.clone();
        let started = {
            let target = target.clone();
            let child = Arc::clone(&child);
            std::thread::Builder::new()
                .name(format!("machine-watch-{name}"))
                .spawn(move || super::reader::run(&target, &program, &shared, &stopped, &child))
        };
        match started {
            Ok(reader) => {
                readers.insert(
                    name,
                    Watch {
                        target,
                        stop,
                        reader,
                        child,
                    },
                );
            }
            Err(error) => {
                tracing::warn!(machine = %target.machine, %error, "the watch of a machine cannot start its reader");
                self.shared.set_link(
                    &target.machine,
                    WatchLink::Refused {
                        reason: WatchRefusal::Other,
                        since_millis: unix_millis(),
                    },
                );
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Watch>> {
        self.readers.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Default for MachineWatch {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MachineWatch {
    fn drop(&mut self) {
        self.stop_all();
    }
}

/// Stops one reader: it asks the reader to end, kills its `ssh` child when the reader
/// does not end in [`STOP_GRACE`], and joins it within [`STOP_TIMEOUT`].
fn stop_watch(machine: &str, watch: Watch) {
    let _ = watch.stop.send(());
    if !wait_for_reader(&watch.reader, STOP_GRACE)
        && let Some(child) = watch.child.lock().unwrap_or_else(PoisonError::into_inner).as_mut()
    {
        let _ = child.kill();
    }
    if !wait_for_reader(&watch.reader, STOP_TIMEOUT) {
        tracing::warn!(machine, "the reader of a machine outlives its stop");
        return;
    }
    let _ = watch.reader.join();
}

/// Whether the reader of one watch ended within `limit`.
fn wait_for_reader(reader: &JoinHandle<()>, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if reader.is_finished() {
            return true;
        }
        sleep(STOP_POLL);
    }
    reader.is_finished()
}
