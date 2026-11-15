//! The terminal that a machine record opens, for an agent and for a click.
//!
//! A record is agent-written text, so Horizon never replays a saved command: it
//! re-checks the route and rebuilds the tmux command from the record
//! ([`horizon_core::Machine::checked_terminal`]), and it re-derives the key path
//! from the key name with [`horizon_core::MachineKeys::require`]. The key check
//! reads the file system, so it runs on the coordination worker and the panel
//! opens on the frame that applies its answer. A click, a press at the row of an
//! agent and an agent's `open_terminal { machine }` build the same panel from
//! [`HorizonApp::open_machine_terminal`] and [`HorizonApp::create_machine_panel`],
//! so a second click focuses the panel of the first instead of opening a second
//! one.

use std::path::Path;

use egui::Context;
use horizon_browser_control::manifest::block;
use horizon_core::machine_watch::MachineAgent;
use horizon_core::{
    BlockPanelState, MachineError, MachineKeyRef, MachineKeys, PanelId, PanelKind, PanelOptions, SshConnection,
    WorkspaceId, WorkspacePlace,
};

use super::HorizonApp;
use super::block_requests::BlockAnswer;
use super::browser_requests::ActorPanel;
use super::machine_click::{OPENED_FROM_MACHINES, deleted_key_notice};

impl HorizonApp {
    /// Answers `open_terminal { machine }`: opens the terminal of the record in
    /// the caller's workspace, beside the calling agent panel, without taking
    /// keyboard focus.
    pub(super) fn answer_machine_terminal(
        &mut self,
        name: &str,
        title: Option<&str>,
        actor: ActorPanel,
        request: &block::Request,
        root: Option<&Path>,
        ctx: &Context,
    ) {
        let Some(record) = self.machine_list.find(name).cloned() else {
            self.answer_machine_now(root, request, ctx, machine_gone(name));
            return;
        };
        let terminal = match record.checked_terminal() {
            Ok(terminal) => terminal,
            Err(error) => {
                self.answer_machine_now(root, request, ctx, super::block_machines::refused(&error));
                return;
            }
        };
        if !self.agent_opens.terminals.allows(&request.actor) {
            self.answer_machine_now(
                root,
                request,
                ctx,
                block::Outcome::failed(
                    "too_many_terminals",
                    &format!(
                        "This agent already opened the maximum of {} terminals in this Horizon run; close one of them and ask the person to open another",
                        block::MAX_TERMINALS_PER_OWNER
                    ),
                ),
            );
            return;
        }
        let keys = MachineKeys::from_home(self.session_store.home());
        let agent_name = self
            .board
            .panel(actor.panel_id)
            .map_or("Agent", |panel| panel.kind.display_name())
            .to_owned();
        let agent_actor = self
            .board
            .panel(actor.panel_id)
            .map(|panel| horizon_core::browser_actor(&panel.local_id));
        let (name, title) = (name.to_owned(), title.map_or_else(|| record.title(), str::to_owned));
        let (root, request) = (root.map(Path::to_path_buf), request.clone());
        let (writing, owner, workspace) = (ctx.clone(), request.actor.clone(), actor.workspace_id);
        let key = self.machine_list.key(name.as_str()).map(str::to_owned);
        self.block_claim.worker_job_started();
        self.browser_create_host.io.then(
            move || required_key(&keys, key.as_deref()),
            move |app, required| {
                app.block_claim.worker_job_finished();
                let outcome = match required {
                    None => stopped(),
                    Some(Err(error)) => super::block_machines::refused(&error),
                    Some(Ok(key)) => {
                        let options = machine_panel_options(
                            &name,
                            &title,
                            connection_for(&terminal, key.as_ref()),
                            Some(agent_name),
                            agent_actor,
                        );
                        let focused = app.board.focused;
                        match app.create_machine_panel(workspace, options, Some(actor.panel_id)) {
                            Ok(panel_id) => {
                                // The person keeps the keyboard where it was.
                                if let Some(focused) = focused {
                                    app.board.focus(focused);
                                }
                                app.agent_opens.terminals.record(&owner);
                                block::Outcome::Opened { panel_id }
                            }
                            Err(error) => {
                                // The agent reads its own typed failure; the
                                // reason stays in the log, because the agent's
                                // path reports no notice to the person.
                                tracing::error!(%error, "failed to open the terminal of a machine");
                                block::Outcome::failed(
                                    "terminal_unavailable",
                                    "Horizon could not open the SSH terminal of this machine",
                                )
                            }
                        }
                    }
                };
                app.write_block_answer(BlockAnswer::new(root.as_deref(), request, outcome));
                writing.request_repaint();
            },
        );
    }

    /// Opens the terminal of the machine `name`: the terminal of its record in the
    /// workspace that is active or, when `agent` names a session of that machine, the
    /// terminal of that session in the workspace of the machine.
    ///
    /// The route passes the rule of an SSH terminal again and the key of the record is
    /// read on the coordination worker. The panel takes the keyboard, and a press at the
    /// row of an agent is the look of the person at that agent.
    pub(in crate::app) fn open_machine_terminal(&mut self, name: &str, agent: Option<&MachineAgent>, ctx: &Context) {
        let Some(record) = self.machine_list.find(name).cloned() else {
            self.machine_refused(
                name,
                &MachineError::Unavailable {
                    reason: format!("This Horizon has no machine {name} anymore"),
                },
            );
            return;
        };
        let terminal = match record.checked_terminal() {
            Ok(terminal) => terminal,
            Err(error) => {
                self.machine_refused(name, &error);
                return;
            }
        };
        // A session name outside the rule of this build is no word of a command line, so
        // its row says why it opens nothing and this call is never asked for it.
        let command = match agent.map(MachineAgent::open_command) {
            Some(Ok(command)) => Some(command),
            Some(Err(_)) => return,
            None => None,
        };
        let workspace = match (agent, self.board.active_workspace) {
            (Some(_), _) => None,
            (None, Some(workspace)) => Some(workspace),
            (None, None) => {
                self.set_sidebar_notice(format!("{name}: there is no workspace to open a terminal in"));
                return;
            }
        };
        let keys = MachineKeys::from_home(self.session_store.home());
        let (name, title) = (
            name.to_owned(),
            agent.map_or_else(|| record.title(), |agent| agent.name.clone()),
        );
        let key = self.machine_list.key(&name).map(str::to_owned);
        let bound = key.is_some();
        let session = agent.cloned();
        let opening = ctx.clone();
        self.browser_create_host.io.then(
            move || required_key(&keys, key.as_deref()),
            move |app, required| {
                let key = match required {
                    Some(Ok(key)) => key,
                    Some(Err(error))
                        if bound && error.code() == horizon_browser_control::manifest::block_keys::KEY_UNAVAILABLE =>
                    {
                        // A record that binds a key which this computer no longer holds is
                        // a key that a person deleted here or on another computer, and the
                        // only login left is the one of the person's own keys.
                        app.set_sidebar_notice(deleted_key_notice(&name));
                        opening.request_repaint();
                        return;
                    }
                    Some(Err(error)) => {
                        app.machine_refused(&name, &error);
                        opening.request_repaint();
                        return;
                    }
                    None => return,
                };
                let workspace = workspace.unwrap_or_else(|| app.machine_workspace(&name, &opening));
                let mut terminal = terminal;
                // The session of the agent replaces the one of the record.
                if let Some(command) = command {
                    terminal.remote_command = Some(command);
                }
                let mut options = machine_panel_options(
                    &name,
                    &title,
                    connection_for(&terminal, key.as_ref()),
                    Some(OPENED_FROM_MACHINES.to_owned()),
                    // A person pressed the row, so no agent panel opened it.
                    None,
                );
                options.machine_session = session.as_ref().map(|agent| agent.session.clone());
                // The person pressed the row, so the reason of a terminal that did not
                // start is theirs to read, not the log's only, and the terminal comes into
                // view as the panel of a second press does.
                match app.create_machine_panel(workspace, options, None) {
                    Ok(panel) => {
                        if let Some(panel) = app.board.panel_id_by_local_id(&panel) {
                            app.reveal_selected_panel(&opening, panel);
                        }
                    }
                    Err(error) => app.set_sidebar_notice(format!("{title}: {error}")),
                }
                if let Some(agent) = &session {
                    app.mark_machine_agent_seen(agent);
                }
                opening.request_repaint();
            },
        );
    }

    /// The workspace of `machine`: the one that carries its name, or a new one whose
    /// terminals and cards run on that machine.
    ///
    /// A workspace that holds no panel has no arrangement of the person's yet, so it
    /// starts free: the cards of a machine fit themselves there, as the card of a panel's
    /// workspace does, and a preset that the person picks later stays theirs.
    pub(in crate::app) fn machine_workspace(&mut self, machine: &str, ctx: &Context) -> WorkspaceId {
        let workspace = self.ensure_workspace_of_name(ctx, machine);
        if self
            .board
            .workspace(workspace)
            .is_some_and(|workspace| workspace.panels.is_empty())
        {
            self.board.clear_workspace_layout(workspace);
        }
        self.board
            .set_workspace_place(workspace, WorkspacePlace::Machine(machine.to_owned()));
        self.mark_runtime_dirty();
        workspace
    }

    /// Creates one terminal of a machine in `workspace` and returns its local id.
    /// `beside` puts the panel next to the calling agent panel.
    ///
    /// # Errors
    ///
    /// The reason of the terminal that did not start, in the words of the PTY or
    /// of the system. A person's click says it in the sidebar; an agent reads it
    /// in its own typed answer.
    pub(super) fn create_machine_panel(
        &mut self,
        workspace: WorkspaceId,
        options: PanelOptions,
        beside: Option<PanelId>,
    ) -> horizon_core::Result<String> {
        if let Some(beside) = beside {
            return self.create_machine_panel_beside(workspace, options, beside);
        }
        let id = self.board.create_panel_waiting(options, workspace)?;
        self.give_the_keyboard_to(id);
        self.mark_runtime_dirty();
        self.board
            .panel(id)
            .map(|panel| panel.local_id.clone())
            .ok_or_else(|| horizon_core::Error::State("The new terminal is not on the board".into()))
    }

    /// Creates one terminal of a machine next to the calling agent panel, without
    /// taking the keyboard focus of the person.
    fn create_machine_panel_beside(
        &mut self,
        workspace: WorkspaceId,
        options: PanelOptions,
        beside: PanelId,
    ) -> horizon_core::Result<String> {
        let id = self.board.create_panel_beside_waiting(options, workspace, beside)?;
        self.mark_runtime_dirty();
        self.board
            .panel(id)
            .map(|panel| panel.local_id.clone())
            .ok_or_else(|| horizon_core::Error::State("The new terminal is not on the board".into()))
    }

    /// Writes one answer that the frame already knows, such as a refusal that
    /// needs no file work.
    fn answer_machine_now(
        &mut self,
        root: Option<&Path>,
        request: &block::Request,
        ctx: &Context,
        outcome: block::Outcome,
    ) {
        self.write_block_answer(BlockAnswer::new(root, request.clone(), outcome));
        ctx.request_repaint();
    }
}

/// The key of a record, checked on the coordination worker. A record without a
/// key connects with the person's own keys and agent, as `open_terminal` does.
pub(super) fn required_key(keys: &MachineKeys, key: Option<&str>) -> Result<Option<MachineKeyRef>, MachineError> {
    match key {
        Some(key) => keys.require(key).map(Some).map_err(MachineError::from),
        None => Ok(None),
    }
}

/// The connection of a machine terminal, with the key of the record when it has
/// one. A click, an agent's request and a placed workspace terminal all build
/// this connection, so all three hold the bounds of a machine.
pub(super) fn connection_for(
    terminal: &horizon_browser_control::manifest::block_terminal::CheckedTerminal,
    key: Option<&MachineKeyRef>,
) -> SshConnection {
    key.map_or_else(
        || SshConnection::for_agent_terminal(terminal),
        |key| SshConnection::for_agent_terminal_with_key(terminal, key),
    )
    .as_machine_terminal()
}

/// The options of a terminal that a machine opens: an SSH panel tied to the
/// machine name, with the connection that Horizon built and the mark of who
/// opened it.
pub(super) fn machine_panel_options(
    name: &str,
    title: &str,
    connection: SshConnection,
    opened_by: Option<String>,
    opened_by_actor: Option<String>,
) -> PanelOptions {
    PanelOptions {
        kind: PanelKind::Ssh,
        name: Some(BlockPanelState::normalize_title(title)),
        ssh_connection: Some(connection),
        opened_by,
        opened_by_actor,
        machine: Some(name.to_owned()),
        local_id: Some(horizon_core::new_local_id()),
        ..PanelOptions::default()
    }
}

/// The answer of a machine that this Horizon no longer keeps.
fn machine_gone(name: &str) -> block::Outcome {
    block::Outcome::failed(
        horizon_core::machines::MACHINE_UNAVAILABLE,
        &format!(
            "This Horizon has no machine {name}; call machines to list the ones it has, or pass ssh for a host that is not in the list"
        ),
    )
}

/// The answer of a worker job that ended before it answered.
fn stopped() -> block::Outcome {
    block::Outcome::failed(
        horizon_core::machines::MACHINE_UNAVAILABLE,
        "The Horizon worker that keeps the machine list stopped before it answered; call machines again",
    )
}

#[cfg(test)]
mod tests;
