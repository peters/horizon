use std::collections::HashMap;

use horizon_core::{AgentPairQueue, FindingCard, FindingStatus, PanelId, PanelKind, RegressionEvidencePacket};

#[derive(Clone, Debug)]
pub(super) struct LinkablePanel {
    pub panel_id: PanelId,
    pub local_id: String,
    pub title: String,
    pub kind: PanelKind,
    pub workspace_name: String,
    pub terminal_backed: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct CandidateDraft {
    pub title: String,
    pub summary: String,
    pub evidence: String,
    pub suspected_files: String,
    pub suggested_tests: String,
}

impl CandidateDraft {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.title.trim().is_empty() && !self.summary.trim().is_empty() && !self.evidence.trim().is_empty()
    }

    #[must_use]
    pub fn suspected_file_lines(&self) -> Vec<String> {
        split_lines(&self.suspected_files)
    }

    #[must_use]
    pub fn suggested_test_lines(&self) -> Vec<String> {
        split_lines(&self.suggested_tests)
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct EvidenceDraft {
    pub verification_summary: String,
    pub validation_commands: String,
    pub validation_result: String,
    pub regression_scope: String,
}

impl EvidenceDraft {
    #[must_use]
    pub fn packet(&self) -> RegressionEvidencePacket {
        RegressionEvidencePacket {
            verification_summary: self.verification_summary.trim().to_string(),
            validation_commands: split_lines(&self.validation_commands),
            validation_result: self.validation_result.trim().to_string(),
            regression_scope: self.regression_scope.trim().to_string(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(in crate::app) struct AgentPairReviewQueueUiState {
    pub(super) candidate: CandidateDraft,
    pub(super) evidence_by_card: HashMap<String, EvidenceDraft>,
    pub(super) error: Option<String>,
}

impl AgentPairReviewQueueUiState {
    pub(super) fn reset_for_queue(&mut self, queue: &AgentPairQueue) {
        self.evidence_by_card
            .retain(|finding_id, _| queue.cards.iter().any(|card| &card.id == finding_id));
    }

    pub(super) fn evidence_draft_mut(&mut self, card: &FindingCard) -> &mut EvidenceDraft {
        self.evidence_by_card.entry(card.id.clone()).or_insert_with(|| {
            card.regression_evidence
                .as_ref()
                .map_or_else(EvidenceDraft::default, |packet| EvidenceDraft {
                    verification_summary: packet.verification_summary.clone(),
                    validation_commands: packet.validation_commands.join("\n"),
                    validation_result: packet.validation_result.clone(),
                    regression_scope: packet.regression_scope.clone(),
                })
        })
    }
}

#[must_use]
pub(super) fn dispatch_enabled(queue: &AgentPairQueue, card: &FindingCard) -> bool {
    card.status == FindingStatus::Accepted && queue.performer.is_some()
}

#[must_use]
pub(super) fn role_heading(role: horizon_core::AgentPairRole) -> &'static str {
    role.label()
}

#[must_use]
pub(super) fn shorten_middle(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }

    let keep = max_chars - 3;
    let left = keep / 2 + keep % 2;
    let right = keep / 2;
    let prefix = value.chars().take(left).collect::<String>();
    let suffix = value
        .chars()
        .rev()
        .take(right)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{prefix}...{suffix}")
}

fn split_lines(value: &str) -> Vec<String> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use horizon_core::{AgentPairQueue, AgentPairRole, FindingStatus};

    use super::{dispatch_enabled, role_heading, shorten_middle};

    #[test]
    fn role_headings_match_queue_roles() {
        assert_eq!(role_heading(AgentPairRole::Researcher), "Researcher");
        assert_eq!(role_heading(AgentPairRole::Performer), "Performer");
    }

    #[test]
    fn disconnected_performer_disables_dispatch() {
        let mut queue = AgentPairQueue::new();
        let id = queue
            .create_candidate("Title", "Summary", "Evidence", Vec::new(), Vec::new())
            .expect("candidate");
        queue.accept_candidate(&id).expect("accept");
        let card = queue.card(&id).expect("card");

        assert_eq!(card.status, FindingStatus::Accepted);
        assert!(!dispatch_enabled(&queue, card));
    }

    #[test]
    fn connected_performer_enables_accepted_dispatch() {
        let mut queue = AgentPairQueue::new();
        queue
            .link_panel(AgentPairRole::Performer, "performer-local-id")
            .expect("link");
        let id = queue
            .create_candidate("Title", "Summary", "Evidence", Vec::new(), Vec::new())
            .expect("candidate");
        queue.accept_candidate(&id).expect("accept");

        assert!(dispatch_enabled(&queue, queue.card(&id).expect("card")));
    }

    #[test]
    fn shorten_middle_preserves_edges_for_long_titles_and_paths() {
        let value = "crates/horizon-ui/src/app/agent_pair/really_long_review_queue_path.rs";
        let shortened = shorten_middle(value, 32);

        assert!(shortened.starts_with("crates/horizon-"));
        assert!(shortened.ends_with("ue_path.rs"));
        assert!(shortened.len() <= 32);
    }
}
