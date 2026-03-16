use std::time::Duration;

use crate::attention::{AttentionId, AttentionItem, AttentionSeverity};
use crate::panel::PanelId;
use crate::workspace::WorkspaceId;

use super::Board;

impl Board {
    pub(super) fn update_attention(&mut self) {
        // Only inspect panels that received new terminal output this frame.
        // `detect_attention()` locks the terminal and iterates the full
        // display, so skipping idle panels avoids significant per-frame cost.
        let panel_states: Vec<_> = self
            .panels
            .iter_mut()
            .filter(|panel| panel.had_recent_output)
            .map(|panel| {
                let bell = panel.take_bell();
                let notification = panel.take_notification();
                (
                    panel.id,
                    panel.workspace_id,
                    panel.kind,
                    panel.detect_attention(),
                    bell,
                    notification,
                    panel.launched_at_millis,
                )
            })
            .collect();

        for (panel_id, workspace_id, kind, attention, bell, notification, launched_at) in panel_states {
            if let Some(notif) = notification {
                let severity = match notif.severity.as_str() {
                    "attention" => AttentionSeverity::High,
                    "done" => AttentionSeverity::Medium,
                    _ => AttentionSeverity::Low,
                };
                self.create_attention(workspace_id, Some(panel_id), "agent-notify", notif.message, severity);
            }

            if let Some(summary) = attention {
                self.reconcile_agent_attention_signal(panel_id, workspace_id, summary);
            } else {
                self.reconcile_agent_attention_signal(panel_id, workspace_id, "");
            }

            let has_open = self.unresolved_attention_for_panel(panel_id).is_some();

            if attention.is_none() && bell && kind.is_agent() {
                let age_ms = crate::panel::current_unix_millis().saturating_sub(launched_at);
                if !has_open && age_ms >= 10_000 {
                    self.create_attention(
                        workspace_id,
                        Some(panel_id),
                        "agent",
                        "Needs attention",
                        AttentionSeverity::High,
                    );
                }
            } else if attention.is_none() && has_open {
                let ids_to_resolve: Vec<_> = self
                    .attention
                    .iter()
                    .filter(|item| item.panel_id == Some(panel_id) && item.is_open())
                    .map(|item| item.id)
                    .collect();
                for id in ids_to_resolve {
                    let _ = self.resolve_attention(id);
                }
            }
        }

        self.dismiss_expired_ready_attention(super::READY_FOR_INPUT_AUTO_DISMISS_AFTER);
    }

    pub(crate) fn reconcile_agent_attention_signal(
        &mut self,
        panel_id: PanelId,
        workspace_id: WorkspaceId,
        summary: &str,
    ) {
        let next_signal = (!summary.is_empty()).then_some(summary);
        let previous_signal = self.panel_attention_signals.get(&panel_id).map(String::as_str);
        if previous_signal == next_signal {
            return;
        }

        self.resolve_open_attention_for_panel(panel_id);

        match next_signal {
            Some(summary) => {
                self.create_attention(workspace_id, Some(panel_id), "agent", summary, AttentionSeverity::High);
                self.panel_attention_signals.insert(panel_id, summary.to_string());
            }
            None => {
                self.panel_attention_signals.remove(&panel_id);
            }
        }
    }

    fn resolve_open_attention_for_panel(&mut self, panel_id: PanelId) {
        let ids_to_resolve: Vec<_> = self
            .attention
            .iter()
            .filter(|item| item.panel_id == Some(panel_id) && item.is_open())
            .map(|item| item.id)
            .collect();
        for id in ids_to_resolve {
            let _ = self.resolve_attention(id);
        }
    }

    pub(crate) fn dismiss_expired_ready_attention(&mut self, max_age: Duration) {
        let now = std::time::SystemTime::now();
        for item in &mut self.attention {
            let should_dismiss = item.is_open()
                && item.is_agent_ready_for_input()
                && now.duration_since(item.created_at).is_ok_and(|age| age >= max_age);
            if should_dismiss {
                item.dismiss();
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

    #[must_use]
    pub fn resolve_attention(&mut self, id: AttentionId) -> bool {
        if let Some(item) = self.attention.iter_mut().find(|item| item.id == id) {
            item.resolve();
            return true;
        }

        false
    }

    #[must_use]
    pub fn dismiss_attention(&mut self, id: AttentionId) -> bool {
        if let Some(item) = self.attention.iter_mut().find(|item| item.id == id) {
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
