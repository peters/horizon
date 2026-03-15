use crate::attention::{AttentionId, AttentionItem, AttentionSeverity};
use crate::config::Config;
use crate::error::Result;
use crate::panel::{DEFAULT_PANEL_SIZE, Panel, PanelId, PanelOptions};
use crate::workspace::{Workspace, WorkspaceId};

const TILE_GAP: f32 = 20.0;
const WS_INNER_PAD: f32 = 20.0;
const WORKSPACE_GAP: f32 = 80.0;

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

            if let Some(pos) = ws_cfg.position {
                board.move_workspace(ws_id, pos);
            }

            let ws_origin = board.workspace(ws_id).map_or([0.0, 0.0], |ws| ws.position);

            for term_cfg in &ws_cfg.terminals {
                let position = term_cfg
                    .position
                    .map(|pos| [ws_origin[0] + pos[0], ws_origin[1] + pos[1]]);
                let opts = PanelOptions {
                    name: Some(term_cfg.name.clone()),
                    command: term_cfg.command.clone(),
                    args: term_cfg.args.clone(),
                    cwd: term_cfg.cwd.as_ref().map(|s| Config::expand_tilde(s)),
                    rows: term_cfg.rows,
                    cols: term_cfg.cols,
                    kind: term_cfg.kind,
                    resume: term_cfg.resume.clone(),
                    position,
                    size: term_cfg.size,
                };
                board.create_panel(opts, ws_id)?;
            }
        }

        // Ensure at least one workspace always exists.
        if board.workspaces.is_empty() {
            let _ = board.create_workspace("default");
        }

        board.focused = board.panels.first().map(|panel| panel.id);

        Ok(board)
    }

    #[must_use]
    pub fn create_workspace(&mut self, name: &str) -> WorkspaceId {
        let id = WorkspaceId(self.next_workspace_id);
        self.next_workspace_id += 1;
        let color_idx = self.workspaces.len();
        let position = self.next_workspace_position();
        let mut ws = Workspace::new(id, name.to_string(), color_idx);
        ws.position = position;
        self.workspaces.push(ws);
        self.active_workspace.get_or_insert(id);
        tracing::info!(
            "created workspace '{}' ({}) at [{}, {}]",
            name,
            id.0,
            position[0],
            position[1]
        );
        id
    }

    /// Ensures at least one workspace exists. Returns the active workspace,
    /// creating a default one if the board has none.
    pub fn ensure_workspace(&mut self) -> WorkspaceId {
        if let Some(id) = self.active_workspace {
            return id;
        }
        if let Some(ws) = self.workspaces.first() {
            let id = ws.id;
            self.active_workspace = Some(id);
            return id;
        }
        self.create_workspace("default")
    }

    /// Create a panel inside a workspace.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying PTY-backed panel cannot be spawned.
    pub fn create_panel(&mut self, opts: PanelOptions, workspace: WorkspaceId) -> Result<PanelId> {
        let id = PanelId(self.next_panel_id);
        self.next_panel_id += 1;
        let layout_position = opts.position.unwrap_or_else(|| self.default_panel_position(workspace));
        let layout_size = opts.size.unwrap_or(DEFAULT_PANEL_SIZE);
        let mut panel = Panel::spawn(id, workspace, opts)?;
        panel.move_to(layout_position);
        panel.resize_layout(layout_size);
        self.panels.push(panel);

        if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == workspace) {
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
        // Never remove the last workspace.
        if self.workspaces.len() <= 1 {
            return;
        }

        let Some(target_id) = self.workspaces.iter().find(|ws| ws.id != id).map(|ws| ws.id) else {
            return;
        };

        if let Some(index) = self.workspaces.iter().position(|workspace| workspace.id == id) {
            let removed = self.workspaces.remove(index);
            for panel_id in removed.panels {
                self.assign_panel_to_workspace(panel_id, target_id);
            }
        }

        self.attention.retain(|item| item.workspace_id != id);
        if self.active_workspace == Some(id) {
            self.active_workspace = Some(target_id);
        }
    }

    /// Move a panel to a different workspace, physically relocating it to
    /// the next free tile position in the target workspace.
    pub fn assign_panel_to_workspace(&mut self, panel_id: PanelId, workspace_id: WorkspaceId) {
        for ws in &mut self.workspaces {
            ws.remove_panel(panel_id);
        }

        // Compute new position before adding, so it finds a truly free tile.
        let new_position = self.default_panel_position(workspace_id);

        if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == workspace_id) {
            ws.add_panel(panel_id);
        }
        if let Some(panel) = self.panel_mut(panel_id) {
            panel.workspace_id = workspace_id;
            panel.move_to(new_position);
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
        self.panel(id).map(|panel| panel.workspace_id)
    }

    #[must_use]
    pub fn workspace_for_panel(&self, id: PanelId) -> Option<&Workspace> {
        self.panel_workspace_id(id)
            .and_then(|workspace_id| self.workspace(workspace_id))
    }

    /// Computes the bounding rectangle of all panels in a workspace.
    /// Returns `(min, max)` in canvas coordinates, or `None` when the
    /// workspace is empty or does not exist.
    #[must_use]
    pub fn workspace_bounds(&self, id: WorkspaceId) -> Option<([f32; 2], [f32; 2])> {
        let workspace = self.workspace(id)?;
        let mut panels = workspace.panels.iter().filter_map(|pid| self.panel(*pid)).peekable();
        panels.peek()?;

        let mut min = [f32::MAX, f32::MAX];
        let mut max = [f32::MIN, f32::MIN];
        for panel in panels {
            min[0] = min[0].min(panel.layout.position[0]);
            min[1] = min[1].min(panel.layout.position[1]);
            max[0] = max[0].max(panel.layout.position[0] + panel.layout.size[0]);
            max[1] = max[1].max(panel.layout.position[1] + panel.layout.size[1]);
        }

        Some((min, max))
    }

    pub fn move_workspace(&mut self, id: WorkspaceId, position: [f32; 2]) -> bool {
        if let Some(workspace) = self.workspace_mut(id) {
            workspace.position = position;
            return true;
        }

        false
    }

    pub fn translate_workspace(&mut self, id: WorkspaceId, delta: [f32; 2]) -> bool {
        if delta == [0.0, 0.0] {
            return false;
        }

        let Some(panel_ids) = self.workspace(id).map(|workspace| workspace.panels.clone()) else {
            return false;
        };

        if let Some(workspace) = self.workspace_mut(id) {
            workspace.position[0] += delta[0];
            workspace.position[1] += delta[1];
        }

        for panel_id in panel_ids {
            if let Some(panel) = self.panel_mut(panel_id) {
                panel.move_to([panel.layout.position[0] + delta[0], panel.layout.position[1] + delta[1]]);
            }
        }

        true
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

    /// Compute the canvas position for the next workspace so it doesn't
    /// overlap with existing ones.  Uses fixed-width slots so workspaces
    /// never collide even when fully populated (3 columns).
    fn next_workspace_position(&self) -> [f32; 2] {
        let mut right_edge: f32 = 0.0;
        for ws in &self.workspaces {
            right_edge = right_edge.max(ws.position[0] + workspace_slot_width());
        }
        [right_edge, 40.0]
    }

    fn default_panel_position(&self, workspace: WorkspaceId) -> [f32; 2] {
        if let Some(ws) = self.workspace(workspace) {
            return self.first_free_tile_position(ws);
        }
        tiled_panel_position([0.0, 0.0], 0)
    }

    fn first_free_tile_position(&self, workspace: &Workspace) -> [f32; 2] {
        let occupied: Vec<[f32; 2]> = workspace
            .panels
            .iter()
            .filter_map(|id| self.panel(*id))
            .map(|p| p.layout.position)
            .collect();

        let origin = workspace.position;
        let search_limit = occupied.len();
        for index in 0..=search_limit {
            let candidate = tiled_panel_position(origin, index);
            if !position_occupied(&occupied, candidate) {
                return candidate;
            }
        }

        tiled_panel_position(origin, search_limit)
    }
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed horizontal space reserved per workspace (3 panel columns + padding + gap).
fn workspace_slot_width() -> f32 {
    let columns = 3.0;
    let content = columns * DEFAULT_PANEL_SIZE[0] + (columns - 1.0) * TILE_GAP;
    content + 2.0 * WS_INNER_PAD + WORKSPACE_GAP
}

fn tiled_panel_position(origin: [f32; 2], index: usize) -> [f32; 2] {
    let column = usize_to_f32(index % 3);
    let row = usize_to_f32(index / 3);
    [
        origin[0] + WS_INNER_PAD + column * (DEFAULT_PANEL_SIZE[0] + TILE_GAP),
        origin[1] + WS_INNER_PAD + row * (DEFAULT_PANEL_SIZE[1] + TILE_GAP),
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
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("panel should spawn");

        board.focus(panel_id);

        assert_eq!(board.active_workspace, Some(workspace_id));
    }

    #[test]
    fn panels_tile_within_workspace_region() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");
        let ws_pos = board.workspace(workspace_id).unwrap().position;

        let panel_id = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("panel should spawn");
        let panel = board.panel(panel_id).expect("panel should exist");

        // First panel tiles at workspace origin + inner padding.
        assert!((panel.layout.position[0] - (ws_pos[0] + WS_INNER_PAD)).abs() <= f32::EPSILON);
        assert!((panel.layout.position[1] - (ws_pos[1] + WS_INNER_PAD)).abs() <= f32::EPSILON);
    }

    #[test]
    fn workspaces_are_placed_apart() {
        let mut board = Board::new();
        let ws1 = board.create_workspace("first");
        board
            .create_panel(PanelOptions::default(), ws1)
            .expect("panel should spawn");
        let ws2 = board.create_workspace("second");

        let pos1 = board.workspace(ws1).unwrap().position;
        let pos2 = board.workspace(ws2).unwrap().position;

        // Second workspace must start to the right of the first.
        assert!(pos2[0] > pos1[0] + DEFAULT_PANEL_SIZE[0]);
    }

    #[test]
    fn assign_panel_moves_it_to_target_workspace() {
        let mut board = Board::new();
        let ws1 = board.create_workspace("source");
        let ws2 = board.create_workspace("target");

        let panel_id = board
            .create_panel(PanelOptions::default(), ws1)
            .expect("panel should spawn");

        let ws2_pos = board.workspace(ws2).unwrap().position;
        board.assign_panel_to_workspace(panel_id, ws2);

        let panel = board.panel(panel_id).unwrap();
        assert_eq!(panel.workspace_id, ws2);
        // Panel should be within the target workspace's region.
        assert!(panel.layout.position[0] >= ws2_pos[0]);
    }

    #[test]
    fn translating_workspace_moves_workspace_origin_and_panels() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");
        let panel_id = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("panel should spawn");

        let original_workspace_pos = board.workspace(workspace_id).expect("workspace").position;
        let original_panel_pos = board.panel(panel_id).expect("panel").layout.position;

        assert!(board.translate_workspace(workspace_id, [48.0, 24.0]));

        let workspace = board.workspace(workspace_id).expect("workspace");
        let panel = board.panel(panel_id).expect("panel");
        assert!((workspace.position[0] - (original_workspace_pos[0] + 48.0)).abs() <= f32::EPSILON);
        assert!((workspace.position[1] - (original_workspace_pos[1] + 24.0)).abs() <= f32::EPSILON);
        assert!((panel.layout.position[0] - (original_panel_pos[0] + 48.0)).abs() <= f32::EPSILON);
        assert!((panel.layout.position[1] - (original_panel_pos[1] + 24.0)).abs() <= f32::EPSILON);
    }
}
