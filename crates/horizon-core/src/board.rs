use std::path::Path;
use std::time::Duration;

use crate::attention::{AttentionId, AttentionItem, AttentionSeverity};
use crate::config::Config;
use crate::error::Result;
use crate::layout::{
    TILE_GAP, WS_COLLISION_GAP, WS_EMPTY_FRAME_SIZE, WS_FRAME_PAD, WS_FRAME_TOP_EXTRA, WS_INNER_PAD, ceil_sqrt_usize,
    tiled_panel_position, usize_to_f32, workspace_slot_width,
};
use crate::panel::{DEFAULT_PANEL_SIZE, Panel, PanelId, PanelOptions};
use crate::runtime_state::{RuntimeState, WorkspaceState};
use crate::workspace::{Workspace, WorkspaceId};

const PANEL_CHROME_PAD: f32 = 8.0;
const PANEL_CHROME_TITLEBAR: f32 = 34.0;
const AGENT_PANEL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const STACK_OFFSET_X: f32 = 16.0;
const STACK_OFFSET_Y: f32 = 20.0;
const CASCADE_OFFSET_X: f32 = 40.0;
const CASCADE_OFFSET_Y: f32 = 30.0;

/// Predefined layout arrangements for panels inside a workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceLayout {
    /// Single column, panels stacked top-to-bottom.
    Rows,
    /// Single row, panels side by side.
    Columns,
    /// Square-ish grid (auto columns).
    Grid,
    /// Layered pile with slight offsets to keep nearby panels accessible.
    Stack,
    /// Diagonal overlap that fans panels across the workspace.
    Cascade,
}

impl WorkspaceLayout {
    pub const ALL: [Self; 5] = [Self::Rows, Self::Columns, Self::Grid, Self::Stack, Self::Cascade];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rows => "Rows",
            Self::Columns => "Columns",
            Self::Grid => "Grid",
            Self::Stack => "Stack",
            Self::Cascade => "Cascade",
        }
    }
}

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
        Self::from_runtime_state(&RuntimeState::from_config(config))
    }

    /// Build a board from a persisted runtime state snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error if any configured panel fails to spawn.
    pub fn from_runtime_state(state: &RuntimeState) -> Result<Self> {
        Self::from_runtime_state_with_transcripts(state, None)
    }

    /// Build a board from a persisted runtime state snapshot and optional transcript root.
    ///
    /// # Errors
    ///
    /// Returns an error if any configured panel fails to spawn.
    pub fn from_runtime_state_with_transcripts(state: &RuntimeState, transcript_root: Option<&Path>) -> Result<Self> {
        let mut board = Self::new();

        for workspace_state in &state.workspaces {
            let ws_id = board.create_workspace_record(workspace_state);
            for panel_state in &workspace_state.panels {
                let mut options = panel_state.to_panel_options();
                options.transcript_root = transcript_root.map(Path::to_path_buf);
                board.create_panel(options, ws_id)?;
            }
        }

        if let Some(local_id) = &state.active_workspace_local_id
            && let Some(workspace_id) = board.workspace_id_by_local_id(local_id)
        {
            board.active_workspace = Some(workspace_id);
        }

        if let Some(local_id) = &state.focused_panel_local_id
            && let Some(panel_id) = board.panel_id_by_local_id(local_id)
        {
            board.focused = Some(panel_id);
            board.active_workspace = board.panel_workspace_id(panel_id);
        } else {
            board.focused = board.panels.first().map(|panel| panel.id);
        }

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

    /// Create a workspace at a specific canvas position.
    #[must_use]
    pub fn create_workspace_at(&mut self, name: &str, position: [f32; 2]) -> WorkspaceId {
        let id = self.create_workspace(name);
        if let Some(ws) = self.workspace_mut(id) {
            ws.position = position;
        }
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
        let name = format!("Workspace {}", self.workspaces.len() + 1);
        self.create_workspace(&name)
    }

    /// Create a panel inside a workspace.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying PTY-backed panel cannot be spawned.
    pub fn create_panel(&mut self, mut opts: PanelOptions, workspace: WorkspaceId) -> Result<PanelId> {
        let id = PanelId(self.next_panel_id);
        self.next_panel_id += 1;

        // Inherit workspace cwd if the panel doesn't specify one.
        if opts.cwd.is_none()
            && let Some(ws) = self.workspace(workspace)
        {
            opts.cwd.clone_from(&ws.cwd);
        }

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
        let removed_panel = self
            .panels
            .iter()
            .position(|panel| panel.id == id)
            .map(|index| self.panels.remove(index));
        let ws_id = removed_panel.as_ref().map(|panel| panel.workspace_id);
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

        // Remove workspace if it has no panels left.
        if let Some(ws_id) = ws_id {
            let is_empty = self.workspaces.iter().any(|ws| ws.id == ws_id && ws.panels.is_empty());
            if is_empty {
                self.workspaces.retain(|ws| ws.id != ws_id);
                self.attention.retain(|item| item.workspace_id != ws_id);
                if self.active_workspace == Some(ws_id) {
                    self.active_workspace = self.workspaces.first().map(|ws| ws.id);
                }
            }
        }

        if let Some(mut panel) = removed_panel
            && panel.kind.is_agent()
            && !panel.shutdown_with_timeout(AGENT_PANEL_SHUTDOWN_TIMEOUT)
        {
            tracing::warn!(
                panel_id = panel.id.0,
                kind = ?panel.kind,
                timeout_ms = AGENT_PANEL_SHUTDOWN_TIMEOUT.as_millis(),
                "timed out waiting for agent panel shutdown"
            );
        }
    }

    /// Restart a panel's terminal process in-place, preserving identity and
    /// session binding.
    ///
    /// # Errors
    ///
    /// Returns an error if the new terminal cannot be spawned.
    pub fn restart_panel(&mut self, id: PanelId) -> Result<()> {
        let panel = self
            .panels
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| crate::error::Error::Pty(format!("panel {} not found", id.0)))?;
        panel.restart()
    }

    pub fn shutdown_agent_panels(&mut self) {
        for panel in &mut self.panels {
            if panel.kind.is_agent() {
                panel.request_shutdown();
            }
        }

        for panel in &mut self.panels {
            if !panel.kind.is_agent() {
                continue;
            }
            if !panel.wait_for_shutdown(AGENT_PANEL_SHUTDOWN_TIMEOUT) {
                tracing::warn!(
                    panel_id = panel.id.0,
                    kind = ?panel.kind,
                    timeout_ms = AGENT_PANEL_SHUTDOWN_TIMEOUT.as_millis(),
                    "timed out waiting for agent panel shutdown"
                );
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

    #[must_use]
    pub fn rename_panel(&mut self, id: PanelId, name: &str) -> bool {
        self.panel_mut(id).is_some_and(|panel| panel.rename(name))
    }

    pub fn sync_workspace_metadata(&mut self, config: &Config) {
        for workspace in &mut self.workspaces {
            if let Some(template) = &workspace.template
                && let Some(workspace_config) = config.workspaces.get(template.workspace_index)
            {
                if workspace.name != workspace_config.name {
                    workspace_config.name.clone_into(&mut workspace.name);
                }
                workspace.cwd = workspace_config.cwd.as_deref().map(Config::expand_tilde);
            }
        }
    }

    /// Drain pending output from all panels. Returns `true` if any panel had activity.
    pub fn process_output(&mut self) -> bool {
        let mut had_output = false;
        for panel in &mut self.panels {
            had_output |= panel.process_output();
        }
        self.update_attention();
        had_output
    }

    /// Remove workspaces that have no panels.
    pub fn remove_empty_workspaces(&mut self) {
        let empty_ids: Vec<_> = self
            .workspaces
            .iter()
            .filter(|ws| ws.panels.is_empty())
            .map(|ws| ws.id)
            .collect();
        for ws_id in empty_ids {
            self.workspaces.retain(|ws| ws.id != ws_id);
            self.attention.retain(|item| item.workspace_id != ws_id);
            if self.active_workspace == Some(ws_id) {
                self.active_workspace = self.workspaces.first().map(|ws| ws.id);
            }
        }
    }

    /// Returns IDs of panels whose child process has exited.
    #[must_use]
    pub fn exited_panels(&self) -> Vec<PanelId> {
        self.panels
            .iter()
            .filter(|panel| panel.child_exited())
            .map(|panel| panel.id)
            .collect()
    }

    fn update_attention(&mut self) {
        use crate::attention::AttentionSeverity;

        let panel_states: Vec<_> = self
            .panels
            .iter_mut()
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

            let has_open = self.unresolved_attention_for_panel(panel_id).is_some();

            if let Some(summary) = attention {
                if !has_open {
                    self.create_attention(workspace_id, Some(panel_id), "agent", summary, AttentionSeverity::High);
                }
            } else if bell && kind.is_agent() {
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
            } else if has_open {
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

    #[must_use]
    pub fn workspace_id_by_local_id(&self, local_id: &str) -> Option<WorkspaceId> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.local_id == local_id)
            .map(|workspace| workspace.id)
    }

    #[must_use]
    pub fn panel_id_by_local_id(&self, local_id: &str) -> Option<PanelId> {
        self.panels
            .iter()
            .find(|panel| panel.local_id == local_id)
            .map(|panel| panel.id)
    }

    /// Computes the bounding rectangle of all panels in a workspace.
    /// Returns `(min, max)` in canvas coordinates, or `None` when the
    /// workspace is empty or does not exist.
    #[must_use]
    pub fn workspace_bounds(&self, id: WorkspaceId) -> Option<([f32; 2], [f32; 2])> {
        let workspace = self.workspace(id)?;
        let mut panels = workspace.panels.iter().filter_map(|pid| self.panel(*pid)).peekable();
        panels.peek()?;

        // Anchor min to the workspace origin so the frame doesn't chase
        // panels when they are dragged past the workspace position.
        let origin = workspace.position;
        let mut min = [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD];
        let mut max = [f32::MIN, f32::MIN];
        for panel in panels {
            let chrome_w = panel.layout.size[0] + 2.0 * PANEL_CHROME_PAD;
            let chrome_h = panel.layout.size[1] + PANEL_CHROME_TITLEBAR + 2.0 * PANEL_CHROME_PAD;
            min[0] = min[0].min(panel.layout.position[0]);
            min[1] = min[1].min(panel.layout.position[1]);
            max[0] = max[0].max(panel.layout.position[0] + chrome_w);
            max[1] = max[1].max(panel.layout.position[1] + chrome_h);
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

    /// Translate a workspace and push any colliding workspaces further along
    /// the drag direction, cascading through the chain of collisions.
    pub fn translate_workspace_with_push(&mut self, id: WorkspaceId, delta: [f32; 2]) -> bool {
        if !self.translate_workspace(id, delta) {
            return false;
        }
        self.resolve_workspace_collisions(id, delta);
        true
    }

    /// Returns the visual frame rect `[min_x, min_y, max_x, max_y]` for a
    /// workspace, including the title area and background padding.
    fn workspace_frame_rect(&self, id: WorkspaceId) -> Option<[f32; 4]> {
        let workspace = self.workspace(id)?;
        if let Some((min, max)) = self.workspace_bounds(id) {
            Some([
                min[0] - WS_FRAME_PAD,
                min[1] - WS_FRAME_PAD - WS_FRAME_TOP_EXTRA,
                max[0] + WS_FRAME_PAD,
                max[1] + WS_FRAME_PAD,
            ])
        } else {
            let p = workspace.position;
            Some([p[0], p[1], p[0] + WS_EMPTY_FRAME_SIZE[0], p[1] + WS_EMPTY_FRAME_SIZE[1]])
        }
    }

    /// After the workspace `source` was moved, push every overlapping
    /// workspace along `drag_dir`, cascading until nothing overlaps.
    fn resolve_workspace_collisions(&mut self, source: WorkspaceId, drag_dir: [f32; 2]) {
        let mut queue = vec![source];
        let mut settled = vec![source];

        while let Some(check_id) = queue.pop() {
            let Some(check_rect) = self.workspace_frame_rect(check_id) else {
                continue;
            };

            let candidates: Vec<WorkspaceId> = self
                .workspaces
                .iter()
                .map(|ws| ws.id)
                .filter(|id| !settled.contains(id))
                .collect();

            for other_id in candidates {
                let Some(other_rect) = self.workspace_frame_rect(other_id) else {
                    continue;
                };
                let push = collision_push(check_rect, other_rect, drag_dir, WS_COLLISION_GAP);
                if push[0] != 0.0 || push[1] != 0.0 {
                    self.translate_workspace(other_id, push);
                    settled.push(other_id);
                    queue.push(other_id);
                }
            }
        }
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

    /// Arrange all panels in a workspace according to a predefined layout.
    /// Panels are equally sized and positioned with gaps.
    pub fn arrange_workspace(&mut self, id: WorkspaceId, layout: WorkspaceLayout) {
        let Some(workspace) = self.workspace(id) else {
            return;
        };
        let panel_ids: Vec<PanelId> = workspace.panels.clone();
        let origin = workspace.position;
        let count = panel_ids.len();
        if count == 0 {
            return;
        }

        for (index, panel_id) in panel_ids.iter().enumerate() {
            let (position, size) = arranged_panel_layout(origin, layout, index, count);

            if let Some(panel) = self.panel_mut(*panel_id) {
                panel.move_to(position);
                panel.resize_layout(size);
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

    fn create_workspace_record(&mut self, workspace_state: &WorkspaceState) -> WorkspaceId {
        let id = self.create_workspace(&workspace_state.name);
        if let Some(workspace) = self.workspace_mut(id) {
            workspace.local_id.clone_from(&workspace_state.local_id);
            workspace.position = workspace_state.position.unwrap_or(workspace.position);
            workspace.cwd = workspace_state.cwd.as_deref().map(Config::expand_tilde);
            workspace.template.clone_from(&workspace_state.template);
        }
        id
    }
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

fn arranged_panel_layout(
    origin: [f32; 2],
    layout: WorkspaceLayout,
    index: usize,
    count: usize,
) -> ([f32; 2], [f32; 2]) {
    let panel_size = DEFAULT_PANEL_SIZE;

    match layout {
        WorkspaceLayout::Rows => {
            let x = origin[0] + WS_INNER_PAD;
            let y = origin[1] + WS_INNER_PAD + usize_to_f32(index) * (panel_size[1] + TILE_GAP);
            ([x, y], panel_size)
        }
        WorkspaceLayout::Columns => {
            let x = origin[0] + WS_INNER_PAD + usize_to_f32(index) * (panel_size[0] + TILE_GAP);
            let y = origin[1] + WS_INNER_PAD;
            ([x, y], panel_size)
        }
        WorkspaceLayout::Grid => {
            let cols = ceil_sqrt_usize(count);
            let col = index % cols;
            let row = index / cols;
            let x = origin[0] + WS_INNER_PAD + usize_to_f32(col) * (panel_size[0] + TILE_GAP);
            let y = origin[1] + WS_INNER_PAD + usize_to_f32(row) * (panel_size[1] + TILE_GAP);
            ([x, y], panel_size)
        }
        WorkspaceLayout::Stack => {
            let x = origin[0] + WS_INNER_PAD + usize_to_f32(index) * STACK_OFFSET_X;
            let y = origin[1] + WS_INNER_PAD + usize_to_f32(index) * STACK_OFFSET_Y;
            ([x, y], panel_size)
        }
        WorkspaceLayout::Cascade => {
            let x = origin[0] + WS_INNER_PAD + usize_to_f32(index) * CASCADE_OFFSET_X;
            let y = origin[1] + WS_INNER_PAD + usize_to_f32(index) * CASCADE_OFFSET_Y;
            ([x, y], panel_size)
        }
    }
}

fn position_occupied(positions: &[[f32; 2]], candidate: [f32; 2]) -> bool {
    positions
        .iter()
        .any(|pos| (pos[0] - candidate[0]).abs() < 1.0 && (pos[1] - candidate[1]).abs() < 1.0)
}

/// Compute the translation needed to push rect `b` away from rect `a` along
/// `drag_dir` so they no longer overlap, maintaining `gap` pixels of space.
/// Both rects are `[min_x, min_y, max_x, max_y]`.
fn collision_push(a: [f32; 4], b: [f32; 4], drag_dir: [f32; 2], gap: f32) -> [f32; 2] {
    // No overlap → no push.
    if a[2] <= b[0] || b[2] <= a[0] || a[3] <= b[1] || b[3] <= a[1] {
        return [0.0, 0.0];
    }

    let len_sq = drag_dir[0] * drag_dir[0] + drag_dir[1] * drag_dir[1];
    if len_sq < 1e-6 {
        return [0.0, 0.0];
    }
    let len = len_sq.sqrt();
    let dx = drag_dir[0] / len;
    let dy = drag_dir[1] / len;

    // For each axis where the drag has a non-zero component, compute the
    // scalar `t` along the direction vector that would separate the rects
    // on that axis.  The minimum such `t` is sufficient because clearing
    // even one axis eliminates the AABB overlap.
    let mut min_t = f32::MAX;

    if dx > 1e-4 {
        let t = (a[2] + gap - b[0]) / dx;
        if t > 0.0 {
            min_t = min_t.min(t);
        }
    } else if dx < -1e-4 {
        let t = (a[0] - gap - b[2]) / dx;
        if t > 0.0 {
            min_t = min_t.min(t);
        }
    }

    if dy > 1e-4 {
        let t = (a[3] + gap - b[1]) / dy;
        if t > 0.0 {
            min_t = min_t.min(t);
        }
    } else if dy < -1e-4 {
        let t = (a[1] - gap - b[3]) / dy;
        if t > 0.0 {
            min_t = min_t.min(t);
        }
    }

    if min_t < f32::MAX {
        [dx * min_t, dy * min_t]
    } else {
        [0.0, 0.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkspaceConfig;
    use crate::runtime_state::WorkspaceTemplateRef;
    use std::path::PathBuf;

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
    fn rename_panel_updates_matching_panel() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");
        let panel_id = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("panel should spawn");

        assert!(board.rename_panel(panel_id, "backend shell"));
        assert_eq!(
            board.panel(panel_id).expect("panel should exist").title,
            "backend shell"
        );
    }

    #[test]
    fn rename_panel_rejects_blank_names() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");
        let panel_id = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("panel should spawn");
        let original_title = board.panel(panel_id).expect("panel should exist").title.clone();

        assert!(!board.rename_panel(panel_id, "   "));
        assert_eq!(board.panel(panel_id).expect("panel should exist").title, original_title);
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

    #[test]
    fn sync_workspace_metadata_updates_only_templated_workspaces() {
        let mut board = Board::new();
        let templated_workspace = board.create_workspace("stale");
        let manual_workspace = board.create_workspace("manual");

        {
            let workspace = board.workspace_mut(templated_workspace).expect("templated workspace");
            workspace.template = Some(WorkspaceTemplateRef {
                workspace_index: 0,
                workspace_name: "template".to_string(),
            });
            workspace.cwd = Some(PathBuf::from("/tmp/old"));
        }
        {
            let workspace = board.workspace_mut(manual_workspace).expect("manual workspace");
            workspace.cwd = Some(PathBuf::from("/tmp/manual"));
        }

        let config = Config {
            workspaces: vec![WorkspaceConfig {
                name: "synced".to_string(),
                color: None,
                cwd: Some("~/repo".to_string()),
                position: None,
                terminals: Vec::new(),
            }],
            ..Config::default()
        };

        board.sync_workspace_metadata(&config);

        let templated = board.workspace(templated_workspace).expect("templated workspace");
        assert_eq!(templated.name, "synced");
        assert_eq!(templated.cwd, Some(Config::expand_tilde("~/repo")));

        let manual = board.workspace(manual_workspace).expect("manual workspace");
        assert_eq!(manual.name, "manual");
        assert_eq!(manual.cwd, Some(PathBuf::from("/tmp/manual")));
    }

    /// Regression: when adding a panel with an explicit position (e.g. from a
    /// click on a panned canvas), the panel must be placed at that position
    /// instead of falling back to the default tiled slot.
    #[test]
    fn explicit_position_overrides_default_tiling() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("test");

        // Create a first panel so the workspace is non-empty.
        board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("first panel should spawn");

        // Add a second panel with an explicit canvas position far from the
        // workspace origin — simulating a Ctrl+double-click on a panned canvas.
        let click_pos = [800.0, 600.0];
        let panel_id = board
            .create_panel(
                PanelOptions {
                    position: Some(click_pos),
                    ..PanelOptions::default()
                },
                workspace_id,
            )
            .expect("second panel should spawn");

        let panel = board.panel(panel_id).expect("panel should exist");
        assert!(
            (panel.layout.position[0] - click_pos[0]).abs() <= f32::EPSILON
                && (panel.layout.position[1] - click_pos[1]).abs() <= f32::EPSILON,
            "panel should be placed at the explicit click position ({:?}), not at the default tile position ({:?})",
            click_pos,
            panel.layout.position,
        );
    }

    #[test]
    fn arrange_workspace_stack_offsets_panels_in_layers() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("stacked");
        let origin = board.workspace(workspace_id).expect("workspace").position;

        let first = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("first panel should spawn");
        let second = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("second panel should spawn");

        board.arrange_workspace(workspace_id, WorkspaceLayout::Stack);

        let first_position = board.panel(first).expect("first panel").layout.position;
        let second_position = board.panel(second).expect("second panel").layout.position;

        assert_eq!(first_position, [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]);
        assert_eq!(
            second_position,
            [
                origin[0] + WS_INNER_PAD + STACK_OFFSET_X,
                origin[1] + WS_INNER_PAD + STACK_OFFSET_Y,
            ]
        );
    }

    #[test]
    fn arrange_workspace_cascade_offsets_panels_diagonally() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("cascade");
        let origin = board.workspace(workspace_id).expect("workspace").position;

        let first = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("first panel should spawn");
        let second = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("second panel should spawn");

        board.arrange_workspace(workspace_id, WorkspaceLayout::Cascade);

        let first_position = board.panel(first).expect("first panel").layout.position;
        let second_position = board.panel(second).expect("second panel").layout.position;

        assert_eq!(first_position, [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]);
        assert_eq!(
            second_position,
            [
                origin[0] + WS_INNER_PAD + CASCADE_OFFSET_X,
                origin[1] + WS_INNER_PAD + CASCADE_OFFSET_Y,
            ]
        );
    }
}
