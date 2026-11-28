//! The four worker operations of `block_panel`: the start of one worker of an
//! orchestrator, the read of its workers, the message to one of them, and the
//! report of a worker to the orchestrator that started it.
//!
//! A worker is an agent panel of the workspace of the orchestrator, and Horizon
//! opens it through the path of a preset of the person's settings: the command,
//! the arguments and the kind come from that preset, the folder is the folder of
//! the workspace, and the panel takes no keyboard. The request names the preset
//! and one task, and the task becomes the first prompt line of the panel, with
//! the sentence that tells the worker how to report.
//!
//! The programs that the chip `Workers` of the person holds start at once. A
//! request for any other program starts nothing: the conversation of the
//! orchestrator gets one row that the person decides, and Horizon sends one line
//! to the program when the person pressed one of its controls.
//!
//! Every text of these operations is text of a program. Horizon types the task and
//! each message into a terminal as one line and passes each report to the program
//! of the orchestrator as one turn, so each is bounded before the request reaches
//! the dock.

use std::time::Instant;

use egui::Context;
use horizon_browser_control::manifest::block::Outcome;
use horizon_browser_control::manifest::block_machines;
use horizon_browser_control::manifest::block_preset;
use horizon_browser_control::manifest::block_workers::{
    MAX_WORKERS, NOT_ALLOWED, WorkerSummary, check_message, check_report, check_task, task_line, worker_prompt,
};
use horizon_core::agent_bridge::WorkerAskState;
use horizon_core::{AgentStatus, PresetConfig, preset_named};

use super::super::HorizonApp;
use super::super::block_panels::preset_kind_name;
use super::super::orchestrator_dock::WorkerRecord;
use super::caller::{CardCaller, caller_unavailable};
use super::work_items::PROPOSAL_ANSWER;

/// The mark of the titlebar of a worker: the text of Horizon that names the role
/// of that panel.
pub(in crate::app) const WORKER_MARK: &str = "worker of the orchestrator";

/// The first words of the line that the program of the orchestrator reads after a
/// press on the row of a request.
const STARTED_LINE: &str = "The person started the worker of request";
const NOT_STARTED_LINE: &str = "The person did not start the worker of request";

/// What the person pressed on the row of a request for a worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum WorkerAnswer {
    /// Start this one worker.
    Start,
    /// Add the program to the chip and start the worker.
    Always,
    /// Start nothing.
    NotNow,
}

impl HorizonApp {
    /// Answers `worker_start`: checks the preset, the chip and the bound, and opens
    /// one worker of the conversation of the calling orchestrator.
    pub(in crate::app) fn answer_worker_start(
        &mut self,
        program: &str,
        task: &str,
        caller: CardCaller<'_>,
        ctx: &Context,
    ) -> Outcome {
        let Some(local_id) = self.worker_owner_local_id(caller) else {
            return caller_unavailable();
        };
        if let Err(refusal) = check_task(task) {
            return Outcome::failed(refusal.code, &refusal.message);
        }
        let name = match block_preset::check_name(program) {
            Ok(name) => name,
            Err(refusal) => return Outcome::failed(refusal.code, &refusal.message),
        };
        let Some(preset) = preset_named(&self.template_config.presets, name).cloned() else {
            return Outcome::failed(
                block_preset::UNKNOWN_PRESET,
                &block_preset::unknown_preset_message(&self.preset_names()),
            );
        };
        if !preset.kind.is_agent() {
            return Outcome::failed(
                block_machines::INVALID_REQUEST,
                &format!(
                    "The preset {} starts no agent program, and a worker is an agent program",
                    preset.name
                ),
            );
        }
        if !self
            .orchestrator_dock
            .workers_allowed(&local_id)
            .iter()
            .any(|held| held == name)
        {
            if self.orchestrator_dock.worker_ask_declined(&local_id, &preset.name) {
                return Outcome::failed(
                    NOT_ALLOWED,
                    &format!(
                        "The person said Not now to a worker of {}, and this work item starts none of that program",
                        preset.name
                    ),
                );
            }
            // The person decides which programs this orchestrator starts, so this
            // request starts nothing: it becomes one row that they answer.
            let request = horizon_core::new_local_id();
            self.orchestrator_dock
                .worker_ask(&local_id, &request, &preset.name, task);
            self.mark_runtime_dirty();
            ctx.request_repaint();
            return Outcome::WorkerAsked {
                request,
                program: preset.name.clone(),
                text: PROPOSAL_ANSWER.to_owned(),
            };
        }
        self.start_worker(&local_id, &preset, task, ctx)
    }

    /// Answers `workers`: the workers of the calling orchestrator, with the state
    /// of the agent of each panel and the last report of each.
    pub(in crate::app) fn answer_workers(&self, caller: CardCaller<'_>) -> Outcome {
        let Some(local_id) = self.worker_owner_local_id(caller) else {
            return caller_unavailable();
        };
        let workers = self
            .orchestrator_dock
            .workers_of(&local_id)
            .iter()
            .map(|worker| WorkerSummary {
                worker: worker.panel.clone(),
                program: worker.program.clone(),
                kind: worker.kind.clone(),
                state: self.worker_state_of(&worker.panel),
                last_report: worker.last_report().map(str::to_owned),
            })
            .collect();
        Outcome::Workers { workers }
    }

    /// Answers `worker_message`: types one message into the terminal of one worker
    /// of the calling orchestrator, as the first prompt line of that worker is
    /// typed.
    pub(in crate::app) fn answer_worker_message(
        &mut self,
        worker: &str,
        text: &str,
        caller: CardCaller<'_>,
        ctx: &Context,
    ) -> Outcome {
        let Some(local_id) = self.worker_owner_local_id(caller) else {
            return caller_unavailable();
        };
        if let Err(refusal) = check_message(text) {
            return Outcome::failed(refusal.code, &refusal.message);
        }
        let now = Instant::now();
        let Some(program) = self.worker_program(&local_id, worker) else {
            return Outcome::failed(
                "not_found",
                "This orchestrator started no worker of that id; call workers to read it",
            );
        };
        match self.orchestrator_dock.worker_message_due(&local_id, worker, now) {
            Some(true) => {}
            Some(false) => {
                return Outcome::failed(
                    "too_soon",
                    "One message to one worker every ten seconds; that worker read a message of yours a moment ago",
                );
            }
            None => {
                return Outcome::failed(
                    "not_found",
                    "This orchestrator started no worker of that id; call workers to read it",
                );
            }
        }
        let Some(panel) = self.board.panel_id_by_local_id(worker) else {
            return Outcome::failed("not_found", "The panel of that worker is not in this workspace");
        };
        // The message takes the one path of a prompt line of an agent panel: it goes
        // out as a person's paste and Enter once the program of the panel is at its
        // prompt.
        self.agent_opens.panels.hold_line(panel, &task_line(text), now);
        self.orchestrator_dock.worker_message(&local_id, worker, &program, text);
        self.orchestrator_dock.note_worker_message(&local_id, worker, now);
        self.mark_runtime_dirty();
        ctx.request_repaint();
        Outcome::WorkerMessage {
            worker: worker.to_owned(),
            program,
        }
    }

    /// Answers `worker_report`: passes one report of one worker to the orchestrator
    /// that started it as one turn, in the queue of the lines of that conversation.
    pub(in crate::app) fn answer_worker_report(
        &mut self,
        text: &str,
        caller: CardCaller<'_>,
        ctx: &Context,
    ) -> Outcome {
        if let Err(refusal) = check_report(text) {
            return Outcome::failed(refusal.code, &refusal.message);
        }
        // A report belongs to the worker alone: an agent panel that this Horizon
        // started as a worker of an orchestrator whose program runs now, and no
        // other panel of this host.
        let CardCaller::Panel(actor) = caller else {
            return caller_unavailable();
        };
        let Some(panel) = self.board.panel(actor.panel_id).map(|panel| panel.local_id.clone()) else {
            return caller_unavailable();
        };
        let Some(owner) = self.orchestrator_dock.worker_owner(&panel) else {
            return caller_unavailable();
        };
        if !self.orchestrator_dock.running(&owner.local_id) {
            return caller_unavailable();
        }
        let now = Instant::now();
        if !self.orchestrator_dock.worker_report_due(&owner.local_id, &panel, now) {
            return Outcome::failed(
                "too_soon",
                "One report of one worker every ten seconds; your last report went out a moment ago",
            );
        }
        let line = report_line(&owner.program, &panel, text);
        if !self
            .orchestrator_dock
            .queue_worker_report(&owner.local_id, &panel, &line, text, now)
        {
            return Outcome::failed(
                "busy",
                "The orchestrator of this worker has eight reports that wait for the end of its turn",
            );
        }
        self.orchestrator_dock
            .worker_report(&owner.local_id, &panel, &owner.program, text);
        self.mark_runtime_dirty();
        ctx.request_repaint();
        Outcome::WorkerReported {
            worker: panel,
            program: owner.program,
        }
    }

    /// Starts one worker of the conversation of `local_id` from `preset`, with one
    /// task, and answers the id of its panel.
    pub(in crate::app) fn start_worker(
        &mut self,
        local_id: &str,
        preset: &PresetConfig,
        task: &str,
        ctx: &Context,
    ) -> Outcome {
        let Some(workspace) = self.board.workspace_id_by_local_id(local_id) else {
            return caller_unavailable();
        };
        if !self.orchestrator_dock.worker_slot_free(local_id) {
            return Outcome::failed(
                horizon_browser_control::manifest::block_workers::TOO_MANY_WORKERS,
                &format!(
                    "This orchestrator runs the maximum of {MAX_WORKERS} workers; the person closes one panel before another starts"
                ),
            );
        }
        let directory = self
            .board
            .workspace(workspace)
            .and_then(|workspace| workspace.cwd.clone())
            .unwrap_or_else(|| std::env::var_os("HOME").map_or_else(std::path::PathBuf::new, std::path::PathBuf::from));
        let mut options = super::super::actions::support::preset_panel_options(preset, &self.template_config.browser);
        options.name = Some(preset.name.clone());
        options.cwd = Some(directory);
        // The mark is Horizon's own text: a worker is a panel as any other, and the
        // person reads which role it has.
        options.opened_by = Some(WORKER_MARK.to_owned());
        super::super::actions::panels::normalize_new_panel_resume(&mut options);
        let focused = self.board.focused;
        // The newest worker of this conversation is the neighbour of a new worker,
        // and an orchestrator without a worker has no neighbour to point at.
        let beside = self
            .orchestrator_dock
            .workers_of(local_id)
            .last()
            .and_then(|worker| self.board.panel_id_by_local_id(&worker.panel));
        let Ok(id) = self.create_agent_child_panel(options, workspace, beside) else {
            return Outcome::failed(
                block_preset::PANEL_UNAVAILABLE,
                "Horizon could not open the panel of the worker in this workspace",
            );
        };
        // A worker must not steal keyboard input from the person or the program that
        // asked for it.
        if let Some(focused) = focused {
            self.board.focus(focused);
        }
        let Some(panel) = self.board.panel(id).map(|panel| panel.local_id.clone()) else {
            return Outcome::failed(
                block_preset::PANEL_UNAVAILABLE,
                "The new panel is not in this workspace",
            );
        };
        self.agent_opens
            .panels
            .hold_line(id, &worker_prompt(task), Instant::now());
        self.orchestrator_dock.note_worker(
            local_id,
            WorkerRecord::new(&panel, &preset.name, &preset_kind_name(preset.kind)),
        );
        self.orchestrator_dock
            .worker_started(local_id, &panel, &preset.name, task);
        self.mark_runtime_dirty();
        ctx.request_repaint();
        Outcome::WorkerStarted {
            worker: panel,
            program: preset.name.clone(),
        }
    }

    /// Applies the press of the person on the row of a request for a worker: it
    /// starts the worker, or it starts nothing, and either way the program of the
    /// orchestrator of `local_id` reads one line.
    pub(in crate::app) fn press_worker_ask(&mut self, ctx: &Context, local_id: &str, id: &str, answer: WorkerAnswer) {
        let Some((program, task)) = self.orchestrator_dock.open_worker_ask(local_id, id) else {
            return;
        };
        let Some(preset) = preset_named(&self.template_config.presets, &program).cloned() else {
            return;
        };
        let state = match answer {
            WorkerAnswer::NotNow => WorkerAskState::Declined,
            WorkerAnswer::Always | WorkerAnswer::Start => {
                if answer == WorkerAnswer::Always {
                    // `Always for this work item` adds the program to the chip, so a
                    // later start of the same program needs no press.
                    self.orchestrator_dock.allow_worker_program(local_id, &preset.name);
                }
                match self.start_worker(local_id, &preset, &task, ctx) {
                    Outcome::WorkerStarted { .. } if answer == WorkerAnswer::Always => WorkerAskState::Always,
                    Outcome::WorkerStarted { .. } => WorkerAskState::Started,
                    // The worker did not start, so the row reads that the person did
                    // not start it, and the program reads the same line.
                    _ => WorkerAskState::Declined,
                }
            }
        };
        if !self.orchestrator_dock.set_worker_ask_state(local_id, id, state) {
            return;
        }
        let line = match state {
            WorkerAskState::Declined => format!("{NOT_STARTED_LINE} {id}."),
            _ => format!("{STARTED_LINE} {id}."),
        };
        self.orchestrator_dock.session_system_message(local_id, &line);
        self.mark_runtime_dirty();
        ctx.request_repaint();
    }

    /// The state word of the worker of the panel `panel`, as `workers` reports it.
    fn worker_state_of(&self, panel: &str) -> String {
        let status = self
            .board
            .panel_id_by_local_id(panel)
            .and_then(|id| self.board.panel(id))
            .map_or(AgentStatus::Idle, horizon_core::Panel::agent_status);
        match status {
            AgentStatus::Working => "working",
            AgentStatus::Idle => "idle",
        }
        .to_owned()
    }

    /// The name of the preset of the worker of the panel `panel` in the conversation
    /// of `local_id`.
    fn worker_program(&self, local_id: &str, panel: &str) -> Option<String> {
        self.orchestrator_dock
            .workers_of(local_id)
            .iter()
            .find(|held| held.panel == panel)
            .map(|held| held.program.clone())
    }

    /// The local id of the conversation of an orchestrator caller, while the dock
    /// runs its program for a workspace of this board.
    fn worker_owner_local_id(&self, caller: CardCaller<'_>) -> Option<String> {
        let CardCaller::Orchestrator { workspace } = caller else {
            return None;
        };
        self.board
            .workspace(workspace)
            .map(|workspace| workspace.local_id.clone())
    }
}

/// The one line that the program of an orchestrator reads when a worker reports:
/// the words of Horizon, the program of the worker, the id of its panel and the
/// report under them.
fn report_line(program: &str, panel: &str, text: &str) -> String {
    format!("Report of your worker {program} ({panel}):\n{text}")
}

#[cfg(test)]
mod tests {
    use super::report_line;

    #[test]
    fn the_line_of_a_report_names_the_program_the_panel_and_the_text() {
        assert_eq!(
            report_line("Codex", "p-1", "the redirect is fixed"),
            "Report of your worker Codex (p-1):\nthe redirect is fixed"
        );
    }
}
