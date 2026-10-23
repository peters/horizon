//! The setup board panel: the app fills its rows from the setup runs each frame and
//! carries out what it asks for.
use super::run::Action;
use crate::app::HorizonApp;
use horizon_core::setup_board::{Request, Row};
use horizon_core::{PanelId, PanelKind, PanelOptions};

/// The size of a new setup board.
const BOARD_SIZE: [f32; 2] = [760.0, 620.0];

impl HorizonApp {
    /// The board panels there are now.
    fn setup_boards(&self) -> Vec<PanelId> {
        self.board
            .panels
            .iter()
            .filter(|panel| panel.kind == PanelKind::SetupBoard)
            .map(|panel| panel.id)
            .collect()
    }

    /// Makes a setup board in the workspace of the setup agents when there is none, and
    /// shows it when `reveal`.
    pub(in crate::app) fn open_setup_board(&mut self, ctx: &egui::Context, reveal: bool) {
        let existing = self.setup_boards().first().copied();
        let board = if let Some(board) = existing {
            board
        } else {
            let workspace = self.board.ensure_local_workspace(super::session::WORKSPACE);
            let created = self.board.create_panel(
                PanelOptions {
                    kind: PanelKind::SetupBoard,
                    size: Some(BOARD_SIZE),
                    ..PanelOptions::default()
                },
                workspace,
            );
            match created {
                Ok(board) => {
                    self.mark_runtime_dirty();
                    board
                }
                Err(error) => {
                    self.cloud_prototype.error = Some(error.to_string());
                    return;
                }
            }
        };
        self.sync_setup_boards();
        if reveal && let Some(workspace) = self.board.panel(board).map(|panel| panel.workspace_id) {
            self.reveal_new_panel(ctx, workspace, board);
        }
    }

    /// Gives every board the rows of the setup runs as they are now.
    pub(super) fn sync_setup_boards(&mut self) {
        let boards = self.setup_boards();
        if boards.is_empty() {
            return;
        }
        let rows: Vec<Row> = self
            .cloud_prototype
            .production
            .setup_runs
            .iter()
            .map(super::run::Run::board_row)
            .collect();
        let builder = self.cloud_prototype.production.setup_builder.line();
        for board in boards {
            if let Some(content) = self
                .board
                .panel_mut(board)
                .and_then(|panel| panel.content.setup_board_mut())
            {
                if content.rows != rows {
                    content.rows.clone_from(&rows);
                }
                content.builder.clone_from(&builder);
            }
        }
    }

    /// Carries out what setup boards asked for while they were drawn.
    pub(in crate::app) fn apply_setup_board_requests(&mut self, ctx: &egui::Context) {
        let requests: Vec<Request> = self
            .board
            .panels
            .iter_mut()
            .filter_map(|panel| panel.content.setup_board_mut()?.request.take())
            .collect();
        for request in requests {
            match request {
                Request::OpenAgent(run) => self.cloud_repository_setup_action(ctx, run, Action::Show),
                Request::Stop(run) => self.cloud_repository_setup_action(ctx, run, Action::Stop),
                Request::OpenInNewCloud(run) => self.open_cloud_for_setup(ctx, Some(run)),
                Request::NewSetup => self.open_cloud_for_setup(ctx, None),
            }
        }
        self.sync_setup_boards();
    }

    /// Opens New cloud in a new workspace, as **New cloud** does, for the repository of
    /// `run`: the dialog then shows that setup. Without a run it asks for a repository.
    fn open_cloud_for_setup(&mut self, ctx: &egui::Context, run: Option<PanelId>) {
        let entered = run
            .and_then(|run| self.cloud_prototype.production.setup_run(run))
            .map(|run| run.entered.clone());
        if run.is_some() && entered.is_none() {
            return;
        }
        // Before the cloud state loaded New cloud does not open, and an empty workspace
        // would stay; the button works again a moment later.
        if !self.cloud_launch_ready() {
            return;
        }
        self.create_new_workspace(ctx, crate::app::sidebar::NewWorkspace::Cloud, None);
        if let Some(entered) = entered
            && self.cloud_prototype.production.creating
        {
            self.cloud_prototype.production.repository = entered;
            self.read_cloud_profiles(ctx);
        }
    }
}

// The setup agent is offered on Linux only.
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use horizon_core::setup_board::Request;
    use horizon_core::{PanelKind, PresetConfig};
    use std::time::{Duration, Instant};

    #[test]
    fn a_setup_opens_the_board_and_its_stop_ends_the_run() {
        let (temp, ctx, mut app) =
            crate::app::test_support::test_app_with_startup(horizon_core::StartupDecision::Ephemeral {
                runtime_state: Box::default(),
            });
        // A non-process panel stands in for the agent, so no real login starts in a test.
        app.cloud_prototype.production.setup_agent = Some(PanelKind::Usage);
        app.cloud_prototype.production.setup_presets = vec![PresetConfig {
            name: "Test panel".into(),
            kind: PanelKind::Usage,
            alias: None,
            command: None,
            args: Vec::new(),
            resume: horizon_core::PanelResume::default(),
            ssh_connection: None,
        }];
        let folder = temp.path().join("web-api");
        std::fs::create_dir_all(&folder).unwrap();
        app.cloud_prototype.production.repository = folder.display().to_string();
        app.start_cloud_repository_setup(&ctx);
        let started = Instant::now();
        while !app.cloud_prototype.production.setup_starting.is_empty() {
            assert!(started.elapsed() < Duration::from_secs(20), "the baseline was read");
            std::thread::sleep(Duration::from_millis(20));
            app.sync_cloud_repository_setup(&ctx);
        }
        let boards = app.setup_boards();
        assert_eq!(boards.len(), 1, "the first setup opens the board");
        let board = boards[0];
        let run = app.cloud_prototype.production.setup_runs[0].panel;
        let panel = app.board.panel(board).unwrap();
        assert_eq!(
            Some(panel.workspace_id),
            app.board.panel(run).map(|agent| agent.workspace_id),
            "the board sits beside the setup agents"
        );
        let rows = &panel.content.setup_board().unwrap().rows;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].run, run);
        assert_eq!(rows[0].name, "web-api");
        // A second setup adds a row, not a second board.
        app.open_setup_board(&ctx, false);
        assert_eq!(app.setup_boards(), vec![board]);
        app.board
            .panel_mut(board)
            .and_then(|panel| panel.content.setup_board_mut())
            .unwrap()
            .request = Some(Request::Stop(run));
        app.apply_setup_board_requests(&ctx);
        assert!(
            app.cloud_prototype.production.setup_runs.is_empty(),
            "Stop ended the run"
        );
        assert!(app.board.panel(run).is_none(), "its agent's panel closed");
        let rows = &app.board.panel(board).unwrap().content.setup_board().unwrap().rows;
        assert!(rows.is_empty(), "the board shows the runs there are");
    }

    #[test]
    fn a_stopped_setup_can_be_taken_off_the_board() {
        let (temp, ctx, mut app) =
            crate::app::test_support::test_app_with_startup(horizon_core::StartupDecision::Ephemeral {
                runtime_state: Box::default(),
            });
        app.cloud_prototype.production.setup_agent = Some(PanelKind::Usage);
        app.cloud_prototype.production.setup_presets = vec![PresetConfig {
            name: "Test panel".into(),
            kind: PanelKind::Usage,
            alias: None,
            command: None,
            args: Vec::new(),
            resume: horizon_core::PanelResume::default(),
            ssh_connection: None,
        }];
        app.cloud_prototype.production.repository = temp.path().display().to_string();
        app.start_cloud_repository_setup(&ctx);
        let started = Instant::now();
        while !app.cloud_prototype.production.setup_starting.is_empty() {
            assert!(started.elapsed() < Duration::from_secs(20), "the baseline was read");
            std::thread::sleep(Duration::from_millis(20));
            app.sync_cloud_repository_setup(&ctx);
        }
        let run = app.cloud_prototype.production.setup_runs[0].panel;
        let board = app.setup_boards()[0];
        // The person closes the agent's panel before the setup is ready.
        app.close_panel(run);
        app.sync_cloud_repository_setup(&ctx);
        let row = app.board.panel(board).unwrap().content.setup_board().unwrap().rows[0].clone();
        assert_eq!(row.state, horizon_core::setup_board::RowState::Stopped);
        assert!(!row.committing);
        app.board
            .panel_mut(board)
            .and_then(|panel| panel.content.setup_board_mut())
            .unwrap()
            .request = Some(Request::Stop(run));
        app.apply_setup_board_requests(&ctx);
        assert!(
            app.cloud_prototype.production.setup_runs.is_empty(),
            "Remove took it off"
        );
    }
}
