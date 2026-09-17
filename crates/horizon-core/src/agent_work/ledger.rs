use serde::{Deserialize, Serialize};

use super::TurnState;

/// Normalized lifecycle input. Adapters must omit prompt/tool contents.
#[derive(Clone, Debug, Deserialize)]
pub struct HookEvent {
    pub session_id: String,
    #[serde(default, alias = "turn_id")]
    pub prompt_id: Option<String>,
    pub hook_event_name: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
}

/// A matching Stop closes only its own prompt. Missing identifiers never
/// establish an unfinished turn, even if the terminal shows a spinner.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct TurnLedger {
    pub session_id: String,
    pub prompt_id: Option<String>,
    pub state: TurnState,
    pub updated_at_millis: i64,
    pub ended_at_millis: Option<i64>,
    pub deliberate_exit: bool,
    pub generation: u64,
    pub(super) pending_questions: Vec<String>,
    pub(super) turn_closed: bool,
}

impl TurnLedger {
    /// Apply one event from the owning session. Unsupported events and stale
    /// prompt events cannot clear a permission/question blocker.
    pub fn apply(&mut self, event: &HookEvent, now_millis: i64) {
        if event.session_id.is_empty() || (!self.session_id.is_empty() && self.session_id != event.session_id) {
            return;
        }
        if now_millis < self.updated_at_millis {
            self.state = TurnState::Unknown;
            return;
        }
        self.session_id.clone_from(&event.session_id);
        match event.hook_event_name.as_str() {
            "SessionStart" if event.source.as_deref() != Some("compact") => {
                self.generation = self.generation.saturating_add(1);
                self.prompt_id = None;
                self.pending_questions.clear();
                self.state = TurnState::Unknown;
                self.ended_at_millis = None;
                self.deliberate_exit = false;
                self.turn_closed = false;
            }
            "UserPromptSubmit" => {
                self.prompt_id = event.prompt_id.clone().filter(|id| !id.is_empty());
                self.state = if self.prompt_id.is_some() {
                    TurnState::Working
                } else {
                    TurnState::Unknown
                };
                self.pending_questions.clear();
                self.ended_at_millis = None;
                self.deliberate_exit = false;
                self.turn_closed = false;
            }
            "SessionEnd" => {
                self.ended_at_millis = Some(now_millis);
                self.deliberate_exit = event.reason.as_deref() != Some("other");
            }
            _ if self.prompt_id.is_none() || self.prompt_id != event.prompt_id => return,
            "Stop" => self.close_turn(TurnState::Finished),
            "StopFailure" => self.close_turn(TurnState::Blocked),
            _ if self.turn_closed => return,
            "Interrupt" => self.close_turn(if self.pending_questions.is_empty() {
                TurnState::Interrupted
            } else {
                TurnState::Blocked
            }),
            "PermissionRequest" | "Elicitation" => self.block_on(event.tool_use_id.as_deref()),
            "PreToolUse"
                if matches!(
                    event.tool_name.as_deref(),
                    Some("AskUserQuestion" | "request_user_input")
                ) =>
            {
                self.block_on(event.tool_use_id.as_deref());
            }
            "PostToolUse" | "ElicitationResult" => {
                if let Some(id) = event.tool_use_id.as_ref().filter(|id| !id.is_empty()) {
                    let was_blocked = !self.pending_questions.is_empty();
                    self.pending_questions.retain(|pending| pending != id);
                    if was_blocked && self.pending_questions.is_empty() && self.state == TurnState::Blocked {
                        self.state = TurnState::Working;
                    }
                }
            }
            _ => return,
        }
        self.updated_at_millis = now_millis;
    }

    fn close_turn(&mut self, state: TurnState) {
        self.turn_closed = true;
        self.state = state;
    }

    fn block_on(&mut self, id: Option<&str>) {
        // An uncorrelated request remains blocked for the rest of the turn.
        let id = id.unwrap_or("").to_string();
        if !self.pending_questions.contains(&id) {
            self.pending_questions.push(id);
        }
        self.state = TurnState::Blocked;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &str, prompt: Option<&str>) -> HookEvent {
        HookEvent {
            session_id: "session".into(),
            prompt_id: prompt.map(str::to_owned),
            hook_event_name: name.into(),
            reason: None,
            source: None,
            tool_name: None,
            tool_use_id: None,
        }
    }

    #[test]
    fn completed_and_missing_prompt_ids_cannot_prove_work() {
        let mut ledger = TurnLedger::default();
        ledger.apply(&event("UserPromptSubmit", None), 1);
        assert_eq!(ledger.state, TurnState::Unknown);
        ledger.apply(&event("UserPromptSubmit", Some("new")), 2);
        ledger.apply(&event("Stop", Some("old")), 3);
        assert_eq!(ledger.state, TurnState::Working);
        ledger.apply(&event("Stop", Some("new")), 4);
        assert_eq!(ledger.state, TurnState::Finished);
    }

    #[test]
    fn concurrent_permissions_need_matching_results() {
        let mut ledger = TurnLedger::default();
        ledger.apply(&event("UserPromptSubmit", Some("p")), 1);
        for id in ["a", "b"] {
            let mut request = event("PermissionRequest", Some("p"));
            request.tool_use_id = Some(id.into());
            ledger.apply(&request, 2);
        }
        let mut result = event("PostToolUse", Some("p"));
        for (id, expected) in [
            ("unrelated", TurnState::Blocked),
            ("a", TurnState::Blocked),
            ("b", TurnState::Working),
        ] {
            result.tool_use_id = Some(id.into());
            ledger.apply(&result, 3);
            assert_eq!(ledger.state, expected);
        }
    }

    #[test]
    fn uncorrelated_questions_and_failures_remain_blocked() {
        let mut ledger = TurnLedger::default();
        ledger.apply(&event("UserPromptSubmit", Some("p")), 1);
        ledger.apply(&event("Elicitation", Some("p")), 2);
        ledger.apply(&event("ElicitationResult", Some("p")), 3);
        let mut empty_result = event("ElicitationResult", Some("p"));
        empty_result.tool_use_id = Some(String::new());
        ledger.apply(&empty_result, 3);
        assert_eq!(ledger.state, TurnState::Blocked);
        ledger.apply(&event("UserPromptSubmit", Some("q")), 4);
        ledger.apply(&event("StopFailure", Some("q")), 5);
        ledger.apply(&event("PostToolUse", Some("q")), 6);
        assert_eq!(ledger.state, TurnState::Blocked);
    }

    #[test]
    fn late_tool_events_cannot_reopen_a_closed_turn() {
        for (end, expected) in [
            ("Stop", TurnState::Finished),
            ("Interrupt", TurnState::Blocked),
            ("StopFailure", TurnState::Blocked),
        ] {
            let mut ledger = TurnLedger::default();
            ledger.apply(&event("UserPromptSubmit", Some("p")), 1);
            let mut request = event("PermissionRequest", Some("p"));
            request.tool_use_id = Some("tool".into());
            ledger.apply(&request, 2);
            ledger.apply(&event(end, Some("p")), 3);
            ledger.apply(&request, 4);
            request.hook_event_name = "PostToolUse".into();
            ledger.apply(&request, 5);
            assert_eq!(ledger.state, expected);
        }
    }

    #[test]
    fn completion_or_failure_after_interrupt_remains_a_resume_veto() {
        for (end, expected) in [("Stop", TurnState::Finished), ("StopFailure", TurnState::Blocked)] {
            let mut ledger = TurnLedger::default();
            ledger.apply(&event("UserPromptSubmit", Some("p")), 1);
            ledger.apply(&event("Interrupt", Some("p")), 2);
            ledger.apply(&event(end, Some("p")), 3);
            ledger.apply(&event("Interrupt", Some("p")), 4);
            assert_eq!(ledger.state, expected);
        }
    }

    #[test]
    fn interrupt_does_not_erase_a_question_that_raced_with_suspend() {
        let mut ledger = TurnLedger::default();
        ledger.apply(&event("UserPromptSubmit", Some("p")), 1);
        let mut request = event("PermissionRequest", Some("p"));
        request.tool_use_id = Some("question".into());
        ledger.apply(&request, 2);
        ledger.apply(&event("Interrupt", Some("p")), 3);
        request.hook_event_name = "PostToolUse".into();
        ledger.apply(&request, 4);
        assert_eq!(ledger.state, TurnState::Blocked);
    }

    #[test]
    fn deliberate_exit_and_reopening_invalidate_a_handoff() {
        let mut ledger = TurnLedger::default();
        ledger.apply(&event("SessionStart", None), 1);
        ledger.apply(&event("UserPromptSubmit", Some("p")), 2);
        let mut end = event("SessionEnd", Some("p"));
        end.reason = Some("prompt_input_exit".into());
        ledger.apply(&end, 3);
        assert!(ledger.deliberate_exit);
        assert_eq!(ledger.ended_at_millis, Some(3));
        ledger.apply(&event("SessionStart", None), 4);
        assert_eq!(ledger.generation, 2);
        assert_eq!(ledger.state, TurnState::Unknown);
        assert!(ledger.prompt_id.is_none());
    }
}
