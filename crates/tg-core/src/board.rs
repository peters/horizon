use crate::config::Config;
use crate::error::Result;
use crate::panel::{Panel, PanelId, PanelOptions};
use crate::workspace::{Workspace, WorkspaceId};

pub struct Board {
    pub panels: Vec<Panel>,
    pub workspaces: Vec<Workspace>,
    pub focused: Option<PanelId>,
    next_panel_id: u64,
    next_workspace_id: u64,
}

impl Board {
    pub fn new() -> Self {
        Self {
            panels: Vec::new(),
            workspaces: Vec::new(),
            focused: None,
            next_panel_id: 1,
            next_workspace_id: 1,
        }
    }

    /// Build a board from a YAML config.
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

    pub fn create_workspace(&mut self, name: &str) -> WorkspaceId {
        let id = WorkspaceId(self.next_workspace_id);
        self.next_workspace_id += 1;
        let color_idx = self.workspaces.len();
        self.workspaces.push(Workspace::new(id, name.to_string(), color_idx));
        tracing::info!("created workspace '{}' ({})", name, id.0);
        id
    }

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
        if self.focused == Some(id) {
            self.focused = self.panels.last().map(|p| p.id);
        }
    }

    pub fn remove_workspace(&mut self, id: WorkspaceId) {
        self.workspaces.retain(|w| w.id != id);
    }

    pub fn process_output(&mut self) {
        for panel in &mut self.panels {
            panel.process_output();
        }
    }

    pub fn focus(&mut self, id: PanelId) {
        self.focused = Some(id);
    }
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}
