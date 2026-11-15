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

use horizon_core::MachineKeys;
use horizon_core::machine_watch::{AgentPrograms, MachineWatch, MachineWatchSnapshot};

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
    pub(in crate::app) fn refresh_machine_watch(&mut self) {
        self.watch_programs();
        self.reconcile_watch();
        self.machine_watch.snapshot = Some(self.machine_watch.watch.snapshot());
        self.look_at_machine_session();
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

#[cfg(test)]
mod tests;
