use crate::attention::{AttentionId, AttentionItem, AttentionSeverity};
use crate::config::Config;
use crate::error::Result;
use crate::panel::{Panel, PanelId, PanelOptions};
use crate::workspace::{Workspace, WorkspaceId};

pub struct Board {
    pub panels: Vec<Panel>,
    pub workspaces: Vec<Workspace>,
    pub attention: Vec<AttentionItem>,
    pub focused: Option<PanelId>,
    next_panel_id: u64,
    next_workspace_id: u64,
    next_attention_id: u64,
}

impl Board {
    #[must_use]
    pub fn new() -> Self {
        Self {
            panels: Vec::new(),
            workspaces: Vec::new(),
            attention: Vec::new(),
            focused: None,
            next_panel_id: 1,
            next_workspace_id: 1,
            next_attention_id: 1,
        }
    }

    /// Build a board from a YAML config.
    ///
    /// # Errors
    ///
    /// Returns an error if any configured panel fails to spawn.
    pub fn from_config(config: &Config) -> Result<Self> {
        let mut board = Self::new();

        for ws_cfg in &config.workspaces {
            let ws_id = board.create_workspace(&ws_cfg.name);

            if let Some(pos) = ws_cfg.position
                && let Some(ws) = board.workspaces.iter_mut().find(|w| w.id == ws_id)
            {
                ws.position = pos;
            }

            for term_cfg in &ws_cfg.terminals {
                let opts = PanelOptions {
                    name: Some(term_cfg.name.clone()),
                    command: term_cfg.command.clone(),
                    args: term_cfg.args.clone(),
                    cwd: term_cfg.cwd.as_ref().map(|s| Config::expand_tilde(s)),
                    rows: term_cfg.rows,
                    cols: term_cfg.cols,
                };
                board.create_panel(opts, Some(ws_id))?;
            }
        }

        Ok(board)
    }

    #[must_use]
    pub fn create_workspace(&mut self, name: &str) -> WorkspaceId {
        let id = WorkspaceId(self.next_workspace_id);
        self.next_workspace_id += 1;
        let color_idx = self.workspaces.len();
        self.workspaces.push(Workspace::new(id, name.to_string(), color_idx));
        tracing::info!("created workspace '{}' ({})", name, id.0);
        id
    }

    /// Create a panel and optionally attach it to a workspace.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying PTY-backed panel cannot be spawned.
    pub fn create_panel(&mut self, opts: PanelOptions, workspace: Option<WorkspaceId>) -> Result<PanelId> {
        let id = PanelId(self.next_panel_id);
        self.next_panel_id += 1;
        let panel = Panel::spawn(id, opts)?;
        self.panels.push(panel);
        self.focused = Some(id);

        if let Some(ws_id) = workspace
            && let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == ws_id)
        {
            ws.add_panel(id);
        }

        Ok(id)
    }

    pub fn close_panel(&mut self, id: PanelId) {
        self.panels.retain(|p| p.id != id);
        for ws in &mut self.workspaces {
            ws.remove_panel(id);
        }
        self.attention.retain(|item| item.panel_id != Some(id));
        if self.focused == Some(id) {
            self.focused = self.panels.last().map(|p| p.id);
        }
    }

    pub fn remove_workspace(&mut self, id: WorkspaceId) {
        self.workspaces.retain(|w| w.id != id);
        self.attention.retain(|item| item.workspace_id != id);
    }

    #[must_use]
    pub fn rename_workspace(&mut self, id: WorkspaceId, name: &str) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return false;
        }

        if let Some(workspace) = self.workspaces.iter_mut().find(|workspace| workspace.id == id) {
            trimmed.clone_into(&mut workspace.name);
            return true;
        }

        false
    }

    pub fn process_output(&mut self) {
        for panel in &mut self.panels {
            panel.process_output();
        }
    }

    pub fn focus(&mut self, id: PanelId) {
        self.focused = Some(id);
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

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_workspace_updates_matching_workspace() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");

        assert!(board.rename_workspace(workspace_id, "backend"));
        assert_eq!(board.workspaces[0].name, "backend");
    }

    #[test]
    fn rename_workspace_rejects_blank_names() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");

        assert!(!board.rename_workspace(workspace_id, "   "));
        assert_eq!(board.workspaces[0].name, "frontend");
    }

    #[test]
    fn close_panel_removes_panel_attention() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");
        let panel_id = PanelId(7);

        board.create_attention(
            workspace_id,
            Some(panel_id),
            "codex-ui",
            "Needs user feedback",
            AttentionSeverity::High,
        );

        board.close_panel(panel_id);

        assert!(board.unresolved_attention().next().is_none());
    }

    #[test]
    fn resolve_attention_marks_item_resolved() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");
        let attention_id = board.create_attention(
            workspace_id,
            None,
            "system",
            "Review build result",
            AttentionSeverity::Medium,
        );

        assert!(board.resolve_attention(attention_id));
        assert!(board.unresolved_attention().next().is_none());
    }
}
