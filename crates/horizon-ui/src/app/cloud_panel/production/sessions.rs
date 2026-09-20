//! Recreate local terminal views from durable remote session references.
use super::{HorizonApp, PanelKind, PanelOptions};

impl HorizonApp {
    pub(super) fn restore_missing_cloud_sessions(&mut self, index: usize) {
        let group = &self.cloud_prototype.groups.0[index];
        let Some(workspace) = self.board.workspace_id_by_local_id(&group.workspace) else {
            return;
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
        for session in sessions {
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
                local_id: Some(session.panel_id),
                position: Some(self.cloud_prototype.groups.0[index].next_position(&self.board)),
                transcript_root: self.transcript_root.clone(),
                ..PanelOptions::default()
            };
            if let Err(error) = self.prepare_cloud_remote_panel(index, &mut options) {
                self.cloud_prototype.error = Some(error.to_string());
                continue;
            }
            match self.board.create_panel(options, workspace) {
                Ok(id) => self.cloud_panel_created(index, id),
                Err(error) => self.cloud_prototype.error = Some(error.to_string()),
            }
        }
        if collapsed {
            self.cloud_prototype.groups.0[index].set_collapsed(&mut self.board, true);
        }
        self.save_cloud_prototype();
    }
}
