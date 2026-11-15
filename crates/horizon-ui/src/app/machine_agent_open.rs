//! The terminal that a click on the row of an agent of a machine opens, and the jump to
//! the panel that already shows the session of that agent.
//!
//! The panel is the one that a click on the machine row opens, with the session of the
//! agent in the place of the record's: the route passes the rule of an SSH terminal
//! again, the key is checked on the coordination worker, and the command is Horizon's
//! own text for that session, so the session name is the only value that reaches it.
//!
//! The panel opens in the workspace of the machine, which carries its name, and its new
//! terminals run on that machine. The person's click is a look at the agent, so the
//! agent is seen.

use egui::Context;
use horizon_core::machine_watch::MachineAgent;
use horizon_core::{MachineKeys, PanelId, WorkspaceId, WorkspacePlace};

use super::HorizonApp;
use super::machine_click::OPENED_FROM_MACHINES;
use super::machine_open::{connection_for, machine_panel_options, required_key};

impl HorizonApp {
    /// What a click on the row of `agent` does: it goes to the panel that shows the
    /// session of the agent, or opens one, and it is the person's look at that agent.
    pub(in crate::app) fn open_machine_agent(&mut self, agent: &MachineAgent, ctx: &Context) {
        if let Some(panel) = self.session_panel(&agent.machine, &agent.session) {
            self.reveal_selected_panel(ctx, panel);
            self.mark_machine_agent_seen(agent);
            return;
        }
        // A session name outside the rule of this build is no word of a command line,
        // so its row says why it opens nothing and this call is never asked for it.
        let Ok(command) = agent.open_command() else {
            return;
        };
        let Some(record) = self.machine_list.find(&agent.machine).cloned() else {
            self.machine_refused(
                &agent.machine,
                &horizon_core::MachineError::Unavailable {
                    reason: format!("This Horizon has no machine {} anymore", agent.machine),
                },
            );
            return;
        };
        let terminal = match record.checked_terminal() {
            Ok(terminal) => terminal,
            Err(error) => {
                self.machine_refused(&agent.machine, &error);
                return;
            }
        };
        let keys = MachineKeys::from_home(self.session_store.home());
        let key = self.machine_list.key(agent.machine.as_str()).map(str::to_owned);
        let (agent, opening) = (agent.clone(), ctx.clone());
        self.browser_create_host.io.then(
            move || required_key(&keys, key.as_deref()),
            move |app, required| {
                let key = match required {
                    Some(Ok(key)) => key,
                    Some(Err(error)) => {
                        app.machine_refused(&agent.machine, &error);
                        opening.request_repaint();
                        return;
                    }
                    None => return,
                };
                let workspace = app.machine_workspace(&agent.machine, &opening);
                let mut terminal = terminal;
                terminal.remote_command = Some(command);
                let mut options = machine_panel_options(
                    &agent.machine,
                    &agent.name,
                    connection_for(&terminal, key.as_ref()),
                    Some(OPENED_FROM_MACHINES.to_owned()),
                    None,
                );
                options.machine_session = Some(agent.session.clone());
                if let Err(error) = app.create_machine_panel(workspace, options, None) {
                    app.set_sidebar_notice(format!("{}: {error}", agent.name));
                }
                app.mark_machine_agent_seen(&agent);
                opening.request_repaint();
            },
        );
    }

    /// The workspace of `machine`: the one that carries its name, or a new one whose
    /// terminals run on that machine.
    fn machine_workspace(&mut self, machine: &str, ctx: &Context) -> WorkspaceId {
        if let Some(workspace) = self.board.workspaces.iter().find(|workspace| workspace.name == machine) {
            return workspace.id;
        }
        let workspace = self.create_workspace_visible(ctx, machine);
        self.board
            .set_workspace_place(workspace, WorkspacePlace::Machine(machine.to_owned()));
        self.mark_runtime_dirty();
        workspace
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

    /// Records the person's look at one agent of a machine.
    pub(in crate::app) fn mark_machine_agent_seen(&self, agent: &MachineAgent) {
        self.machine_watch
            .watch
            .mark_seen(&agent.machine, &agent.session, &agent.pane);
    }

    /// Records the look of the person at the agents whose session the panel with the
    /// keyboard focus shows, on the frame that the focus moves.
    pub(in crate::app) fn look_at_machine_session(&mut self) {
        let focused = self.board.focused;
        if focused == self.machine_watch.looked_at {
            return;
        }
        self.machine_watch.looked_at = focused;
        let Some(panel) = focused.and_then(|panel| self.board.panel(panel)) else {
            return;
        };
        let (Some(machine), Some(session)) = (panel.machine(), panel.machine_session()) else {
            return;
        };
        let (machine, session) = (machine.to_owned(), session.to_owned());
        let snapshot = self.machine_watch.watch.snapshot();
        for agent in snapshot
            .agents(&machine)
            .iter()
            .filter(|agent| agent.session == session)
        {
            self.mark_machine_agent_seen(agent);
        }
    }
}
