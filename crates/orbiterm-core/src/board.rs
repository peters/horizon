use crate::attention::{AttentionId, AttentionItem, AttentionSeverity};
use crate::config::Config;
use crate::error::Result;
use crate::panel::{DEFAULT_PANEL_SIZE, Panel, PanelId, PanelOptions};
use crate::workspace::{Workspace, WorkspaceId};

const TILE_GAP: f32 = 20.0;

pub struct Board {
    pub panels: Vec<Panel>,
    pub workspaces: Vec<Workspace>,
    pub attention: Vec<AttentionItem>,
    pub focused: Option<PanelId>,
    pub active_workspace: Option<WorkspaceId>,
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
            active_workspace: None,
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
                    auto_resize_pty: term_cfg.auto_resize_pty,
                    kind: term_cfg.kind,
                    resume: term_cfg.resume.clone(),
                    position: term_cfg.position,
                    size: term_cfg.size,
                };
                board.create_panel(opts, Some(ws_id))?;
            }
        }

        if let Some(first_workspace) = board.workspaces.first() {
            board.active_workspace = Some(first_workspace.id);
            board.focused = first_workspace
                .panels
                .first()
                .copied()
                .or_else(|| board.panels.first().map(|panel| panel.id));
        }

        Ok(board)
    }

    #[must_use]
    pub fn create_workspace(&mut self, name: &str) -> WorkspaceId {
        let id = WorkspaceId(self.next_workspace_id);
        self.next_workspace_id += 1;
        let color_idx = self.workspaces.len();
        self.workspaces.push(Workspace::new(id, name.to_string(), color_idx));
        self.active_workspace.get_or_insert(id);
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
        let layout_position = opts
            .position
            .unwrap_or_else(|| self.default_panel_position(workspace, id));
        let layout_size = opts.size.unwrap_or(DEFAULT_PANEL_SIZE);
        let mut panel = Panel::spawn(id, opts)?;
        panel.workspace_id = workspace;
        panel.move_to(layout_position);
        panel.resize_layout(layout_size);
        self.panels.push(panel);

        if let Some(ws_id) = workspace
            && let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == ws_id)
        {
            ws.add_panel(id);
        }

        self.focus(id);

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
            if let Some(focused) = self.focused {
                self.active_workspace = self.panel_workspace_id(focused);
            }
        }
    }

    pub fn remove_workspace(&mut self, id: WorkspaceId) {
        if let Some(index) = self.workspaces.iter().position(|workspace| workspace.id == id) {
            let workspace = self.workspaces.remove(index);
            for panel_id in workspace.panels {
                if let Some(panel) = self.panel_mut(panel_id) {
                    panel.move_to([
                        panel.layout.position[0] + workspace.position[0],
                        panel.layout.position[1] + workspace.position[1],
                    ]);
                    panel.workspace_id = None;
                }
            }
        }
        self.attention.retain(|item| item.workspace_id != id);
        if self.active_workspace == Some(id) {
            self.active_workspace = self.workspaces.first().map(|workspace| workspace.id);
        }
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
        self.active_workspace = self.panel_workspace_id(id);
    }

    pub fn focus_workspace(&mut self, id: WorkspaceId) {
        self.active_workspace = Some(id);
        if let Some(workspace) = self.workspace(id)
            && let Some(&panel_id) = workspace.panels.last()
        {
            self.focused = Some(panel_id);
        }
    }

    #[must_use]
    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.iter().find(|workspace| workspace.id == id)
    }

    pub fn workspace_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|workspace| workspace.id == id)
    }

    #[must_use]
    pub fn panel(&self, id: PanelId) -> Option<&Panel> {
        self.panels.iter().find(|panel| panel.id == id)
    }

    pub fn panel_mut(&mut self, id: PanelId) -> Option<&mut Panel> {
        self.panels.iter_mut().find(|panel| panel.id == id)
    }

    #[must_use]
    pub fn panel_workspace_id(&self, id: PanelId) -> Option<WorkspaceId> {
        self.panel(id).and_then(|panel| panel.workspace_id)
    }

    #[must_use]
    pub fn workspace_for_panel(&self, id: PanelId) -> Option<&Workspace> {
        self.panel_workspace_id(id)
            .and_then(|workspace_id| self.workspace(workspace_id))
    }

    pub fn move_workspace(&mut self, id: WorkspaceId, position: [f32; 2]) -> bool {
        if let Some(workspace) = self.workspace_mut(id) {
            workspace.position = position;
            return true;
        }

        false
    }

    pub fn move_panel(&mut self, id: PanelId, position: [f32; 2]) -> bool {
        if let Some(panel) = self.panel_mut(id) {
            panel.move_to(position);
            return true;
        }

        false
    }

    pub fn resize_panel(&mut self, id: PanelId, size: [f32; 2]) -> bool {
        if let Some(panel) = self.panel_mut(id) {
            panel.resize_layout(size);
            return true;
        }

        false
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

    fn default_panel_position(&self, workspace: Option<WorkspaceId>, _panel_id: PanelId) -> [f32; 2] {
        if let Some(workspace_id) = workspace
            && let Some(workspace) = self.workspace(workspace_id)
        {
            return self.first_free_tile_position(workspace);
        }

        self.first_free_orphan_position()
    }

    fn first_free_tile_position(&self, workspace: &Workspace) -> [f32; 2] {
        let occupied: Vec<[f32; 2]> = workspace
            .panels
            .iter()
            .filter_map(|id| self.panel(*id))
            .map(|p| p.layout.position)
            .collect();

        let search_limit = occupied.len();
        for index in 0..=search_limit {
            let candidate = tiled_panel_position(index);
            if !position_occupied(&occupied, candidate) {
                return candidate;
            }
        }

        tiled_panel_position(search_limit)
    }

    fn first_free_orphan_position(&self) -> [f32; 2] {
        let occupied: Vec<[f32; 2]> = self
            .panels
            .iter()
            .filter(|p| p.workspace_id.is_none())
            .map(|p| p.layout.position)
            .collect();

        let search_limit = occupied.len();
        for index in 0..=search_limit {
            let candidate = orphan_panel_position(index);
            if !position_occupied(&occupied, candidate) {
                return candidate;
            }
        }

        orphan_panel_position(search_limit)
    }
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

fn tiled_panel_position(index: usize) -> [f32; 2] {
    let column = usize_to_f32(index % 3);
    let row = usize_to_f32(index / 3);
    [
        90.0 + column * (DEFAULT_PANEL_SIZE[0] + TILE_GAP),
        108.0 + row * (DEFAULT_PANEL_SIZE[1] + TILE_GAP),
    ]
}

fn orphan_panel_position(index: usize) -> [f32; 2] {
    let column = usize_to_f32(index % 3);
    let row = usize_to_f32(index / 3);
    [
        140.0 + column * (DEFAULT_PANEL_SIZE[0] + TILE_GAP),
        140.0 + row * (DEFAULT_PANEL_SIZE[1] + TILE_GAP),
    ]
}

fn position_occupied(positions: &[[f32; 2]], candidate: [f32; 2]) -> bool {
    positions
        .iter()
        .any(|pos| (pos[0] - candidate[0]).abs() < 1.0 && (pos[1] - candidate[1]).abs() < 1.0)
}

fn usize_to_f32(value: usize) -> f32 {
    let clamped = u16::try_from(value).unwrap_or(u16::MAX);
    f32::from(clamped)
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

    #[test]
    fn focusing_panel_tracks_active_workspace() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");
        let panel_id = board
            .create_panel(PanelOptions::default(), Some(workspace_id))
            .expect("panel should spawn");

        board.focus(panel_id);

        assert_eq!(board.active_workspace, Some(workspace_id));
    }

    #[test]
    fn workspace_panel_positions_are_relative() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");
        let panel_id = board
            .create_panel(PanelOptions::default(), Some(workspace_id))
            .expect("panel should spawn");
        let panel = board.panel(panel_id).expect("panel should exist");

        assert!((panel.layout.position[0] - 90.0).abs() <= f32::EPSILON);
        assert!((panel.layout.position[1] - 108.0).abs() <= f32::EPSILON);
    }
}
