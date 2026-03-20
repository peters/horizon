use std::path::PathBuf;

use crate::board::WorkspaceLayout;
use crate::panel::PanelId;
use crate::runtime_state::{WorkspaceTemplateRef, new_local_id};
use crate::task::TaskWorkspaceBinding;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WorkspaceId(pub u64);

/// A visual workspace cluster of terminal panels on the canvas.
/// Workspaces are always visible — no tabs, no hidden state.
pub struct Workspace {
    pub id: WorkspaceId,
    pub local_id: String,
    pub name: String,
    pub color_idx: usize,
    pub panels: Vec<PanelId>,
    pub collapsed: bool,
    /// Canvas position (top-left) of the workspace badge.
    pub position: [f32; 2],
    /// Default working directory for new terminals in this workspace.
    pub cwd: Option<PathBuf>,
    pub template: Option<WorkspaceTemplateRef>,
    pub layout: Option<WorkspaceLayout>,
    pub task_binding: Option<TaskWorkspaceBinding>,
}

/// Predefined accent colors for workspace clusters.
pub const WORKSPACE_COLORS: &[(u8, u8, u8)] = &[
    (137, 180, 250), // blue
    (166, 227, 161), // green
    (249, 226, 175), // yellow
    (243, 139, 168), // red
    (245, 194, 231), // pink
    (148, 226, 213), // teal
    (203, 166, 247), // mauve
    (250, 179, 135), // peach
];

impl Workspace {
    #[must_use]
    pub fn new(id: WorkspaceId, name: String, color_idx: usize) -> Self {
        Self {
            id,
            local_id: new_local_id(),
            name,
            color_idx,
            panels: Vec::new(),
            collapsed: false,
            position: [0.0, 0.0],
            cwd: None,
            template: None,
            layout: None,
            task_binding: None,
        }
    }

    #[must_use]
    pub fn accent(&self) -> (u8, u8, u8) {
        WORKSPACE_COLORS[self.color_idx % WORKSPACE_COLORS.len()]
    }

    pub fn add_panel(&mut self, panel_id: PanelId) {
        if !self.panels.contains(&panel_id) {
            self.panels.push(panel_id);
        }
    }

    #[must_use]
    pub fn panel_index(&self, panel_id: PanelId) -> Option<usize> {
        self.panels.iter().position(|id| *id == panel_id)
    }

    pub fn remove_panel(&mut self, panel_id: PanelId) {
        self.panels.retain(|&id| id != panel_id);
    }
}
