//! The terminal that a press on the row of an agent of a machine opens, and the jump to
//! the panel that already shows the session of that agent.
//!
//! The panel is the one that a click on the machine row opens, with the session of the
//! agent in the place of the record's, in the workspace of its machine. A session name
//! reaches that command line only through Horizon's own text for it, and the press is
//! the look of the person at that agent.

use egui::Context;
use horizon_core::PanelId;
use horizon_core::machine_watch::{MachineAgent, MachineWatchSnapshot};

use super::HorizonApp;

impl HorizonApp {
    /// What a click on the row of `agent` does: it goes to the panel that shows the
    /// session of the agent, or opens one, and it is the person's look at that agent.
    pub(in crate::app) fn open_machine_agent(&mut self, agent: &MachineAgent, ctx: &Context) {
        if let Some(panel) = self.session_panel(&agent.machine, &agent.session) {
            self.reveal_selected_panel(ctx, panel);
            self.mark_machine_agent_seen(agent);
            return;
        }
        self.open_machine_terminal(&agent.machine, Some(agent), ctx);
    }

    /// The panel that shows the session `session` of `machine`, which a click on the
    /// row of its agent goes to.
    fn session_panel(&self, machine: &str, session: &str) -> Option<PanelId> {
        self.board
            .panels
            .iter()
            .find(|panel| panel.machine() == Some(machine) && panel.machine_session() == Some(session))
            .map(|panel| panel.id)
    }

    /// Records the person's look at one agent of a machine. Answers whether a row of
    /// the agent changed.
    pub(in crate::app) fn mark_machine_agent_seen(&self, agent: &MachineAgent) -> bool {
        self.machine_watch
            .watch
            .mark_seen(&agent.machine, &agent.session, &agent.pane)
    }

    /// Records the look of the person at the agents whose session the panel with the
    /// keyboard focus shows, on the frame that the focus moves or that the snapshot of
    /// the watch changes: a turn that ends in the panel that the person works in is no
    /// `Done`. The rule is the one of a local panel, which is the focus of the board
    /// alone. Answers whether a row changed.
    pub(in crate::app) fn look_at_machine_session(&mut self, snapshot: &MachineWatchSnapshot) -> bool {
        let focused = self.board.focused;
        if focused == self.machine_watch.looked_at && Some(snapshot.version) == self.machine_watch.looked_version {
            return false;
        }
        self.machine_watch.looked_at = focused;
        self.machine_watch.looked_version = Some(snapshot.version);
        let Some(panel) = focused.and_then(|panel| self.board.panel(panel)) else {
            return false;
        };
        let (Some(machine), Some(session)) = (panel.machine(), panel.machine_session()) else {
            return false;
        };
        snapshot
            .agents(machine)
            .iter()
            .filter(|agent| agent.session == session)
            .fold(false, |changed, agent| changed | self.mark_machine_agent_seen(agent))
    }
}
