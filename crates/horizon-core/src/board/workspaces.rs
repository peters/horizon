use crate::config::Config;
use crate::error::Result;
use crate::panel::{DEFAULT_PANEL_SIZE, Panel, PanelId, PanelOptions};
use crate::runtime_state::WorkspaceState;
use crate::workspace::{Workspace, WorkspaceId};

use super::Board;

impl Board {
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
        let workspace_layout = self.workspace_layout_value(workspace);

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
        if let Some(layout) = workspace_layout {
            self.apply_workspace_layout(workspace, layout);
        }

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
        self.panel_attention_signals.remove(&id);
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
            } else {
                self.reflow_workspace_layout(ws_id);
            }
        }

        // Send the shutdown request early so the event loop starts tearing
        // down the PTY while the `Terminal::Drop` impl detaches the thread.
        // This avoids blocking the UI thread — the child process is cleaned
        // up asynchronously in the background.
        if let Some(mut panel) = removed_panel
            && panel.kind.is_agent()
        {
            panel.request_shutdown();
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
        let Some(source_workspace_id) = self.panel_workspace_id(panel_id) else {
            return;
        };
        if source_workspace_id == workspace_id || self.workspace(workspace_id).is_none() {
            return;
        }

        for ws in &mut self.workspaces {
            ws.remove_panel(panel_id);
        }
        let target_layout = self.workspace_layout_value(workspace_id);

        if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == workspace_id) {
            ws.add_panel(panel_id);
        }
        if let Some(panel) = self.panel_mut(panel_id) {
            panel.workspace_id = workspace_id;
        }

        if let Some(layout) = target_layout {
            self.apply_workspace_layout(workspace_id, layout);
        } else {
            let new_position = self.default_panel_position(workspace_id);
            if let Some(panel) = self.panel_mut(panel_id) {
                panel.move_to(new_position);
            }
        }

        self.reflow_workspace_layout(source_workspace_id);
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

    pub(super) fn create_workspace_record(&mut self, workspace_state: &WorkspaceState) -> WorkspaceId {
        let id = self.create_workspace(&workspace_state.name);
        if let Some(workspace) = self.workspace_mut(id) {
            workspace.local_id.clone_from(&workspace_state.local_id);
            workspace.position = workspace_state.position.unwrap_or(workspace.position);
            workspace.cwd = workspace_state.cwd.as_deref().map(Config::expand_tilde);
            workspace.template.clone_from(&workspace_state.template);
            workspace.layout = workspace_state.layout;
        }
        id
    }
}
