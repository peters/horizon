//! Host side of the `agent_panels` tool: list the agents of the caller's
//! workspace, type a message into an idle one, and read an agent's output.
//!
//! Only the assistant may send. The message is typed as a bracketed paste and
//! Enter follows shortly after as a separate write, so the agent sees a paste
//! and then a submit rather than a pasted newline.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::{Context, Key, Modifiers};
use horizon_core::browser::manifest::{
    self,
    agent_panels::{self, DEFAULT_READ_LINES, MAX_PENDING_APPROVALS, Operation, Outcome, Request},
};
use horizon_core::{PanelId, Reach};

use super::{HorizonApp, browser_requests::actor_panel};
use crate::input::{KeyEventContext, KeyIdentity, paste_bytes, translate_key_event_with_physical};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Gap between the pasted text and Enter, so a TUI's paste handling has finished.
const SUBMIT_DELAY: Duration = Duration::from_millis(150);
/// After a send, the agent counts as busy for this long even before its screen shows work.
const RECENT_SEND: Duration = Duration::from_secs(3);
const APPROVAL_MESSAGE: &str =
    "The person has to approve this message in the assistant drawer. Nothing was typed; do not send it again.";

struct PendingSubmit {
    panel_id: PanelId,
    at: Instant,
}

#[derive(Default)]
pub(in crate::app) struct AgentPanelRequests {
    last_poll: Option<Instant>,
    pending_submits: Vec<PendingSubmit>,
    /// When each agent last had a message typed into it.
    recent_sends: HashMap<PanelId, Instant>,
}

impl AgentPanelRequests {
    fn note_sent(&mut self, target: PanelId, now: Instant, submit: bool) {
        self.recent_sends
            .retain(|_, at| now.duration_since(*at) < RECENT_SEND * 4);
        self.recent_sends.insert(target, now);
        if submit {
            self.pending_submits.push(PendingSubmit {
                panel_id: target,
                at: now + SUBMIT_DELAY,
            });
        }
    }

    /// Whether a message to `target` is still being delivered or has only just
    /// landed, so a second one must wait instead of merging into the same prompt.
    pub(super) fn in_flight(&self, target: PanelId, now: Instant) -> bool {
        self.pending_submits.iter().any(|entry| entry.panel_id == target)
            || self
                .recent_sends
                .get(&target)
                .is_some_and(|at| now.duration_since(*at) < RECENT_SEND)
    }
}

impl HorizonApp {
    /// Answers queued `agent_panels` calls and presses Enter for sends that are due.
    pub(super) fn poll_agent_panel_requests(&mut self, ctx: &Context) {
        let now = Instant::now();
        self.flush_agent_submits(ctx, now);
        if self
            .agent_panel_requests
            .last_poll
            .is_some_and(|last| now.duration_since(last) < POLL_INTERVAL)
        {
            return;
        }
        self.agent_panel_requests.last_poll = Some(now);
        let requests = match agent_panels::claim(manifest::host_instance()) {
            Ok(requests) => requests,
            Err(error) => {
                tracing::warn!(%error, "could not poll agent panel requests");
                return;
            }
        };
        for request in requests {
            let outcome = self.apply_agent_panel_request(&request, now);
            if let Err(error) = agent_panels::complete(&request, outcome) {
                tracing::warn!(%error, "could not publish agent panel result");
            }
        }
        if !self.agent_panel_requests.pending_submits.is_empty() {
            ctx.request_repaint_after(SUBMIT_DELAY);
        }
    }

    fn apply_agent_panel_request(&mut self, request: &Request, now: Instant) -> Outcome {
        if request.host_instance != manifest::host_instance() {
            return Outcome::failed("wrong_host", "Request belongs to another Horizon host");
        }
        if request.deadline_at_millis <= manifest::now_millis() {
            return Outcome::failed("request_expired", "Agent panel request expired before dispatch");
        }
        let Some(caller) = actor_panel(&self.board, &request.actor) else {
            return Outcome::failed(
                "caller_unavailable",
                "The calling agent panel is no longer in this host",
            );
        };
        // Naming the assistant's panel is not enough: it must also hold this
        // launch's secret, which only the assistant's own environment carries.
        let is_assistant = self
            .board
            .panel(caller.panel_id)
            .is_some_and(horizon_core::Panel::is_assistant)
            && horizon_core::assistant::token_matches(request.credential.as_deref());
        // The assistant is not tied to a workspace; any other agent sees only its own.
        let reach = if is_assistant {
            self.assistant_reach()
        } else {
            Reach::workspace(caller.workspace_id)
        };
        match &request.operation {
            Operation::List => {
                let panels = self.board.agent_panels_in(&reach, caller.panel_id);
                if is_assistant {
                    self.assistant_did(format!("Looked at {} agents", panels.len().saturating_sub(1)));
                }
                Outcome::Panels { panels }
            }
            Operation::Read { panel_id, lines } => {
                let Some(target) = self.board.agent_in_reach(panel_id, &reach) else {
                    return unavailable();
                };
                let wanted = usize::from(lines.unwrap_or(DEFAULT_READ_LINES));
                if is_assistant {
                    let title = self
                        .board
                        .panel(target)
                        .map_or_else(String::new, |panel| panel.display_title().into_owned());
                    self.assistant_did(format!("Read {title}"));
                }
                match (self.board.agent_output(target, wanted), self.board.agent_state(target)) {
                    (Some((text, truncated)), Some(state)) => Outcome::Output {
                        panel_id: panel_id.clone(),
                        state,
                        text,
                        truncated,
                    },
                    _ => unavailable(),
                }
            }
            Operation::Approvals => {
                if !is_assistant {
                    return only_the_assistant("read approvals");
                }
                Outcome::Approvals {
                    items: self.assistant_approvals(),
                }
            }
            Operation::Plan { steps } => {
                if !is_assistant {
                    return only_the_assistant("post a plan");
                }
                let count = steps.len();
                self.assistant_did(format!("Planned {count} steps"));
                self.assistant_set_plan(steps.clone());
                Outcome::Planned { steps: count }
            }
            Operation::Note { title, markdown } => {
                if !is_assistant {
                    return only_the_assistant("post notes");
                }
                self.assistant_post_note(title.trim().to_string(), markdown.clone());
                Outcome::Noted
            }
            Operation::Send { panel_id, text, submit } => {
                if !is_assistant {
                    return only_the_assistant("send messages to other agents");
                }
                self.send_for_assistant(caller.panel_id, &reach, panel_id, text, *submit, now)
            }
        }
    }

    /// Types a message into another agent for the assistant, or asks the person first.
    fn send_for_assistant(
        &mut self,
        caller: PanelId,
        reach: &Reach,
        panel_id: &str,
        text: &str,
        submit: bool,
        now: Instant,
    ) -> Outcome {
        let Some(target) = self.board.agent_in_reach(panel_id, reach) else {
            return unavailable();
        };
        if self.agent_panel_requests.in_flight(target, now) {
            return Outcome::failed(
                "agent_busy",
                "A message was just sent to that agent. Wait until list shows it idle again.",
            );
        }
        if let Err(refusal) = self.board.check_agent_can_receive(caller, target) {
            return Outcome::failed(refusal.code(), refusal.message());
        }
        // The person approves, and the agent receives, exactly this text.
        let clean = agent_panels::printable(text);
        if self.assistant_asks_before_send() {
            if self.assistant_pending_approvals() >= MAX_PENDING_APPROVALS {
                return Outcome::failed(
                    "too_many_pending",
                    "Several messages are already waiting for the person. Wait for them to answer.",
                );
            }
            if !self.assistant_request_approval(target, clean, submit) {
                return unavailable();
            }
            return Outcome::AwaitingApproval {
                panel_id: panel_id.to_string(),
                message: APPROVAL_MESSAGE.to_string(),
            };
        }
        if !self.send_to_agent(target, &clean, submit, now) {
            return unavailable();
        }
        self.assistant_record_sent(target, clean);
        Outcome::Sent {
            panel_id: panel_id.to_string(),
            submitted: submit,
        }
    }

    /// Types `text` into the agent and, if asked, presses Enter shortly after.
    pub(super) fn send_to_agent(&mut self, target: PanelId, text: &str, submit: bool, now: Instant) -> bool {
        if !self.type_into_agent(target, text) {
            return false;
        }
        self.agent_panel_requests.note_sent(target, now, submit);
        true
    }

    /// Pastes `text` into the agent's prompt. Returns false when the panel has no terminal.
    fn type_into_agent(&mut self, panel_id: PanelId, text: &str) -> bool {
        let Some(panel) = self.board.panel_mut(panel_id) else {
            return false;
        };
        let Some(terminal) = panel.terminal() else {
            return false;
        };
        let bytes = paste_bytes(&agent_panels::printable(text), terminal.mode(), true);
        panel.write_input(&bytes);
        true
    }

    fn flush_agent_submits(&mut self, ctx: &Context, now: Instant) {
        let pending = &mut self.agent_panel_requests.pending_submits;
        if pending.is_empty() {
            return;
        }
        let (due, waiting): (Vec<_>, Vec<_>) = std::mem::take(pending).into_iter().partition(|entry| entry.at <= now);
        *pending = waiting;
        for entry in due {
            let Some(panel) = self.board.panel_mut(entry.panel_id) else {
                continue;
            };
            let Some(terminal) = panel.terminal() else {
                continue;
            };
            let context = KeyEventContext::new(true, false, Modifiers::NONE, terminal.mode());
            if let Some(enter) = translate_key_event_with_physical(KeyIdentity::new(Key::Enter, None, None), context) {
                panel.write_input(&enter.bytes);
            }
        }
        if !self.agent_panel_requests.pending_submits.is_empty() {
            ctx.request_repaint_after(SUBMIT_DELAY);
        }
    }
}

fn only_the_assistant(action: &str) -> Outcome {
    Outcome::failed("assistant_only", &format!("Only the assistant can {action}."))
}

fn unavailable() -> Outcome {
    Outcome::failed(
        "panel_unavailable",
        "No agent with that panel_id is in this workspace. List the agents first.",
    )
}

#[cfg(test)]
mod tests;
