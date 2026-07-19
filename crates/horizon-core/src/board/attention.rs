use std::time::{Duration, SystemTime};

use crate::attention::{AttentionId, AttentionItem, AttentionSeverity, AttentionState, RESOLVED_ATTENTION_RETENTION};
use crate::panel::{AgentAttentionSignal, PanelId, current_unix_millis};
use crate::workspace::WorkspaceId;

use super::Board;

struct AgentAttentionObservation<'a> {
    summary: &'a str,
    fingerprint: &'a str,
}

pub(super) fn should_baseline_attention_signal(signal: &AgentAttentionSignal) -> bool {
    signal.is_ready_for_input()
}

impl Board {
    pub(super) fn update_attention(&mut self) {
        let panel_states: Vec<_> = self
            .panels
            .iter_mut()
            .filter_map(|panel| {
                let initial_scan_pending = panel.take_initial_attention_scan_pending();
                let should_scan = panel.had_recent_output || initial_scan_pending;
                if !should_scan {
                    return None;
                }

                let bell = panel.take_bell();
                let notifications = panel.take_notifications();
                Some((
                    panel.id,
                    panel.workspace_id,
                    panel.kind,
                    panel.detect_attention_signal(),
                    bell,
                    notifications,
                    panel.launched_at_millis,
                ))
            })
            .collect();

        for (panel_id, workspace_id, kind, attention_scan, bell, notifications, launched_at) in panel_states {
            for notification in notifications {
                let severity = match notification.severity.as_str() {
                    "attention" => AttentionSeverity::High,
                    "done" => AttentionSeverity::Medium,
                    _ => AttentionSeverity::Low,
                };
                self.create_attention(
                    workspace_id,
                    Some(panel_id),
                    "agent-notify",
                    notification.message,
                    severity,
                );
            }

            self.observe_agent_attention_signal(panel_id, workspace_id, attention_scan.as_ref(), launched_at);

            let has_open = self.unresolved_attention_for_panel(panel_id).is_some();
            let has_detected_signal = attention_scan.is_some();

            if !has_detected_signal && bell && kind.is_agent() {
                let age_ms = current_unix_millis().saturating_sub(launched_at);
                let grace_ms = i64::try_from(super::ATTENTION_STARTUP_GRACE.as_millis()).unwrap_or(i64::MAX);
                if !has_open && age_ms >= grace_ms {
                    self.create_attention(
                        workspace_id,
                        Some(panel_id),
                        "agent",
                        "Needs attention",
                        AttentionSeverity::High,
                    );
                    self.panel_attention_signals.insert(panel_id, "agent-bell".to_string());
                }
            }
        }

        let now = SystemTime::now();
        self.dismiss_expired_ready_attention_at(now, super::READY_FOR_INPUT_AUTO_DISMISS_AFTER);
        self.prune_closed_attention(now, RESOLVED_ATTENTION_RETENTION);
    }

    pub(crate) fn observe_agent_attention_signal(
        &mut self,
        panel_id: PanelId,
        workspace_id: WorkspaceId,
        signal: Option<&AgentAttentionSignal>,
        launched_at: i64,
    ) {
        let age_ms = current_unix_millis().saturating_sub(launched_at);
        let grace_ms = i64::try_from(super::ATTENTION_STARTUP_GRACE.as_millis()).unwrap_or(i64::MAX);
        if let Some(signal) = signal
            && age_ms < grace_ms
            && signal.is_ready_for_input()
            && self.panel_attention_startup_baselined.insert(panel_id)
        {
            self.resolve_open_agent_attention_for_panel(panel_id);
            self.panel_attention_signals
                .insert(panel_id, signal.fingerprint.clone());
            return;
        }

        let observation = signal.map(|signal| AgentAttentionObservation {
            summary: signal.summary,
            fingerprint: signal.fingerprint.as_str(),
        });
        self.reconcile_agent_attention_observation(panel_id, workspace_id, observation);
    }

    #[cfg(test)]
    pub(crate) fn reconcile_agent_attention_signal(
        &mut self,
        panel_id: PanelId,
        workspace_id: WorkspaceId,
        summary: &str,
    ) {
        let observation = (!summary.is_empty()).then_some(AgentAttentionObservation {
            summary,
            fingerprint: summary,
        });
        self.reconcile_agent_attention_observation(panel_id, workspace_id, observation);
    }

    fn reconcile_agent_attention_observation(
        &mut self,
        panel_id: PanelId,
        workspace_id: WorkspaceId,
        observation: Option<AgentAttentionObservation<'_>>,
    ) {
        let fingerprint = observation.as_ref().map(|observation| observation.fingerprint);
        let previous_fingerprint = self.panel_attention_signals.get(&panel_id).map(String::as_str);
        if previous_fingerprint == fingerprint {
            if observation.is_none() {
                self.resolve_open_agent_attention_for_panel(panel_id);
            }
            return;
        }

        self.resolve_open_agent_attention_for_panel(panel_id);

        match observation {
            Some(observation) => {
                self.create_attention(
                    workspace_id,
                    Some(panel_id),
                    "agent",
                    observation.summary,
                    AttentionSeverity::High,
                );
                self.panel_attention_signals
                    .insert(panel_id, observation.fingerprint.to_string());
            }
            None => {
                self.panel_attention_signals.remove(&panel_id);
            }
        }
    }

    fn resolve_open_agent_attention_for_panel(&mut self, panel_id: PanelId) {
        let ids_to_resolve: Vec<_> = self
            .attention
            .iter()
            .filter(|item| item.panel_id == Some(panel_id) && item.is_open() && item.is_agent_heuristic())
            .map(|item| item.id)
            .collect();
        for id in ids_to_resolve {
            let _ = self.resolve_attention(id);
        }
    }

    #[cfg(test)]
    pub(crate) fn dismiss_expired_ready_attention(&mut self, max_age: Duration) {
        self.dismiss_expired_ready_attention_at(SystemTime::now(), max_age);
    }

    fn dismiss_expired_ready_attention_at(&mut self, now: SystemTime, max_age: Duration) {
        for item in &mut self.attention {
            let should_dismiss = item.is_open()
                && item.is_agent_ready_for_input()
                && now.duration_since(item.created_at).is_ok_and(|age| age >= max_age);
            if should_dismiss {
                item.dismiss();
            }
        }
    }

    pub(crate) fn prune_closed_attention(&mut self, now: SystemTime, retention: Duration) {
        self.attention.retain(|item| match item.state {
            AttentionState::Open => true,
            AttentionState::Dismissed => false,
            AttentionState::Resolved => item
                .resolved_at
                .is_some_and(|resolved_at| now.duration_since(resolved_at).map_or(true, |age| age < retention)),
        });
    }

    pub(super) fn reset_panel_attention_tracking(&mut self, panel_id: PanelId) {
        self.resolve_open_agent_attention_for_panel(panel_id);
        self.panel_attention_signals.remove(&panel_id);
        self.panel_attention_startup_baselined.remove(&panel_id);
    }

    pub(super) fn discard_pending_attention_events(&mut self) {
        for panel in &mut self.panels {
            let _ = panel.take_bell();
            drop(panel.take_notifications());
        }
    }

    pub fn set_attention_enabled(&mut self, enabled: bool) {
        if !enabled {
            self.attention_enabled = false;
            self.attention.clear();
            self.panel_attention_signals.clear();
            self.panel_attention_startup_baselined.clear();
            self.discard_pending_attention_events();
            return;
        }

        if self.attention_enabled {
            return;
        }
        self.attention_enabled = enabled;
        self.seed_current_agent_attention_signals();
    }

    fn seed_current_agent_attention_signals(&mut self) {
        // Re-arm the initial scan so actionable prompts and OSC events that
        // appeared while attention was disabled are handled on the next update.
        // Only an ordinary ready prompt is safe to baseline here; approval and
        // question prompts must remain actionable.
        let signals: Vec<_> = self
            .panels
            .iter_mut()
            .filter(|panel| panel.kind.is_agent())
            .map(|panel| {
                panel.mark_initial_attention_scan_pending();
                (panel.id, panel.detect_attention_signal())
            })
            .collect();
        for (panel_id, signal) in signals {
            if let Some(signal) = signal
                && should_baseline_attention_signal(&signal)
            {
                self.panel_attention_signals.insert(panel_id, signal.fingerprint);
                self.panel_attention_startup_baselined.insert(panel_id);
            }
        }
    }

    pub fn create_attention(
        &mut self,
        workspace_id: WorkspaceId,
        panel_id: Option<PanelId>,
        source: impl Into<String>,
        summary: impl Into<String>,
        severity: AttentionSeverity,
    ) -> AttentionId {
        let id = AttentionId(self.next_attention_id);
        self.next_attention_id += 1;
        self.attention.push(AttentionItem::new(
            id,
            workspace_id,
            panel_id,
            source,
            summary,
            severity,
        ));
        id
    }

    pub(super) fn reassign_panel_attention(&mut self, panel_id: PanelId, workspace_id: WorkspaceId) {
        for item in self.attention.iter_mut().filter(|item| item.panel_id == Some(panel_id)) {
            item.workspace_id = workspace_id;
        }
    }

    #[must_use]
    pub fn resolve_attention(&mut self, id: AttentionId) -> bool {
        if let Some(item) = self.attention.iter_mut().find(|item| item.id == id) {
            if !item.is_open() {
                return false;
            }
            item.resolve();
            return true;
        }

        false
    }

    #[must_use]
    pub fn dismiss_attention(&mut self, id: AttentionId) -> bool {
        if let Some(item) = self.attention.iter_mut().find(|item| item.id == id) {
            if !item.is_open() {
                return false;
            }
            item.dismiss();
            return true;
        }

        false
    }

    pub fn unresolved_attention(&self) -> impl Iterator<Item = &AttentionItem> + '_ {
        self.attention.iter().filter(|item| item.is_open())
    }

    #[must_use]
    pub fn unresolved_attention_for_panel(&self, panel_id: PanelId) -> Option<&AttentionItem> {
        self.unresolved_attention()
            .filter(|item| item.panel_id == Some(panel_id))
            .max_by(|left, right| {
                left.severity
                    .cmp(&right.severity)
                    .then_with(|| left.id.0.cmp(&right.id.0))
            })
    }
}
