//! Host coordination, auditable controls, and user/agent steering for
//! WebDriver-backed Firefox and Safari sessions.

use std::time::{Duration, Instant};

use crate::session::{BrowserCommand, BrowserEvent, BrowserEventSender};
use crate::{
    AgentAction, AgentActionResult, BrowserAuditAction, BrowserAuditActor, BrowserAuditEntry, BrowserAuditStatus,
    BrowserControlFailure, BrowserControlValue, CoordinationState, HandoffRequest, new_action_id,
};

use super::Driver;

const WRITE_INTERVAL: Duration = Duration::from_millis(200);
const SIGNAL_INTERVAL: Duration = Duration::from_millis(250);
const USER_ACTIVE_STAMP_INTERVAL: Duration = Duration::from_secs(1);
const USER_ACTIVE_TTL: Duration = Duration::from_secs(5);

impl Driver {
    pub(super) fn initialize_coordination(&mut self) {
        let Some(coordination) = self.config.coordination.as_ref() else {
            self.coordination_dirty = false;
            return;
        };
        match coordination.initialize(&self.config.panel_local_id, &self.coordination_state()) {
            Ok(()) => {
                self.coordination_dirty = false;
                self.last_coordination_write = Instant::now();
            }
            Err(error) => {
                self.coordination_dirty = true;
                tracing::warn!(target: "browser", "failed to initialize WebDriver coordination: {error}");
            }
        }
    }

    pub(super) fn write_coordination(&mut self, force: bool) {
        let Some(coordination) = self.config.coordination.as_ref() else {
            self.coordination_dirty = false;
            return;
        };
        if !force && (!self.coordination_dirty || self.last_coordination_write.elapsed() < WRITE_INTERVAL) {
            return;
        }
        match coordination.update(&self.config.panel_local_id, &self.coordination_state()) {
            Ok(()) => {
                self.coordination_dirty = false;
                self.last_coordination_write = Instant::now();
            }
            Err(error) => {
                self.coordination_dirty = true;
                tracing::warn!(target: "browser", "failed to update WebDriver coordination: {error}");
            }
        }
    }

    pub(super) fn tick_coordination(&mut self, event_tx: &BrowserEventSender) -> Vec<AgentAction> {
        if self.last_signal_check.elapsed() < SIGNAL_INTERVAL {
            return Vec::new();
        }
        self.last_signal_check = Instant::now();
        self.expire_user_active();
        let Some(coordination) = self.config.coordination.as_ref() else {
            return Vec::new();
        };
        let signals = match coordination.signals(&self.config.panel_local_id) {
            Ok(signals) => signals,
            Err(error) => {
                tracing::debug!(target: "browser", "failed to read WebDriver coordination: {error}");
                return Vec::new();
            }
        };
        publish_owner_change(&mut self.owner_seen, signals.owner, event_tx);
        self.challenge_loop.observe_handoff_change(
            self.handoff_seen.as_deref(),
            signals.handoff.as_ref().map(|handoff| handoff.request_id.as_str()),
        );
        publish_handoff_change(&mut self.handoff_seen, signals.handoff, event_tx);
        signals.actions
    }

    pub(super) fn resolve_handoff(&mut self, event_tx: &BrowserEventSender) {
        let Some(coordination) = self.config.coordination.as_ref() else {
            let _ = event_tx.send(BrowserEvent::HandoffResolutionFailed(
                "agent handoff coordination is unavailable".to_string(),
            ));
            return;
        };
        let Some(request_id) = self.handoff_seen.as_deref() else {
            let _ = event_tx.send(BrowserEvent::HandoffResolutionFailed(
                "pending handoff no longer exists".to_string(),
            ));
            return;
        };
        match coordination.acknowledge_handoff(&self.config.panel_local_id, request_id) {
            Ok(true) => {
                self.handoff_seen = None;
                self.challenge_loop.handoff_completed();
                let _ = event_tx.send(BrowserEvent::HandoffCleared);
            }
            Ok(false) => {
                let _ = event_tx.send(BrowserEvent::HandoffResolutionFailed(
                    "pending handoff no longer exists".to_string(),
                ));
            }
            Err(error) => {
                tracing::warn!(target: "browser", "failed to resolve WebDriver handoff: {error}");
                let _ = event_tx.send(BrowserEvent::HandoffResolutionFailed(error.to_string()));
            }
        }
    }

    pub(super) fn stamp_user_active(&mut self) {
        if self
            .last_user_active_stamp
            .is_some_and(|last_stamp| last_stamp.elapsed() < USER_ACTIVE_STAMP_INTERVAL)
        {
            return;
        }
        let Some(coordination) = self.config.coordination.as_ref() else {
            return;
        };
        let stamped_at = Instant::now();
        match coordination.set_user_active(&self.config.panel_local_id, true) {
            Ok(()) => self.last_user_active_stamp = Some(stamped_at),
            Err(error) => tracing::warn!(target: "browser", "failed to stamp WebDriver user activity: {error}"),
        }
    }

    pub(super) fn audit_user_command(&mut self, command: &BrowserCommand) {
        // Stop is recorded synchronously by `BrowserSession::send` because
        // setting its atomic flag can end the loop before the queue drains.
        if matches!(command, BrowserCommand::Stop) || !self.audit_sampler.should_record(command) {
            return;
        }
        let actor = if matches!(command, BrowserCommand::SetViewport { .. }) {
            BrowserAuditActor::System
        } else {
            BrowserAuditActor::User
        };
        self.record_audit(
            new_action_id(),
            actor,
            BrowserAuditStatus::Dispatched,
            BrowserAuditAction::from_command(command),
        );
    }

    /// Validate, audit, and run one queued agent action. Navigation settles
    /// from `BiDi` events, so only a synchronously finished navigation
    /// completes here.
    pub(super) fn service_agent_request(&mut self, request: &AgentAction, event_tx: &BrowserEventSender) {
        if let Err(message) = request.action.validate() {
            self.audit_agent_action(request, crate::BrowserAuditStatus::Rejected);
            self.complete_agent_action(
                request,
                Err(crate::BrowserControlFailure::new("invalid_input", message)),
            );
            return;
        }
        self.audit_agent_action(request, crate::BrowserAuditStatus::Dispatched);
        if matches!(request.action, crate::BrowserControlAction::Navigate { .. }) {
            if let crate::navigation::AgentActionExecution::Done(result) = self.navigate_action(request, event_tx) {
                self.complete_agent_action(request, result);
            }
            return;
        }
        let result = self.execute_agent_action(request, event_tx);
        self.complete_agent_action(request, result);
    }

    pub(super) fn audit_agent_action(&self, request: &AgentAction, status: BrowserAuditStatus) {
        self.record_audit(
            request.action_id.clone(),
            BrowserAuditActor::Agent {
                name: request.actor.clone(),
            },
            status,
            BrowserAuditAction::from_control(&request.action),
        );
    }

    fn record_audit(
        &self,
        action_id: String,
        actor: BrowserAuditActor,
        status: BrowserAuditStatus,
        action: BrowserAuditAction,
    ) {
        let Some(coordination) = self.config.coordination.as_ref() else {
            return;
        };
        let entry = BrowserAuditEntry::new(action_id, actor, status, action);
        if let Err(error) = coordination.record_action(&self.config.panel_local_id, &entry) {
            tracing::warn!(target: "browser", "failed to append WebDriver action audit: {error}");
        }
    }

    pub(super) fn complete_agent_action(
        &self,
        request: &AgentAction,
        outcome: Result<BrowserControlValue, BrowserControlFailure>,
    ) {
        let (status, result) = match outcome {
            Ok(value) => (
                BrowserAuditStatus::Completed,
                AgentActionResult::completed(request.action_id.clone(), value),
            ),
            Err(error) => (
                BrowserAuditStatus::Failed,
                AgentActionResult::failed(request.action_id.clone(), error),
            ),
        };
        self.audit_agent_action(request, status);
        let Some(coordination) = self.config.coordination.as_ref() else {
            return;
        };
        if let Err(error) = coordination.complete_action(&self.config.panel_local_id, &result) {
            tracing::warn!(target: "browser", "failed to publish WebDriver action result: {error}");
        }
    }

    fn coordination_state(&self) -> CoordinationState {
        CoordinationState {
            backend: self.config.browser.backend,
            browser_ws: self.automation_ws.clone(),
            target_id: self.context_id.clone().unwrap_or_default(),
            url: self.url.clone(),
            title: self.title.clone(),
        }
    }

    fn expire_user_active(&mut self) {
        let Some(last_stamp) = self.last_user_active_stamp else {
            return;
        };
        if last_stamp.elapsed() < USER_ACTIVE_TTL {
            return;
        }
        let Some(coordination) = self.config.coordination.as_ref() else {
            self.last_user_active_stamp = None;
            return;
        };
        match coordination.set_user_active(&self.config.panel_local_id, false) {
            Ok(()) => self.last_user_active_stamp = None,
            Err(error) => tracing::warn!(target: "browser", "failed to expire WebDriver user activity: {error}"),
        }
    }
}

fn publish_owner_change(owner_seen: &mut Option<String>, owner: Option<String>, event_tx: &BrowserEventSender) {
    if owner == *owner_seen {
        return;
    }
    owner_seen.clone_from(&owner);
    let _ = event_tx.send(BrowserEvent::OwnerChanged(owner));
}

fn publish_handoff_change(
    handoff_seen: &mut Option<String>,
    handoff: Option<HandoffRequest>,
    event_tx: &BrowserEventSender,
) {
    match handoff {
        Some(handoff) if handoff_seen.as_deref() != Some(handoff.request_id.as_str()) => {
            handoff_seen.clone_from(&Some(handoff.request_id));
            let _ = event_tx.send(BrowserEvent::HandoffRequested(handoff.reason));
        }
        None if handoff_seen.take().is_some() => {
            let _ = event_tx.send(BrowserEvent::HandoffCleared);
        }
        Some(_) | None => {}
    }
}
