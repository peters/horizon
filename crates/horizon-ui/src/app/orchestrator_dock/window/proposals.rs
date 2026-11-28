//! The proposal of the front desk in the dock: the press that starts it or leaves
//! it, and the answer of the worker that read the folders of its work items.

use horizon_core::agent_bridge::ProposalState;

use crate::app::HorizonApp;

use super::super::FRONT_DESK;
use super::super::proposal::{self, Choice};

/// The notice of a press that started no work item, because no worker read the
/// folder of an item of the proposal.
const PROPOSAL_CHECK_FAILED: &str = "The work items did not start.";

/// The notice of an item whose folder was not a directory of this computer when
/// the person started the proposal.
fn folder_missing_notice(title: &str) -> String {
    format!("{title}: {}", proposal::FOLDER_MISSING)
}

impl HorizonApp {
    /// Applies the press of the person on the row of a proposal. A start checks the
    /// folders on the worker of the dock first; a decline decides at once.
    pub(in crate::app) fn press_proposal(&mut self, ctx: &egui::Context, id: &str, choice: Choice) {
        match choice {
            Choice::Start => {
                let Some(items) = self.orchestrator_dock.open_proposal(id) else {
                    return;
                };
                if !self.orchestrator_dock.begin_proposal_start(id, &items) {
                    // No worker took the check, so no folder was read and no work
                    // item starts. The proposal stays open and the person reads
                    // that the press started nothing.
                    self.orchestrator_dock.session_notice(FRONT_DESK, PROPOSAL_CHECK_FAILED);
                }
            }
            Choice::Decline => self.decline_proposal(id),
        }
        ctx.request_repaint();
    }

    /// Applies the answer of the check of the folders of a proposal: one workspace
    /// for each item, its folder when the check found it, and the task of the item
    /// as the first message of the orchestrator of that workspace.
    ///
    /// The active workspace and the conversation that the dock shows do not change,
    /// so a press on the row of a proposal leaves the person where they were.
    pub(super) fn apply_proposal_start(&mut self, ctx: &egui::Context) {
        let Some((id, found)) = self.orchestrator_dock.take_proposal_check() else {
            return;
        };
        let Some(items) = self.orchestrator_dock.open_proposal(&id) else {
            return;
        };
        let front_desk_shown = self.orchestrator_dock.front_desk().shows(&self.board);
        let mut started: Vec<(String, String)> = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let workspace = self.board.create_workspace(&item.title);
            // A new workspace starts with the grid arrangement, and an arranged
            // workspace gives every tile the size of the preset: a card of the
            // orchestrator would then hide its own text behind `More below`, as a
            // real run showed. The panels of a work item are cards that take the
            // size of their content, so the work item starts with no arrangement,
            // as the workspace of a machine does.
            self.board.clear_workspace_layout(workspace);
            let Some(local_id) = self
                .board
                .workspace(workspace)
                .map(|workspace| workspace.local_id.clone())
            else {
                continue;
            };
            match (&item.folder, found.get(index).copied().unwrap_or(false)) {
                (Some(folder), true) => {
                    self.board
                        .set_workspace_directory(workspace, Some(std::path::Path::new(folder)));
                }
                (Some(_), false) => {
                    // The folder of a proposal is text of a program, and it is a
                    // directory only when the worker found one. The work item starts
                    // in the folder of this Horizon, and the person reads why.
                    self.orchestrator_dock
                        .session_notice(FRONT_DESK, &folder_missing_notice(&item.title));
                }
                (None, _) => {}
            }
            started.push((local_id, item.task.clone()));
        }
        for (local_id, task) in &started {
            if self.say_to_orchestrator(ctx, local_id, task) {
                continue;
            }
            tracing::warn!(%local_id, "the task of a work item of a proposal did not reach its orchestrator");
        }
        if started.is_empty() {
            return;
        }
        if !self.orchestrator_dock.proposal_state(&id, ProposalState::Started) {
            return;
        }
        self.orchestrator_dock
            .session_notice(FRONT_DESK, &proposal::started_notice(started.len()));
        let names: Vec<(String, String)> = started
            .iter()
            .map(|(local_id, task)| {
                let name = self
                    .board
                    .workspace_id_by_local_id(local_id)
                    .and_then(|workspace| self.board.workspace(workspace))
                    .map_or_else(String::new, |workspace| workspace.name.clone());
                let _ = task;
                (local_id.clone(), name)
            })
            .collect();
        for (local_id, name) in names {
            self.orchestrator_dock.work_item_row(&local_id, &name);
        }
        self.orchestrator_dock
            .front_desk_system_message(&proposal::bridge_line(&id, Choice::Start));
        // The conversation that the dock shows stays the front desk, and the active
        // workspace stays the one of the frame before the press.
        self.orchestrator_dock
            .front_desk_mut()
            .follow_active_workspace(&self.board);
        if front_desk_shown {
            self.orchestrator_dock.front_desk_mut().show();
        }
        self.mark_runtime_dirty();
    }

    /// Applies a press on `Not now`: the proposal is not started, the person reads
    /// it, and the program of the front desk reads the same line.
    fn decline_proposal(&mut self, id: &str) {
        if !self.orchestrator_dock.proposal_state(id, ProposalState::Declined) {
            return;
        }
        self.orchestrator_dock
            .session_notice(FRONT_DESK, proposal::NOT_STARTED_NOTICE);
        self.orchestrator_dock
            .front_desk_system_message(&proposal::bridge_line(id, Choice::Decline));
        self.mark_runtime_dirty();
    }
}
