//! The rows of the sidebar's workspace list. A row is read from the panels of its
//! workspace, so the rows are read again at most once a second, or at once when
//! workspaces come or go or a state of attention changes.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use horizon_core::Attention;
use horizon_core::WorkspaceId;
use horizon_core::workspace_list::{Row, Waiting};

use super::super::HorizonApp;
use super::{SidebarActions, SidebarWorkspaceDragState, WorkspaceSidebarEntry};

const REFRESH: Duration = Duration::from_secs(1);

/// The rows of the last read, by workspace.
#[derive(Default)]
pub(in crate::app) struct ListCache {
    refreshed: Option<Instant>,
    /// The id, persistent local id and machine of each workspace: ids restart at
    /// 1 in a replaced session, so the id alone does not tell the workspaces
    /// apart, and a workspace that moves to a machine says so on the next frame.
    workspaces: Vec<(WorkspaceId, String, Option<String>)>,
    /// The version of the attention states that the rows came from, so a changed
    /// badge shows on the frame that changed it and not a second later.
    attention_version: u64,
    rows: HashMap<WorkspaceId, Row>,
}

impl HorizonApp {
    /// Reads the rows again when they are a second old or the workspaces changed.
    pub(in crate::app) fn refresh_sidebar_rows(&mut self, now: Instant) {
        let cache = &self.sidebar_list;
        let unchanged = cache
            .workspaces
            .iter()
            .map(|(id, local, machine)| (*id, local.as_str(), machine.as_deref()))
            .eq(self
                .board
                .workspaces
                .iter()
                .map(|workspace| (workspace.id, workspace.local_id.as_str(), workspace.place.machine())));
        let attention_version = self.attention.version();
        if unchanged
            && cache.attention_version == attention_version
            && cache.refreshed.is_some_and(|at| now.duration_since(at) < REFRESH)
        {
            return;
        }
        self.sidebar_list = ListCache {
            refreshed: Some(now),
            attention_version,
            workspaces: self
                .board
                .workspaces
                .iter()
                .map(|workspace| {
                    (
                        workspace.id,
                        workspace.local_id.clone(),
                        workspace.place.machine().map(str::to_owned),
                    )
                })
                .collect(),
            rows: self.read_sidebar_rows(),
        };
    }

    /// The row of each workspace now: the heaviest attention state of its
    /// panels, how many of them wait for the person, and its machine.
    pub(in crate::app) fn read_sidebar_rows(&self) -> HashMap<WorkspaceId, Row> {
        self.board
            .workspaces
            .iter()
            .map(|workspace| {
                let panels = workspace.panels.iter().filter_map(|id| self.board.panel(*id));
                let attention = Attention::heaviest(
                    panels
                        .clone()
                        .filter(|panel| panel.kind.is_agent())
                        .map(|panel| self.panel_attention_of(panel)),
                );
                let waiting = sufficient_waiting(
                    panels
                        .clone()
                        .filter(|panel| panel.kind.is_agent())
                        .map(|panel| self.panel_attention_of(panel)),
                );
                (workspace.id, Row::placed(attention, waiting, workspace.place.machine()))
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
            .unwrap_or_else(|| Row::local(Attention::Idle))
    }
}

/// How many agent panels of one workspace wait for the person.
fn sufficient_waiting(states: impl Iterator<Item = Attention>) -> Waiting {
    let mut waiting = Waiting::default();
    for state in states {
        match state {
            Attention::NeedsYou => waiting.needs_you += 1,
            Attention::Done => waiting.done += 1,
            Attention::Idle | Attention::Working | Attention::Background => {}
        }
    }
    waiting
}

/// Whether a workspace drops on the row `target`: on any docked row. A detached
/// workspace has its own window, so it takes no drop.
pub(super) fn accepts_drop(target: &WorkspaceSidebarEntry) -> bool {
    super::sidebar_workspace_drop_should_dock(target.detached)
}

#[cfg(test)]
mod tests {
    use horizon_core::WorkspacePlace;

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

    #[test]
    fn a_changed_attention_state_reads_the_rows_again_at_once() {
        use horizon_core::Attention;

        let (_temp, mut app) = crate::app::test_support::test_app();
        let workspace = app.board.create_workspace("a");
        let panel = app
            .board
            .create_panel(
                horizon_core::PanelOptions {
                    name: Some("Fix the sync".to_owned()),
                    kind: horizon_core::PanelKind::Claude,
                    command: Some("seed".to_owned()),
                    ..horizon_core::PanelOptions::default()
                },
                workspace,
            )
            .expect("panel");
        let local = app.board.panel(panel).expect("panel").local_id.clone();
        let start = std::time::Instant::now();
        app.refresh_sidebar_rows(start);
        assert_eq!(app.sidebar_row(workspace).attention, Attention::Idle);

        // A question opens in the ledger while the person looks elsewhere.
        app.board.focused = None;
        let mut ledger = horizon_core::agent_work::TurnLedger::default();
        ledger.session_id = "session".into();
        ledger.prompt_id = Some("prompt".into());
        ledger.state = horizon_core::agent_work::TurnState::Blocked;
        ledger.updated_at_millis = horizon_core::current_unix_millis() - 1;
        app.apply_attention_values(vec![(panel, local, Some(ledger))], &egui::Context::default());

        let soon = start + std::time::Duration::from_millis(100);
        app.refresh_sidebar_rows(soon);
        assert_eq!(
            app.sidebar_list.refreshed,
            Some(soon),
            "the row of a workspace whose badge changed is read again at once"
        );
        assert_eq!(app.sidebar_row(workspace).attention, Attention::NeedsYou);
        assert_eq!(app.sidebar_row(workspace).waiting.needs_you, 1);
    }

    #[test]
    fn the_place_of_a_workspace_is_kept_in_its_cached_row() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let workspace = app.board.create_workspace("Build");
        let here = app.board.create_workspace("Here");
        let start = std::time::Instant::now();
        app.refresh_sidebar_rows(start);

        assert_eq!(
            app.sidebar_row(workspace).line,
            "",
            "a row of this computer has no line yet"
        );
        assert_eq!(app.sidebar_row(here).line, "");

        // A workspace that moves to a machine reads its rows again at once,
        // because the machine is part of what the cache holds.
        app.board
            .set_workspace_place(workspace, WorkspacePlace::Machine("build-1".to_owned()));
        let soon = start + std::time::Duration::from_millis(100);
        app.refresh_sidebar_rows(soon);
        assert_eq!(app.sidebar_list.refreshed, Some(soon));
        assert_eq!(app.sidebar_row(workspace).line, "on build-1");
        assert_eq!(
            app.sidebar_row(here).line,
            "",
            "this computer says nothing about a place"
        );

        // The composed line is a row text like the others: no frame builds it
        // again while the row is a second old.
        app.refresh_sidebar_rows(soon + std::time::Duration::from_millis(100));
        assert_eq!(app.sidebar_list.refreshed, Some(soon), "the cache stays");
    }
}
