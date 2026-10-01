//! Everything that waits for the person, in one place and in order.
//!
//! With one agent a question is a card. With twenty it is a flood. The inbox turns the questions of
//! every agent in reach into one ordered list, each with a risk label, so a design can show the next
//! one, count the rest, and offer to deal with the harmless ones in a single press.

use egui::Color32;
use horizon_core::PanelId;
use horizon_core::browser::manifest::agent_panels::AgentState;

use super::super::HorizonApp;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Risk {
    Low,
    Medium,
    High,
}

impl Risk {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Low => "Low risk",
            Self::Medium => "Check",
            Self::High => "Careful",
        }
    }

    pub(super) fn color(self) -> Color32 {
        match self {
            Self::Low => theme::PALETTE_GREEN(),
            Self::Medium => theme::PALETTE_YELLOW(),
            Self::High => theme::PALETTE_RED(),
        }
    }

    /// A guess from the words of the question, for ordering and for the label. It is never a
    /// reason to answer on the person's behalf: only the person's press does that.
    pub(super) fn of(question: &str) -> Self {
        let text = question.to_lowercase();
        let has = |words: &[&str]| words.iter().any(|word| text.contains(word));
        if has(&[
            "delete",
            "drop ",
            "production",
            "force",
            "destroy",
            "terminate",
            "rm -rf",
            "wipe",
        ]) {
            Self::High
        } else if has(&["stop", "migrat", "deploy", "publish", "push", "restart", "install"]) {
            Self::Medium
        } else {
            Self::Low
        }
    }
}

/// One question from one agent.
#[derive(Clone)]
pub(in crate::app::assistant::summon) struct Ask {
    pub(in crate::app::assistant::summon) id: PanelId,
    pub(super) agent: String,
    pub(super) workspace: String,
    /// The question, without the "(y/n)".
    pub(super) text: String,
    pub(super) risk: Risk,
}

impl HorizonApp {
    /// The questions waiting, the most careful first so none is lost under the harmless ones.
    pub(in crate::app::assistant::summon) fn inbox(&self) -> Vec<Ask> {
        let mut asks: Vec<Ask> = self
            .feed_agents()
            .into_iter()
            .filter(|agent| agent.state == AgentState::NeedsInput)
            .map(|agent| {
                let text = agent.last.replace("(y/n)", "").replace("(Y/n)", "").trim().to_string();
                Ask {
                    id: agent.id,
                    risk: Risk::of(&text),
                    agent: agent.title,
                    workspace: agent.workspace_name,
                    text,
                }
            })
            .collect();
        asks.sort_by(|a, b| b.risk.cmp(&a.risk).then_with(|| a.agent.cmp(&b.agent)));
        asks
    }

    /// Allows every question that reads as low risk, as the person's own press of one button.
    pub(in crate::app::assistant::summon) fn allow_low_risk(&mut self) {
        for ask in self.inbox().into_iter().filter(|ask| ask.risk == Risk::Low) {
            self.answer_agent(ask.id, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Risk;

    #[test]
    fn risk_follows_the_words_of_the_question() {
        assert_eq!(Risk::of("Apply the fix to src/clone.rs?"), Risk::Low);
        assert_eq!(Risk::of("Allow stopping hetzner-cx42?"), Risk::Medium);
        assert_eq!(Risk::of("Run migration 0042 on the staging database?"), Risk::Medium);
        assert_eq!(Risk::of("Deploy build 8f3c21 to production?"), Risk::High);
        assert!(Risk::High > Risk::Medium && Risk::Medium > Risk::Low);
    }
}
