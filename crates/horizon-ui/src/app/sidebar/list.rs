//! The rows of the sidebar's workspace list. A row reads its status line from a
//! terminal's screen, so the rows are read again at most once a second, or at once
//! when workspaces come or go.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use horizon_core::workspace_list::{self, Row};
use horizon_core::{AgentStatus, WorkspaceId};

use super::super::HorizonApp;
use super::{SidebarActions, SidebarWorkspaceDragState, WorkspaceSidebarEntry};

const REFRESH: Duration = Duration::from_secs(1);

/// The rows of the last read, by workspace.
#[derive(Default)]
pub(in crate::app) struct ListCache {
    refreshed: Option<Instant>,
    /// The id and persistent local id of each workspace: ids restart at 1 in a
    /// replaced session, so the id alone does not tell the workspaces apart.
    workspaces: Vec<(WorkspaceId, String)>,
    rows: HashMap<WorkspaceId, Row>,
}

impl HorizonApp {
    /// Reads the rows again when they are a second old or the workspaces changed.
    pub(in crate::app) fn refresh_sidebar_rows(&mut self, now: Instant) {
        let cache = &self.sidebar_list;
        let unchanged = cache.workspaces.iter().map(|(id, local)| (*id, local.as_str())).eq(self
            .board
            .workspaces
            .iter()
            .map(|workspace| (workspace.id, workspace.local_id.as_str())));
        if unchanged && cache.refreshed.is_some_and(|at| now.duration_since(at) < REFRESH) {
            return;
        }
        self.sidebar_list = ListCache {
            refreshed: Some(now),
            workspaces: self
                .board
                .workspaces
                .iter()
                .map(|workspace| (workspace.id, workspace.local_id.clone()))
                .collect(),
            rows: self.read_sidebar_rows(),
        };
    }

    /// The row of each workspace now.
    pub(in crate::app) fn read_sidebar_rows(&self) -> HashMap<WorkspaceId, Row> {
        self.board
            .workspaces
            .iter()
            .map(|workspace| {
                let panels = workspace.panels.iter().filter_map(|id| self.board.panel(*id));
                let working = panels.clone().any(|panel| panel.agent_status() == AgentStatus::Working);
                let line =
                    workspace_list::primary_panel(panels, self.board.focused).and_then(workspace_list::panel_line);
                (workspace.id, Row::local(working, line.as_deref()))
            })
            .collect()
    }

    /// The rows of the workspaces of the board, in board order.
    pub(super) fn render_sidebar_workspaces(
        &mut self,
        ui: &mut egui::Ui,
        workspace_data: &[WorkspaceSidebarEntry],
        actions: &mut SidebarActions,
        drag_state: &mut SidebarWorkspaceDragState,
    ) {
        for workspace in workspace_data {
            self.render_sidebar_workspace(ui, workspace, actions, drag_state);
        }
    }

    /// The row of `workspace` from the last read; a workspace read for the first
    /// time shows as idle until then.
    pub(super) fn sidebar_row(&self, workspace: WorkspaceId) -> Row {
        self.sidebar_list
            .rows
            .get(&workspace)
            .cloned()
            .unwrap_or_else(|| Row::local(false, None))
    }
}

/// Whether a workspace drops on the row `target`: on any docked row. A detached
/// workspace has its own window, so it takes no drop.
pub(super) fn accepts_drop(target: &WorkspaceSidebarEntry) -> bool {
    super::sidebar_workspace_drop_should_dock(target.detached)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_replaced_session_with_the_same_workspace_ids_reads_the_rows_again() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let _workspace = app.board.create_workspace("a");
        let start = std::time::Instant::now();
        app.refresh_sidebar_rows(start);
        let soon = start + std::time::Duration::from_millis(100);
        app.refresh_sidebar_rows(soon);
        assert_eq!(
            app.sidebar_list.refreshed,
            Some(start),
            "same workspaces: the cache stays"
        );

        // The workspaces of a replaced session get the same ids from 1.
        for workspace in &mut app.board.workspaces {
            workspace.local_id = format!("{}-other-session", workspace.local_id);
        }
        app.refresh_sidebar_rows(soon);
        assert_eq!(app.sidebar_list.refreshed, Some(soon));
    }
}
