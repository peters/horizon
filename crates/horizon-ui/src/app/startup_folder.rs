//! `horizon <folder>`: the board of one folder at the start.
//!
//! Horizon starts as it does without the argument, so the session of the last run
//! comes back, and then shows the workspace of the folder. An existing workspace of
//! that folder is shown and nothing else happens. A board without such a workspace
//! takes a new one, named after the folder, with one panel in it: the first agent
//! preset of the settings whose program this computer has, or a terminal in that
//! folder when no agent program is there.
//!
//! The search of a program reads the folders of `PATH`, so it runs on the
//! coordination worker and the panel opens when the answer arrives. A frame never
//! reads a folder of this computer for this start.

use std::path::PathBuf;

use egui::Context;
use horizon_core::folder_requests::{Answer, FolderRequest};
use horizon_core::setup::{FolderPanel, setup_agents};
use horizon_core::{PanelKind, PanelOptions, PresetConfig, WorkspaceId};

use super::HorizonApp;
use super::setup::installed_agent;

/// The folder that a frame shows, and the request of another process that named
/// it when one did.
pub(in crate::app) struct StartupFolder {
    pub(in crate::app) folder: PathBuf,
    /// The request that this start answers once the folder stands, or `None`
    /// when the person typed the folder on the command line.
    pub(in crate::app) request: Option<FolderRequest>,
}

impl HorizonApp {
    /// Shows the workspace of the folder of this start, with its one panel when the
    /// board had no workspace of that folder.
    ///
    /// This runs once, on the first frame that can show a board, so a start without
    /// a folder does nothing here.
    pub(in crate::app) fn apply_startup_folder(&mut self, ctx: &Context) {
        let Some(startup) = self.startup_folder.take() else {
            return;
        };
        let StartupFolder { folder, request } = startup;
        // A board that already holds the workspace of that folder shows it as it
        // stands: the folder adds no panel to a board that the person built.
        let made = self.workspace_of_folder(folder.as_path()).is_none();
        let workspace_id = self.ensure_workspace_of_folder(ctx, folder.as_path());
        if made {
            self.open_folder_panel(ctx, workspace_id, folder);
        }
        // The request of a second start names one folder and nothing else, and
        // this is the same work that the folder of a start does.
        if let Some(request) = request {
            let workspace = self
                .board
                .workspace(workspace_id)
                .map_or_else(String::new, |workspace| workspace.name.clone());
            let answer = if made {
                Answer::Made { workspace }
            } else {
                Answer::Shown { workspace }
            };
            self.answer_folder_request(request, answer);
        }
    }

    /// Opens the one panel of the workspace of a folder of this start.
    fn open_folder_panel(&mut self, ctx: &Context, workspace_id: WorkspaceId, folder: PathBuf) {
        let programs: Vec<String> = setup_agents(&self.presets)
            .into_iter()
            .map(|agent| agent.program)
            .collect();
        let presets = self.presets.clone();
        let opening = ctx.clone();
        self.browser_create_host.io.then(
            move || {
                programs
                    .iter()
                    .map(|program| horizon_core::setup::find_program(program, None).is_some())
                    .collect::<Vec<bool>>()
            },
            move |app, found| {
                let found = found.unwrap_or_default();
                let choice = installed_agent(&presets, &found);
                let mut options = folder_panel_options(&presets, choice, &folder, &app.template_config.browser);
                options.name = Some(super::session_pickup::folder_name(&folder));
                options.name_is_custom = Some(false);
                match app.create_panel_with_options(options, workspace_id) {
                    Ok(panel_id) => app.reveal_new_panel(&opening, workspace_id, panel_id),
                    Err(error) => tracing::error!("the panel of the folder of this start did not open: {error}"),
                }
            },
        );
    }
}

/// The options of the one panel of a workspace of a folder.
///
/// The panel runs in `folder`. A preset of the settings gives its program and its
/// arguments; a computer with no agent program starts a terminal there.
#[must_use]
pub(in crate::app) fn folder_panel_options(
    presets: &[PresetConfig],
    choice: FolderPanel,
    folder: &std::path::Path,
    browser_config: &horizon_core::browser::BrowserConfig,
) -> PanelOptions {
    let mut options = match choice {
        FolderPanel::Preset(index) => presets.get(index).map_or_else(terminal_options, |preset| {
            super::actions::support::preset_panel_options(preset, browser_config)
        }),
        FolderPanel::Terminal => terminal_options(),
    };
    options.cwd = Some(folder.to_path_buf());
    options
}

/// The options of a terminal of this computer.
fn terminal_options() -> PanelOptions {
    PanelOptions {
        kind: PanelKind::Shell,
        ..PanelOptions::default()
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::PanelKind;
    use horizon_core::PanelResume;
    use horizon_core::PresetConfig;
    use horizon_core::setup::FolderPanel;

    use super::folder_panel_options;

    fn preset(name: &str, kind: PanelKind) -> PresetConfig {
        PresetConfig {
            name: name.to_string(),
            alias: None,
            kind,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        }
    }

    #[test]
    fn a_preset_of_the_settings_gives_the_program_of_the_panel_of_a_folder() {
        let presets = vec![
            preset("Shell", PanelKind::Shell),
            preset("Claude Code", PanelKind::Claude),
        ];
        let folder = std::path::Path::new("/tmp/repo");

        let options = folder_panel_options(
            &presets,
            FolderPanel::Preset(1),
            folder,
            &horizon_core::browser::BrowserConfig::default(),
        );

        assert_eq!(options.kind, PanelKind::Claude);
        assert_eq!(options.cwd.as_deref(), Some(folder));
        assert_eq!(
            options.resume,
            PanelResume::Fresh,
            "a folder of this start continues no conversation: it starts the panel"
        );
    }

    #[test]
    fn a_computer_without_an_agent_program_starts_a_terminal_in_the_folder() {
        let presets = vec![preset("Claude Code", PanelKind::Claude)];
        let folder = std::path::Path::new("/tmp/repo");

        let options = folder_panel_options(
            &presets,
            FolderPanel::Terminal,
            folder,
            &horizon_core::browser::BrowserConfig::default(),
        );

        assert_eq!(options.kind, PanelKind::Shell);
        assert_eq!(options.cwd.as_deref(), Some(folder));
        assert!(options.command.is_none(), "the shell of this computer");
    }

    #[test]
    fn a_preset_index_that_the_settings_no_longer_hold_starts_a_terminal() {
        let folder = std::path::Path::new("/tmp/repo");

        let options = folder_panel_options(
            &[],
            FolderPanel::Preset(7),
            folder,
            &horizon_core::browser::BrowserConfig::default(),
        );

        assert_eq!(options.kind, PanelKind::Shell);
        assert_eq!(options.cwd.as_deref(), Some(folder));
    }
}

#[cfg(test)]
mod start_tests {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use egui::Context;

    use crate::app::HorizonApp;
    use crate::app::test_support::{run_app_frame, test_app_with_startup};

    use horizon_core::{PresetConfig, RuntimeState, StartupDecision, WorkspaceState};

    /// How long a test waits for the worker that searches the programs of the
    /// folder.
    ///
    /// The search reads the folders of `PATH` on the coordination worker, so the
    /// panel opens on a later frame. The bound is the hang guard of the test and
    /// not a rule of the product: a computer that runs this suite beside other
    /// work schedules that worker late, so a count of frames is no wait.
    const PATIENCE: Duration = Duration::from_secs(120);

    /// A board of the last run with one workspace of `name` in `folder` and one
    /// editor panel in it, so the workspace survives the start.
    fn saved_board(name: &str, folder: &Path) -> RuntimeState {
        RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: horizon_core::new_local_id(),
                name: name.to_string(),
                cwd: Some(folder.display().to_string()),
                position: Some([40.0, 40.0]),
                panels: vec![crate::app::test_support::editor_panel_state("notes", [60.0, 100.0])],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        }
    }

    fn running_app(startup: StartupDecision) -> (tempfile::TempDir, Context, HorizonApp) {
        test_app_with_startup(startup)
    }

    fn no_presets(app: &mut HorizonApp) {
        // The rule of a folder is judged with the answer of this test alone, so
        // every preset goes and a terminal stays.
        app.presets
            .retain(|preset: &PresetConfig| preset.kind == horizon_core::PanelKind::Shell);
    }

    #[test]
    fn a_folder_of_a_start_shows_the_workspace_of_the_last_run_and_adds_no_panel() {
        let temp = tempfile::tempdir().expect("temp dir");
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("the folder");
        let (_temp, ctx, mut app) = running_app(StartupDecision::Ephemeral {
            runtime_state: Box::new(saved_board("repo", &repo)),
        });
        run_app_frame(&ctx, &mut app);
        let panels_before = app.board.panels.len();

        app.startup_folder = Some(crate::app::startup_folder::StartupFolder {
            folder: repo.clone(),
            request: None,
        });
        run_app_frame(&ctx, &mut app);
        // The worker of the coordination thread answers one frame later.
        for _ in 0..40 {
            run_app_frame(&ctx, &mut app);
            if app.board.panels.len() > panels_before {
                break;
            }
        }

        assert_eq!(app.board.workspaces.len(), 1, "no second workspace of that folder");
        assert_eq!(app.board.panels.len(), panels_before, "the workspace adds no panel");
        let workspace = app.workspace_of_folder(&repo).expect("the workspace of the folder");
        assert_eq!(app.board.active_workspace, Some(workspace));
    }

    #[test]
    fn a_folder_of_a_start_makes_its_workspace_with_one_terminal_when_no_agent_is_there() {
        let temp = tempfile::tempdir().expect("temp dir");
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("the folder");
        let (_temp, ctx, mut app) = running_app(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        run_app_frame(&ctx, &mut app);
        no_presets(&mut app);

        app.startup_folder = Some(crate::app::startup_folder::StartupFolder {
            folder: repo.clone(),
            request: None,
        });
        run_app_frame(&ctx, &mut app);
        // The worker of the folder answers the frame after the request, and this
        // test waits for the state that it asserts.
        let deadline = Instant::now() + PATIENCE;
        while app.board.panels.is_empty() {
            assert!(
                Instant::now() < deadline,
                "the folder of the start opened no panel within {PATIENCE:?}"
            );
            run_app_frame(&ctx, &mut app);
            std::thread::sleep(Duration::from_millis(2));
        }

        let workspace = app.workspace_of_folder(&repo).expect("the workspace of the folder");
        assert_eq!(app.board.workspaces.len(), 1);
        let panels = &app.board.workspace(workspace).expect("the workspace").panels;
        assert_eq!(panels.len(), 1, "one panel of the folder");
        let panel = app.board.panel(panels[0]).expect("the panel");
        assert_eq!(
            panel.kind,
            horizon_core::PanelKind::Shell,
            "a terminal of this computer"
        );
        assert_eq!(panel.launch_cwd.as_deref(), Some(repo.as_path()));
        assert_eq!(
            app.board.workspace(workspace).expect("the workspace").name,
            "repo",
            "the workspace takes the name of the folder"
        );
        assert_eq!(app.board.active_workspace, Some(workspace));
        assert_eq!(app.board.focused, Some(panel.id), "the keyboard is in the new panel");
        app.board.shutdown_terminal_panels();
    }

    #[test]
    fn a_start_without_a_folder_changes_nothing() {
        let (_temp, ctx, mut app) = running_app(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });

        run_app_frame(&ctx, &mut app);
        run_app_frame(&ctx, &mut app);

        assert!(app.startup_folder.is_none());
        assert!(app.board.workspaces.is_empty(), "the board of the first start");
    }
}
