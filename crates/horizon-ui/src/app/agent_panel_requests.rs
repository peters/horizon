//! Host side of the `agent_panels` tool: list the agents of the caller's
//! workspace, type a message into an idle one, and read an agent's output.
//!
//! Only the assistant may send. The message is typed as a bracketed paste and
//! Enter follows shortly after as a separate write, so the agent sees a paste
//! and then a submit rather than a pasted newline.

use std::time::{Duration, Instant};

use egui::{Context, Key, Modifiers};
use horizon_core::browser::manifest::{
    self,
    agent_panels::{self, DEFAULT_READ_LINES, Operation, Outcome, Request},
};
use horizon_core::{PanelId, WorkspaceId};

use super::{HorizonApp, browser_requests::actor_panel};
use crate::input::{KeyEventContext, KeyIdentity, paste_bytes, translate_key_event_with_physical};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Gap between the pasted text and Enter, so a TUI's paste handling has finished.
const SUBMIT_DELAY: Duration = Duration::from_millis(150);

struct PendingSubmit {
    panel_id: PanelId,
    at: Instant,
}

#[derive(Default)]
pub(super) struct AgentPanelRequests {
    last_poll: Option<Instant>,
    pending_submits: Vec<PendingSubmit>,
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
        match &request.operation {
            Operation::List => Outcome::Panels {
                panels: self
                    .board
                    .agent_panels_in_workspace(caller.workspace_id, caller.panel_id),
            },
            Operation::Read { panel_id, lines } => {
                let Some(target) = self.agent_in_workspace(panel_id, caller.workspace_id) else {
                    return unavailable();
                };
                let wanted = usize::from(lines.unwrap_or(DEFAULT_READ_LINES));
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
            Operation::Send { panel_id, text, submit } => {
                if !self
                    .board
                    .panel(caller.panel_id)
                    .is_some_and(horizon_core::Panel::is_assistant)
                {
                    return Outcome::failed(
                        "assistant_only",
                        "Only the assistant can send messages to other agents.",
                    );
                }
                let Some(target) = self.agent_in_workspace(panel_id, caller.workspace_id) else {
                    return unavailable();
                };
                if let Err(refusal) = self.board.check_agent_can_receive(caller.panel_id, target) {
                    return Outcome::failed(refusal.code(), refusal.message());
                }
                if !self.type_into_agent(target, text) {
                    return unavailable();
                }
                if *submit {
                    self.agent_panel_requests.pending_submits.push(PendingSubmit {
                        panel_id: target,
                        at: now + SUBMIT_DELAY,
                    });
                }
                Outcome::Sent {
                    panel_id: panel_id.clone(),
                    submitted: *submit,
                }
            }
        }
    }

    fn agent_in_workspace(&self, local_id: &str, workspace: WorkspaceId) -> Option<PanelId> {
        self.board.panel_id_by_local_id(local_id).filter(|id| {
            self.board
                .panel(*id)
                .is_some_and(|panel| panel.workspace_id == workspace)
        })
    }

    /// Pastes `text` into the agent's prompt. Returns false when the panel has no terminal.
    fn type_into_agent(&mut self, panel_id: PanelId, text: &str) -> bool {
        let Some(panel) = self.board.panel_mut(panel_id) else {
            return false;
        };
        let Some(terminal) = panel.terminal() else {
            return false;
        };
        let bytes = paste_bytes(&printable(text), terminal.mode(), true);
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

fn unavailable() -> Outcome {
    Outcome::failed(
        "panel_unavailable",
        "No agent with that panel_id is in this workspace. List the agents first.",
    )
}

/// Keeps newlines and tabs and drops every other control character, so a message
/// cannot carry terminal escapes or keystrokes into the target's prompt.
fn printable(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .filter(|ch| *ch == '\n' || *ch == '\t' || !ch.is_control())
        .collect()
}

#[cfg(test)]
mod tests;
