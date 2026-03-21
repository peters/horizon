use crate::panel::PanelId;

use super::Board;

impl Board {
    /// Evaluate agent chains and inject commands into target agents.
    ///
    /// Uses the collect-then-process pattern established by `update_attention()`
    /// to work around borrow-checker constraints.
    pub(super) fn tick_orchestration(&mut self) {
        // Phase 1: Read panel status into a lookup table (immutable borrow).
        let panel_statuses: Vec<_> = self
            .panels
            .iter()
            .filter_map(|p| {
                let status = p.agent_status.as_ref()?.status;
                Some((p.local_id.clone(), p.id, status))
            })
            .collect();

        // Phase 2: Evaluate chains against collected statuses (mutable borrow
        // on workspaces only).
        let mut actions: Vec<(PanelId, String)> = Vec::new();
        for ws in &mut self.workspaces {
            for chain in &mut ws.orchestration.chains {
                if chain.one_shot && chain.fired {
                    continue;
                }

                let Some(source_status) = panel_statuses
                    .iter()
                    .find(|(lid, _, _)| *lid == chain.source_panel_local_id)
                    .map(|(_, _, s)| *s)
                else {
                    continue;
                };

                let Some(target_id) = panel_statuses
                    .iter()
                    .find(|(lid, _, _)| *lid == chain.target_panel_local_id)
                    .map(|(_, id, _)| *id)
                else {
                    continue;
                };

                if source_status == chain.trigger_status {
                    if !chain.fired {
                        chain.fired = true;
                        actions.push((target_id, chain.command.clone()));
                    }
                } else {
                    chain.fired = false;
                }
            }
        }

        // Phase 3: Inject commands (mutable borrow on panels).
        for (target_id, command) in actions {
            if let Some(panel) = self.panel_mut(target_id) {
                let input = format!("{command}\n");
                panel.write_input(input.as_bytes());
                tracing::info!(target = target_id.0, "orchestration: chain fired");
            }
        }
    }

    /// Send text to a target agent panel's stdin.
    pub fn send_to_agent(&mut self, target: PanelId, text: &str) {
        if let Some(panel) = self.panel_mut(target) {
            panel.write_input(text.as_bytes());
        }
    }

    /// Capture the last `line_count` lines from a panel and store them as
    /// pinned context in the panel's workspace.
    #[must_use]
    pub fn share_output(&mut self, source: PanelId, line_count: usize) -> Option<String> {
        let panel = self.panel(source)?;
        let terminal = panel.content.terminal()?;
        let text = terminal.last_lines_text(line_count);
        let ws_id = panel.workspace_id;
        let key = format!("shared-output-{}", source.0);
        if let Some(ws) = self.workspace_mut(ws_id) {
            ws.context.publish(key, text.clone(), Some(source));
        }
        Some(text)
    }
}
