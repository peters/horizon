use serde::{Deserialize, Serialize};

use crate::agent_status::AgentStatus;

/// A rule that fires when a source agent reaches a trigger status, injecting a
/// command into a target agent's stdin.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentChain {
    /// Persistent panel identifier of the source agent.
    pub source_panel_local_id: String,
    /// The status that triggers the chain.
    pub trigger_status: AgentStatus,
    /// Persistent panel identifier of the target agent.
    pub target_panel_local_id: String,
    /// The text to inject into the target agent's stdin (a newline is appended
    /// automatically).
    pub command: String,
    /// When `true` the chain fires at most once; otherwise it re-arms whenever
    /// the source transitions away from the trigger status and back.
    #[serde(default)]
    pub one_shot: bool,
    /// Tracks whether the chain has already fired so it does not re-fire on
    /// consecutive frames with the same trigger status.
    #[serde(skip)]
    pub fired: bool,
}

/// Per-workspace orchestration configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OrchestrationState {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chains: Vec<AgentChain>,
}

impl OrchestrationState {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chains.is_empty()
    }

    pub fn add_chain(&mut self, chain: AgentChain) {
        self.chains.push(chain);
    }

    pub fn remove_chain(&mut self, index: usize) {
        if index < self.chains.len() {
            self.chains.remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip() {
        let state = OrchestrationState {
            chains: vec![AgentChain {
                source_panel_local_id: "panel-a".into(),
                trigger_status: AgentStatus::Idle,
                target_panel_local_id: "panel-b".into(),
                command: "review the changes".into(),
                one_shot: true,
                fired: true, // skipped during serialization
            }],
        };

        let yaml = serde_yaml::to_string(&state).expect("serialize");
        let restored: OrchestrationState = serde_yaml::from_str(&yaml).expect("deserialize");

        assert_eq!(restored.chains.len(), 1);
        assert_eq!(restored.chains[0].command, "review the changes");
        assert_eq!(restored.chains[0].trigger_status, AgentStatus::Idle);
        assert!(restored.chains[0].one_shot);
        assert!(!restored.chains[0].fired, "fired must be reset on deserialize");
    }

    #[test]
    fn is_empty() {
        let empty = OrchestrationState::default();
        assert!(empty.is_empty());

        let mut state = OrchestrationState::default();
        state.add_chain(AgentChain {
            source_panel_local_id: "a".into(),
            trigger_status: AgentStatus::Idle,
            target_panel_local_id: "b".into(),
            command: "go".into(),
            one_shot: false,
            fired: false,
        });
        assert!(!state.is_empty());
    }

    #[test]
    fn remove_chain() {
        let mut state = OrchestrationState::default();
        state.add_chain(AgentChain {
            source_panel_local_id: "a".into(),
            trigger_status: AgentStatus::Idle,
            target_panel_local_id: "b".into(),
            command: "go".into(),
            one_shot: false,
            fired: false,
        });
        state.remove_chain(0);
        assert!(state.is_empty());
    }

    #[test]
    fn remove_chain_out_of_bounds_is_noop() {
        let mut state = OrchestrationState::default();
        state.remove_chain(5); // should not panic
        assert!(state.is_empty());
    }
}
