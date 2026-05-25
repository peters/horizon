mod render;
mod state;

use egui::Context;
use horizon_core::{AgentPairQueue, AgentPairRole, FindingStatus};

use super::HorizonApp;
use crate::input;

pub(super) use state::AgentPairReviewQueueUiState;
use state::LinkablePanel;

pub(super) const AGENT_PAIR_REVIEW_QUEUE_PANEL_ID: &str = "agent_pair_review_queue";
pub(super) const AGENT_PAIR_REVIEW_QUEUE_DEFAULT_WIDTH: f32 = 430.0;
pub(super) const AGENT_PAIR_REVIEW_QUEUE_MIN_WIDTH: f32 = 340.0;
pub(super) const AGENT_PAIR_REVIEW_QUEUE_MAX_WIDTH: f32 = 560.0;

impl HorizonApp {
    pub(in crate::app) fn toggle_agent_pair_review_queue(&mut self) {
        self.agent_pair_review_queue_open = !self.agent_pair_review_queue_open;
    }

    pub(in crate::app) fn open_agent_pair_review_queue(&mut self) {
        self.agent_pair_review_queue_open = true;
    }

    pub(super) fn load_agent_pair_queue_for_active_session(&mut self) {
        let Some(active_session) = self.active_session.as_ref().filter(|session| session.persistent) else {
            self.agent_pair_queue = AgentPairQueue::new();
            self.agent_pair_ui.error = None;
            self.agent_pair_ui.reset_for_queue(&self.agent_pair_queue);
            return;
        };

        match self.session_store.load_agent_pair_queue(&active_session.session_id) {
            Ok(queue) => {
                self.agent_pair_queue = queue;
                self.agent_pair_ui.error = None;
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %active_session.session_id,
                    %error,
                    "failed to load agent pair review queue"
                );
                self.agent_pair_queue = AgentPairQueue::new();
                self.agent_pair_ui.error = Some(format!("Failed to load Review Queue: {error}"));
            }
        }
        self.agent_pair_ui.reset_for_queue(&self.agent_pair_queue);
    }

    fn save_agent_pair_queue(&mut self) {
        let Some(active_session) = self.active_session.as_ref().filter(|session| session.persistent) else {
            return;
        };

        if let Err(error) = self
            .session_store
            .save_agent_pair_queue(&active_session.session_id, &self.agent_pair_queue)
        {
            tracing::warn!(
                session_id = %active_session.session_id,
                %error,
                "failed to save agent pair review queue"
            );
            self.agent_pair_ui.error = Some(format!("Failed to save Review Queue: {error}"));
        }
    }

    fn link_agent_panel(&mut self, role: AgentPairRole, panel_local_id: Option<String>) {
        let result = if let Some(panel_local_id) = panel_local_id {
            self.agent_pair_queue.link_panel(role, panel_local_id)
        } else {
            self.agent_pair_queue.unlink_panel(role);
            Ok(())
        };

        match result {
            Ok(()) => {
                self.agent_pair_ui.error = None;
                self.save_agent_pair_queue();
            }
            Err(error) => self.agent_pair_ui.error = Some(error.to_string()),
        }
    }

    fn create_agent_pair_candidate(&mut self) {
        let result = self.agent_pair_queue.create_candidate(
            self.agent_pair_ui.candidate.title.clone(),
            self.agent_pair_ui.candidate.summary.clone(),
            self.agent_pair_ui.candidate.evidence.clone(),
            self.agent_pair_ui.candidate.suspected_file_lines(),
            self.agent_pair_ui.candidate.suggested_test_lines(),
        );

        match result {
            Ok(_) => {
                self.agent_pair_ui.candidate.clear();
                self.agent_pair_ui.error = None;
                self.save_agent_pair_queue();
            }
            Err(error) => self.agent_pair_ui.error = Some(error.to_string()),
        }
    }

    fn accept_agent_pair_card(&mut self, finding_id: &str) {
        match self.agent_pair_queue.accept_candidate(finding_id) {
            Ok(()) => {
                self.agent_pair_ui.error = None;
                self.save_agent_pair_queue();
            }
            Err(error) => self.agent_pair_ui.error = Some(error.to_string()),
        }
    }

    fn reject_agent_pair_card(&mut self, finding_id: &str) {
        match self.agent_pair_queue.reject_candidate(finding_id) {
            Ok(()) => {
                self.agent_pair_ui.error = None;
                self.save_agent_pair_queue();
            }
            Err(error) => self.agent_pair_ui.error = Some(error.to_string()),
        }
    }

    fn dispatch_agent_pair_card(&mut self, finding_id: &str) {
        let Some(performer_local_id) = self
            .agent_pair_queue
            .link_for(AgentPairRole::Performer)
            .map(|link| link.panel_local_id.clone())
        else {
            self.agent_pair_ui.error = Some("Link a performer panel before dispatch.".to_string());
            return;
        };
        let Some(panel_id) = self.board.panel_id_by_local_id(&performer_local_id) else {
            self.agent_pair_ui.error = Some("The linked performer panel is not open.".to_string());
            return;
        };
        let Some(mode) = self
            .board
            .panel(panel_id)
            .and_then(|panel| panel.terminal().map(horizon_core::Terminal::mode))
        else {
            self.agent_pair_ui.error = Some("The linked performer panel cannot receive terminal input.".to_string());
            return;
        };

        match self.agent_pair_queue.dispatch_to_performer(finding_id) {
            Ok(prompt) => {
                if let Some(panel) = self.board.panel_mut(panel_id) {
                    let mut bytes = input::paste_bytes(&prompt, mode, true);
                    bytes.push(b'\r');
                    panel.write_input(&bytes);
                }
                self.agent_pair_ui.error = None;
                self.save_agent_pair_queue();
            }
            Err(error) => self.agent_pair_ui.error = Some(error.to_string()),
        }
    }

    fn verify_agent_pair_card(&mut self, finding_id: &str) {
        let Some(card) = self.agent_pair_queue.card(finding_id).cloned() else {
            self.agent_pair_ui.error = Some(format!("Finding {finding_id} was not found."));
            return;
        };
        let packet = self.agent_pair_ui.evidence_draft_mut(&card).packet();

        match self.agent_pair_queue.verify_with_evidence(finding_id, packet) {
            Ok(()) => {
                self.agent_pair_ui.error = None;
                self.save_agent_pair_queue();
            }
            Err(error) => self.agent_pair_ui.error = Some(error.to_string()),
        }
    }

    fn focus_linked_agent_panel(&mut self, ctx: &Context, panel_local_id: &str) {
        if let Some(panel_id) = self.board.panel_id_by_local_id(panel_local_id) {
            self.focus_panel_visible(ctx, panel_id, true);
        }
    }

    fn linkable_agent_panels(&self) -> Vec<LinkablePanel> {
        self.board
            .panels
            .iter()
            .filter(|panel| panel.terminal().is_some())
            .map(|panel| {
                let workspace_name = self
                    .board
                    .workspace(panel.workspace_id)
                    .map_or_else(|| "Unknown workspace".to_string(), |workspace| workspace.name.clone());
                LinkablePanel {
                    panel_id: panel.id,
                    local_id: panel.local_id.clone(),
                    title: panel.display_title().into_owned(),
                    kind: panel.kind,
                    workspace_name,
                    terminal_backed: panel.terminal().is_some(),
                }
            })
            .collect()
    }

    fn performer_title_for_card(&self, finding_id: &str) -> Option<String> {
        let local_id = self
            .agent_pair_queue
            .card(finding_id)?
            .assigned_performer_panel_local_id
            .as_deref()?;
        self.board
            .panel_id_by_local_id(local_id)
            .and_then(|panel_id| self.board.panel(panel_id))
            .map(|panel| panel.display_title().into_owned())
    }
}

fn card_status_order(status: FindingStatus) -> usize {
    match status {
        FindingStatus::Candidate => 0,
        FindingStatus::Accepted => 1,
        FindingStatus::Implementing => 2,
        FindingStatus::Verified => 3,
        FindingStatus::Rejected => 4,
    }
}
