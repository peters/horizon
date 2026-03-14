use crate::panel::PanelId;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WorkspaceId(pub u64);

/// A visual cluster ("cloud") of terminal panels on the canvas.
/// Workspaces are always visible — no tabs, no hidden state.
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub color_idx: usize,
    pub panels: Vec<PanelId>,
    pub collapsed: bool,
    /// Canvas position (top-left) of the cloud container.
    pub position: [f32; 2],
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
    pub fn new(id: WorkspaceId, name: String, color_idx: usize) -> Self {
        // Auto-tile: offset each workspace so they don't stack
        let col = color_idx % 3;
        let row = color_idx / 3;
        let x = 40.0 + col as f32 * 720.0;
        let y = 80.0 + row as f32 * 500.0;

        Self {
            id,
            name,
            color_idx,
            panels: Vec::new(),
            collapsed: false,
            position: [x, y],
        }
    }

    pub fn accent(&self) -> (u8, u8, u8) {
        WORKSPACE_COLORS[self.color_idx % WORKSPACE_COLORS.len()]
    }

    pub fn add_panel(&mut self, panel_id: PanelId) {
        if !self.panels.contains(&panel_id) {
            self.panels.push(panel_id);
        }
    }

    pub fn remove_panel(&mut self, panel_id: PanelId) {
        self.panels.retain(|&id| id != panel_id);
    }
}
