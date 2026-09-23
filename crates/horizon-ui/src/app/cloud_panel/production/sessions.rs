//! Recreate local terminal views from durable remote session references.
use super::{HorizonApp, PanelKind, PanelOptions};
use std::collections::HashSet;

impl HorizonApp {
    pub(super) fn restore_missing_cloud_sessions(
        &mut self,
        index: usize,
        pending: &HashSet<String>,
    ) -> HashSet<String> {
        let group = &self.cloud_prototype.groups.0[index];
        let Some(workspace) = self.board.workspace_id_by_local_id(&group.workspace) else {
            return pending.clone();
        };
        let collapsed = group.collapsed;
        let sessions = self
            .cloud_prototype
            .production
            .runtimes
            .get(&group.issue)
            .and_then(|runtime| runtime.state.as_ref())
            .map(|state| state.sessions.clone())
            .unwrap_or_default();
        let mut remaining = HashSet::new();
        for session in sessions
            .into_iter()
            .filter(|session| pending.contains(&session.panel_id))
        {
            if self.board.panel_id_by_local_id(&session.panel_id).is_some() {
                continue;
            }
            let kind = match session.agent.as_str() {
                "codex" => PanelKind::Codex,
                "claude" => PanelKind::Claude,
                "grok" => PanelKind::Grok,
                "shell" => PanelKind::Shell,
                _ => continue,
            };
            let mut options = PanelOptions {
                kind,
                local_id: Some(session.panel_id.clone()),
                position: Some(self.cloud_prototype.groups.0[index].next_position(&self.board)),
                transcript_root: self.transcript_root.clone(),
                ..PanelOptions::default()
            };
            if let Err(error) = self.prepare_cloud_remote_panel(index, &mut options) {
                self.cloud_prototype.error = Some(error.to_string());
                remaining.insert(session.panel_id);
                continue;
            }
            match self.create_cloud_member(index, options, workspace) {
                Ok(_) => {}
                Err(error) => {
                    self.cloud_prototype.error = Some(error.to_string());
                    remaining.insert(session.panel_id);
                }
            }
        }
        if collapsed {
            self.cloud_prototype.groups.0[index].set_collapsed(&mut self.board, true);
        }
        self.save_cloud_prototype();
        remaining
    }
}
