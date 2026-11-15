//! The watch of the agents of the machines of the machine list, on the side of the
//! window: the worker that sets its readers up, the snapshot that a frame reads, and
//! the machines whose agents the person folded.
//!
//! `MachineWatch::reconcile` starts and joins threads, so it runs on the coordination
//! worker and never on a frame. A frame reads the snapshot, which is one `Arc` of the
//! watch, and the rows of the machine section are built from it only when its version
//! changed.
//!
//! The setting `machines.watch_agents` is part of what the readers are set up from, so
//! the row of the Settings window stops every watch on the frame that applies it.

use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::Context;
use horizon_core::MachineKeys;
use horizon_core::machine_watch::{AgentPrograms, MachineAgent, MachineWatch, MachineWatchSnapshot};
use horizon_core::notify::Change;

use super::HorizonApp;

/// The watch of the machines of one window, and what a frame holds of it.
pub(in crate::app) struct MachinesWatch {
    /// The watch of this computer. A test sets a watch with a stand-in `ssh`, so a
    /// unit test starts no real connection.
    pub(in crate::app) watch: Arc<MachineWatch>,
    /// The snapshot that the last frame read, so a changed state is a notice.
    pub(in crate::app) snapshot: Option<Arc<MachineWatchSnapshot>>,
    /// The records, the keys and the setting that the readers were set up from.
    reconciled: Option<u64>,
    /// The presets that the programs of the agent kinds came from.
    programs: Option<u64>,
    /// The machines whose agents the person folded.
    folded: BTreeSet<String>,
    /// The panel that had the keyboard focus at the last look at a machine terminal.
    pub(in crate::app) looked_at: Option<horizon_core::PanelId>,
}

impl MachinesWatch {
    /// The watch of this window, before its first frame.
    #[must_use]
    pub(in crate::app) fn new() -> Self {
        Self {
            watch: Arc::new(MachineWatch::new()),
            snapshot: None,
            reconciled: None,
            programs: None,
            folded: BTreeSet::new(),
            looked_at: None,
        }
    }

    /// Whether the person folded the agents of `machine`.
    pub(in crate::app) fn folded(&self, machine: &str) -> bool {
        self.folded.contains(machine)
    }

    /// Folds the agents of `machine`, or shows them again.
    pub(in crate::app) fn toggle_fold(&mut self, machine: &str) {
        if !self.folded.remove(machine) {
            self.folded.insert(machine.to_owned());
        }
    }
}

impl Default for MachinesWatch {
    fn default() -> Self {
        Self::new()
    }
}

impl HorizonApp {
    /// Sets the readers of the watch up when the machine list, a machine key, the
    /// setting or the presets changed, and reads the snapshot of this frame.
    ///
    /// A frame costs two fingerprints, one lock of the shared snapshot and no wait: the
    /// worker starts and joins the readers.
    pub(in crate::app) fn refresh_machine_watch(&mut self, ctx: &Context) {
        self.watch_programs();
        self.reconcile_watch();
        let next = self.machine_watch.watch.snapshot();
        if let Some(previous) = self.machine_watch.snapshot.replace(std::sync::Arc::clone(&next))
            && previous.version != next.version
        {
            self.note_machine_changes(&previous, &next, ctx);
        }
        self.look_at_machine_session();
    }

    /// Queues one desktop notification for each agent of a machine that changed to a
    /// state that waits for the person, while no window of Horizon has the focus: the
    /// same text as a panel of this computer, with the name of its machine.
    fn note_machine_changes(&mut self, previous: &MachineWatchSnapshot, next: &MachineWatchSnapshot, ctx: &Context) {
        let setting = self.template_config.features.notifications;
        if !setting.speaks() || super::notify::window_in_front(ctx) {
            return;
        }
        let now = std::time::Instant::now();
        for agent in moved_agents(previous, next) {
            let Some(kind) = super::notify::change_kind(agent.state.attention()) else {
                continue;
            };
            let change = Change {
                panel: agent.name.clone(),
                workspace: None,
                machine: Some(agent.machine.clone()),
                kind,
            };
            self.notifications.note_change(change, setting, now);
        }
    }

    /// Gives the watch the programs of the agent kinds of the presets, once for one
    /// set of presets.
    fn watch_programs(&mut self) {
        let presets = &self.template_config.presets;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for preset in presets {
            preset.kind.hash(&mut hasher);
            preset.command.hash(&mut hasher);
        }
        let fingerprint = hasher.finish();
        if self.machine_watch.programs == Some(fingerprint) {
            return;
        }
        self.machine_watch.programs = Some(fingerprint);
        self.machine_watch
            .watch
            .set_programs(AgentPrograms::builtin().with_presets(presets));
    }

    /// Starts and stops the readers of the watch, on the coordination worker, once for
    /// one machine list, one set of keys and one value of the setting.
    fn reconcile_watch(&mut self) {
        let enabled = self.template_config.machines.watch_agents;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        enabled.hash(&mut hasher);
        for machine in self.machine_list.records() {
            machine.name.hash(&mut hasher);
            machine.host.hash(&mut hasher);
            machine.user.hash(&mut hasher);
            machine.port.hash(&mut hasher);
            machine.session.hash(&mut hasher);
            self.machine_list.key(&machine.name).hash(&mut hasher);
        }
        let fingerprint = hasher.finish();
        if self.machine_watch.reconciled == Some(fingerprint) {
            return;
        }
        self.machine_watch.reconciled = Some(fingerprint);
        let watch = Arc::clone(&self.machine_watch.watch);
        let machines = self.machine_list.clone();
        let keys = MachineKeys::from_home(self.session_store.home());
        self.browser_create_host
            .io
            .write(move || watch.reconcile(&machines, &keys, enabled));
    }
}

/// The agents that a report moved: every agent of `next` whose state differs from the
/// one of the same agent in `previous`, or that `previous` did not name.
///
/// Both snapshots hold their machines in name order and the agents of a machine in
/// session and pane order, so one walk reads the two of them.
fn moved_agents<'a>(previous: &'a MachineWatchSnapshot, next: &'a MachineWatchSnapshot) -> Vec<&'a MachineAgent> {
    let mut before = previous.machines.iter().flat_map(|machine| &machine.agents).peekable();
    let mut moved = Vec::new();
    for agent in next.machines.iter().flat_map(|machine| &machine.agents) {
        while before.peek().is_some_and(|held| key(held) < key(agent)) {
            before.next();
        }
        let held = before.peek().filter(|held| key(held) == key(agent));
        if held.map(|held| held.state) != Some(agent.state) {
            moved.push(agent);
        }
    }
    moved
}

/// What tells one agent of a snapshot from another.
fn key(agent: &MachineAgent) -> (&str, &str, &str) {
    (agent.machine.as_str(), agent.session.as_str(), agent.pane.as_str())
}

#[cfg(test)]
mod tests;
