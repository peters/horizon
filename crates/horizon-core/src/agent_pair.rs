#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Error, Result};

const AGENT_PAIR_QUEUE_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct AgentPairQueue {
    pub version: u32,
    pub researcher: Option<AgentPanelLink>,
    pub performer: Option<AgentPanelLink>,
    pub cards: Vec<FindingCard>,
}

impl Default for AgentPairQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentPairQueue {
    #[must_use]
    pub fn new() -> Self {
        Self {
            version: AGENT_PAIR_QUEUE_VERSION,
            researcher: None,
            performer: None,
            cards: Vec::new(),
        }
    }

    pub fn normalize(&mut self) {
        if self.version == 0 {
            self.version = AGENT_PAIR_QUEUE_VERSION;
        }
    }

    #[must_use]
    pub fn link_for(&self, role: AgentPairRole) -> Option<&AgentPanelLink> {
        match role {
            AgentPairRole::Researcher => self.researcher.as_ref(),
            AgentPairRole::Performer => self.performer.as_ref(),
        }
    }

    /// Link a stable panel identity to one side of the agent pair.
    ///
    /// # Errors
    ///
    /// Returns an error when `panel_local_id` is blank.
    pub fn link_panel(&mut self, role: AgentPairRole, panel_local_id: impl Into<String>) -> Result<()> {
        let panel_local_id = panel_local_id.into();
        if panel_local_id.trim().is_empty() {
            return Err(Error::State("panel link requires a stable local id".to_string()));
        }

        let link = AgentPanelLink::new(role, panel_local_id);
        match role {
            AgentPairRole::Researcher => self.researcher = Some(link),
            AgentPairRole::Performer => self.performer = Some(link),
        }
        Ok(())
    }

    pub fn unlink_panel(&mut self, role: AgentPairRole) {
        match role {
            AgentPairRole::Researcher => self.researcher = None,
            AgentPairRole::Performer => self.performer = None,
        }
    }

    /// Add a new candidate finding to the queue.
    ///
    /// # Errors
    ///
    /// Returns an error when the title, summary, or evidence is blank.
    pub fn create_candidate(
        &mut self,
        title: impl Into<String>,
        summary: impl Into<String>,
        evidence: impl Into<String>,
        suspected_files: Vec<String>,
        suggested_tests: Vec<String>,
    ) -> Result<String> {
        let title = title.into();
        let summary = summary.into();
        let evidence = evidence.into();
        let title = normalized_required(&title, "finding title")?;
        let summary = normalized_required(&summary, "finding summary")?;
        let evidence = normalized_required(&evidence, "finding evidence")?;
        let now = current_unix_millis();
        let id = Uuid::new_v4().to_string();
        self.cards.push(FindingCard {
            id: id.clone(),
            title,
            summary,
            evidence,
            suspected_files: normalize_lines(suspected_files),
            suggested_tests: normalize_lines(suggested_tests),
            status: FindingStatus::Candidate,
            assigned_performer_panel_local_id: None,
            regression_evidence: None,
            created_at_millis: now,
            updated_at_millis: now,
        });
        Ok(id)
    }

    /// Mark a candidate finding as accepted.
    ///
    /// # Errors
    ///
    /// Returns an error when the finding does not exist or is not a candidate.
    pub fn accept_candidate(&mut self, finding_id: &str) -> Result<()> {
        self.transition_candidate(finding_id, FindingStatus::Accepted)
    }

    /// Mark a candidate finding as rejected.
    ///
    /// # Errors
    ///
    /// Returns an error when the finding does not exist or is not a candidate.
    pub fn reject_candidate(&mut self, finding_id: &str) -> Result<()> {
        self.transition_candidate(finding_id, FindingStatus::Rejected)
    }

    /// Generate the performer handoff and mark an accepted finding as implementing.
    ///
    /// # Errors
    ///
    /// Returns an error when no performer is linked, the finding does not exist,
    /// or the finding is not accepted.
    pub fn dispatch_to_performer(&mut self, finding_id: &str) -> Result<String> {
        let performer = self
            .performer
            .as_ref()
            .ok_or_else(|| Error::State("no performer panel is linked".to_string()))?
            .panel_local_id
            .clone();
        let card = self.card_mut(finding_id)?;
        if card.status != FindingStatus::Accepted {
            return Err(Error::State(format!(
                "only accepted findings can be dispatched; {} is {}",
                card.id,
                card.status.label()
            )));
        }

        let prompt = card.performer_prompt();
        card.status = FindingStatus::Implementing;
        card.assigned_performer_panel_local_id = Some(performer);
        card.updated_at_millis = current_unix_millis();
        Ok(prompt)
    }

    /// Attach complete regression evidence and mark an implementing finding as verified.
    ///
    /// # Errors
    ///
    /// Returns an error when the evidence is incomplete, the finding does not
    /// exist, or the finding is not implementing.
    pub fn verify_with_evidence(&mut self, finding_id: &str, evidence: RegressionEvidencePacket) -> Result<()> {
        if !evidence.is_complete() {
            return Err(Error::State("regression evidence packet is incomplete".to_string()));
        }

        let card = self.card_mut(finding_id)?;
        if card.status != FindingStatus::Implementing {
            return Err(Error::State(format!(
                "only implementing findings can be verified; {} is {}",
                card.id,
                card.status.label()
            )));
        }

        card.regression_evidence = Some(evidence);
        card.status = FindingStatus::Verified;
        card.updated_at_millis = current_unix_millis();
        Ok(())
    }

    #[must_use]
    pub fn card(&self, finding_id: &str) -> Option<&FindingCard> {
        self.cards.iter().find(|card| card.id == finding_id)
    }

    fn transition_candidate(&mut self, finding_id: &str, status: FindingStatus) -> Result<()> {
        let card = self.card_mut(finding_id)?;
        if card.status != FindingStatus::Candidate {
            return Err(Error::State(format!(
                "only candidate findings can change to {}; {} is {}",
                status.label(),
                card.id,
                card.status.label()
            )));
        }
        card.status = status;
        card.updated_at_millis = current_unix_millis();
        Ok(())
    }

    fn card_mut(&mut self, finding_id: &str) -> Result<&mut FindingCard> {
        self.cards
            .iter_mut()
            .find(|card| card.id == finding_id)
            .ok_or_else(|| Error::State(format!("finding {finding_id} was not found")))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPairRole {
    Researcher,
    Performer,
}

impl AgentPairRole {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Researcher => "Researcher",
            Self::Performer => "Performer",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct AgentPanelLink {
    pub role: AgentPairRole,
    pub panel_local_id: String,
}

impl AgentPanelLink {
    #[must_use]
    pub fn new(role: AgentPairRole, panel_local_id: String) -> Self {
        Self { role, panel_local_id }
    }
}

impl Default for AgentPanelLink {
    fn default() -> Self {
        Self {
            role: AgentPairRole::Researcher,
            panel_local_id: String::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct FindingCard {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub evidence: String,
    pub suspected_files: Vec<String>,
    pub suggested_tests: Vec<String>,
    pub status: FindingStatus,
    pub assigned_performer_panel_local_id: Option<String>,
    pub regression_evidence: Option<RegressionEvidencePacket>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
}

impl FindingCard {
    #[must_use]
    pub fn performer_prompt(&self) -> String {
        format!(
            "Implement accepted finding {} from the Horizon Review Queue.\n\nTitle: {}\nSummary: {}\nEvidence: {}\nSuspected files: {}\nSuggested tests: {}\n\nVerify the finding first. Edit only after confirming it is credible. Run the required validation and report the regression evidence packet before marking the card verified.",
            self.id,
            self.title,
            self.summary,
            self.evidence,
            format_list(&self.suspected_files),
            format_list(&self.suggested_tests),
        )
    }

    #[must_use]
    pub fn assignment_label(&self, performer_title: Option<&str>) -> String {
        match self.status {
            FindingStatus::Accepted => "Ready for Performer".to_string(),
            FindingStatus::Implementing => performer_title.map_or_else(
                || "Implementing by linked performer".to_string(),
                |title| format!("Implementing by {title}"),
            ),
            FindingStatus::Verified => "Verified".to_string(),
            FindingStatus::Rejected => "Rejected".to_string(),
            FindingStatus::Candidate => "Awaiting user review".to_string(),
        }
    }
}

impl Default for FindingCard {
    fn default() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            summary: String::new(),
            evidence: String::new(),
            suspected_files: Vec::new(),
            suggested_tests: Vec::new(),
            status: FindingStatus::Candidate,
            assigned_performer_panel_local_id: None,
            regression_evidence: None,
            created_at_millis: 0,
            updated_at_millis: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingStatus {
    #[default]
    Candidate,
    Accepted,
    Rejected,
    Implementing,
    Verified,
}

impl FindingStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Implementing => "implementing",
            Self::Verified => "verified",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct RegressionEvidencePacket {
    pub verification_summary: String,
    pub validation_commands: Vec<String>,
    pub validation_result: String,
    pub regression_scope: String,
}

impl RegressionEvidencePacket {
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !self.verification_summary.trim().is_empty()
            && self
                .validation_commands
                .iter()
                .any(|command| !command.trim().is_empty())
            && !self.validation_result.trim().is_empty()
            && !self.regression_scope.trim().is_empty()
    }
}

fn normalized_required(value: &str, field: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(Error::State(format!("{field} cannot be empty")));
    }
    Ok(trimmed.to_string())
}

fn normalize_lines(lines: Vec<String>) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

fn format_list(values: &[String]) -> String {
    if values.is_empty() {
        "None listed".to_string()
    } else {
        values.join(", ")
    }
}

fn current_unix_millis() -> i64 {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}
