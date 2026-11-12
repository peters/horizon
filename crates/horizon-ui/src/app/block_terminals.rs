//! The SSH terminal an agent asks Horizon to open for a person.
//!
//! `block_panel open_terminal` reaches a machine that the person uses, so the
//! host checks the route and the remote command again with the shared check,
//! builds the `ssh` command line itself, puts the panel beside the calling
//! agent panel, and leaves the keyboard focus where it was. The mark that the
//! panel carries afterwards is Horizon's own text, never a string from the
//! request. A terminal that names a machine key of the workspace connects with
//! Horizon's own key pair for that machine, and offers no other key.
//!
//! After the panel exists it is an ordinary SSH panel: the person types in it
//! and closes it, and the agent cannot read it, write to it or close it.

use std::collections::HashMap;

use egui::Context;
use horizon_browser_control::manifest::block;
use horizon_browser_control::manifest::block_terminal::CheckedTerminal;
use horizon_core::{BlockPanelState, MachineKeyRef, PanelKind, PanelOptions, SshConnection};

use super::HorizonApp;
use super::browser_requests::ActorPanel;

/// The terminals each agent panel opened in this Horizon run. Closing a
/// terminal does not free its place, so the bound holds for the whole run.
#[derive(Default)]
pub(in crate::app) struct AgentTerminals {
    opened: HashMap<String, usize>,
}

impl AgentTerminals {
    /// How many terminals `owner` opened in this run.
    pub(in crate::app) fn count(&self, owner: &str) -> usize {
        self.opened.get(owner).copied().unwrap_or(0)
    }

    /// Whether `owner` may open one more terminal.
    pub(in crate::app) fn allows(&self, owner: &str) -> bool {
        self.count(owner) < block::MAX_TERMINALS_PER_OWNER
    }

    /// Count one terminal that `owner` now has.
    pub(in crate::app) fn record(&mut self, owner: &str) {
        *self.opened.entry(owner.to_owned()).or_default() += 1;
    }
}

impl HorizonApp {
    /// Open an SSH terminal panel in the caller's workspace, beside the calling
    /// agent panel, without taking keyboard focus. `key` selects one machine key
    /// of the workspace, whose options Horizon adds to the command line.
    pub(super) fn open_terminal(
        &mut self,
        terminal: &CheckedTerminal,
        title: Option<&str>,
        key: Option<&MachineKeyRef>,
        actor: ActorPanel,
        owner: &str,
        ctx: &Context,
    ) -> block::Outcome {
        if !self.agent_opens.terminals.allows(owner) {
            return block::Outcome::failed(
                "too_many_terminals",
                &format!(
                    "This agent already opened the maximum of {} terminals in this Horizon run; close one of them and ask the person to open another",
                    block::MAX_TERMINALS_PER_OWNER
                ),
            );
        }
        let connection = key.map_or_else(
            || SshConnection::for_agent_terminal(terminal),
            |key| SshConnection::for_agent_terminal_with_key(terminal, key),
        );
        let name = title.map_or_else(|| connection.display_label(), BlockPanelState::normalize_title);
        let agent_name = self.opener_mark(actor);
        let options = PanelOptions {
            kind: PanelKind::Ssh,
            name: Some(name),
            ssh_connection: Some(connection),
            opened_by: Some(agent_name),
            ..PanelOptions::default()
        };
        let focused = self.board.focused;
        let Ok(id) = self.create_agent_child_panel(options, actor.workspace_id, actor.panel_id) else {
            return block::Outcome::failed(
                "terminal_unavailable",
                "Horizon could not open the SSH terminal in this workspace",
            );
        };
        // Opening a terminal for the person must not steal keyboard input from
        // the agent that asked for it.
        if let Some(focused) = focused {
            self.board.focus(focused);
        }
        self.agent_opens.terminals.record(owner);
        self.mark_runtime_dirty();
        ctx.request_repaint();
        let Some(panel) = self.board.panel(id) else {
            return block::Outcome::failed("terminal_unavailable", "The new SSH terminal is not in this workspace");
        };
        block::Outcome::Opened {
            panel_id: panel.local_id.clone(),
        }
    }
}

#[cfg(test)]
mod tests;
