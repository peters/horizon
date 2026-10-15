//! The setup runs: at most one for each repository folder, each named by its agent's
//! panel, and the runs being ended in the background.
use super::run::Run;
use crate::app::cloud_panel::production::Production;
use horizon_core::PanelId;
use std::time::{Duration, Instant};

impl Production {
    /// The run of the repository the dialog shows.
    pub(in crate::app::cloud_panel::production) fn run_here(&self) -> Option<&Run> {
        self.setup_runs
            .iter()
            .find(|run| super::same_folder(run, &self.repository))
    }

    /// The agent panel that names the run of the repository the dialog shows.
    pub(in crate::app::cloud_panel::production) fn run_here_key(&self) -> Option<PanelId> {
        self.run_here().map(|run| run.panel)
    }

    pub(super) fn run_here_mut(&mut self) -> Option<&mut Run> {
        let entered = self.repository.clone();
        self.setup_runs.iter_mut().find(|run| super::same_folder(run, &entered))
    }

    /// The run whose agent started in `panel`.
    pub(super) fn setup_run(&self, panel: PanelId) -> Option<&Run> {
        self.setup_runs.iter().find(|run| run.panel == panel)
    }

    pub(super) fn setup_run_mut(&mut self, panel: PanelId) -> Option<&mut Run> {
        self.setup_runs.iter_mut().find(|run| run.panel == panel)
    }

    /// Takes the run whose agent started in `panel` out of the set; it must leave
    /// through [`Run::discard`], since dropping it here would wait for its ending.
    pub(super) fn take_setup_run(&mut self, panel: PanelId) -> Option<Run> {
        let index = self.setup_runs.iter().position(|run| run.panel == panel)?;
        Some(self.setup_runs.remove(index))
    }

    /// The agent panels of every run.
    pub(super) fn setup_run_panels(&self) -> Vec<PanelId> {
        self.setup_runs.iter().map(|run| run.panel).collect()
    }

    /// A setup of the repository the dialog shows is about to start.
    pub(in crate::app::cloud_panel::production) fn starting_here(&self) -> bool {
        let entered = self.repository.trim();
        self.setup_starting.iter().any(|starting| starting.entered == entered)
    }
}

/// Runs, agents and proof resources being ended in the background, each with the
/// repository whose run it ends when it ends a run or what that run made.
static ENDING: std::sync::Mutex<Vec<(Option<std::path::PathBuf>, std::thread::JoinHandle<()>)>> =
    std::sync::Mutex::new(Vec::new());

/// Keeps `ending`, a thread that ends something a run made, such as a container, so
/// Horizon's exit waits for it.
pub(in crate::app::cloud_panel::production::repository_setup) fn keep_ending(ending: std::thread::JoinHandle<()>) {
    ENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((None, ending));
}

/// Keeps `ending`, a thread that ends a run of `repository`, its agent or its proof
/// resources, so a new setup of that repository and Horizon's exit wait for it.
pub(in crate::app::cloud_panel::production::repository_setup) fn keep_ending_of(
    repository: &std::path::Path,
    ending: std::thread::JoinHandle<()>,
) {
    ENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((Some(repository.to_path_buf()), ending));
}

/// Waits until every ending of a run of `repository` has ended, with what their ending
/// of that repository started in turn: a new setup of it starts only then, so its agent
/// never works beside an older one that is still ending. Runs of other repositories go
/// on as they are.
pub(in crate::app::cloud_panel::production::repository_setup) fn wait_for_ended_runs_of(repository: &std::path::Path) {
    let _ = wait_for(Some(repository), None);
}

/// Waits for at most `limit` until every run, agent and proof resource being ended in
/// the background has ended, with what their ending started in turn: Horizon exits only
/// then. Whether every one ended in time.
pub(in crate::app::cloud_panel::production::repository_setup) fn wait_for_ended_runs_within(
    limit: Option<Duration>,
) -> bool {
    wait_for(None, limit)
}

/// No waiter takes a handle out before its thread has finished, so every waiter sees
/// each running one: a thread leaves the list only once its work has returned, and any
/// ending it started by then is in the list already. `repository` limits the wait to the
/// endings of that repository's runs.
fn wait_for(repository: Option<&std::path::Path>, limit: Option<Duration>) -> bool {
    let deadline = limit.map(|limit| Instant::now() + limit);
    loop {
        {
            let mut ending = ENDING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            ending.retain(|(_, run)| !run.is_finished());
            let waits =
                |of: &Option<std::path::PathBuf>| repository.is_none_or(|repository| of.as_deref() == Some(repository));
            if !ending.iter().any(|(of, _)| waits(of)) {
                return true;
            }
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

// The setup agent is offered on Linux only.
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::super::run::{self, Action};
    use crate::app::HorizonApp;
    use horizon_core::{PanelId, PanelKind};
    use std::time::{Duration, Instant};

    fn app() -> (tempfile::TempDir, egui::Context, HorizonApp) {
        let (temp, ctx, mut app) =
            crate::app::test_support::test_app_with_startup(horizon_core::StartupDecision::Ephemeral {
                runtime_state: Box::default(),
            });
        // A non-process panel stands in for the agent, so no real login starts in a test.
        app.cloud_prototype.production.setup_agent = Some(PanelKind::Usage);
        app.cloud_prototype.production.setup_presets = vec![horizon_core::PresetConfig {
            name: "Test panel".into(),
            kind: PanelKind::Usage,
            alias: None,
            command: None,
            args: Vec::new(),
            resume: horizon_core::PanelResume::default(),
            ssh_connection: None,
        }];
        (temp, ctx, app)
    }

    /// Starts a setup of `folder` from the dialog and waits until its agent started.
    fn start(app: &mut HorizonApp, ctx: &egui::Context, folder: &std::path::Path) -> PanelId {
        app.cloud_prototype.production.repository = folder.display().to_string();
        app.start_cloud_repository_setup(ctx);
        let started = Instant::now();
        while !app.cloud_prototype.production.setup_starting.is_empty() {
            assert!(started.elapsed() < Duration::from_secs(20), "the baseline was read");
            std::thread::sleep(Duration::from_millis(20));
            app.sync_cloud_repository_setup(ctx);
        }
        app.cloud_prototype
            .production
            .run_here_key()
            .expect("the setup of the repository the dialog shows runs")
    }

    #[test]
    fn two_repositories_are_set_up_at_once_and_each_stops_alone() {
        let (temp, ctx, mut app) = app();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let a = start(&mut app, &ctx, &first);
        let b = start(&mut app, &ctx, &second);
        assert_ne!(a, b);
        assert!(app.cloud_prototype.error.is_none(), "{:?}", app.cloud_prototype.error);
        assert_eq!(app.cloud_prototype.production.setup_runs.len(), 2, "the first goes on");
        // A frame keeps both agents: neither is an orphan of the other.
        app.sync_cloud_repository_setup(&ctx);
        assert!(app.board.panel(a).is_some() && app.board.panel(b).is_some());
        // The dialog shows the run of the repository it shows.
        app.cloud_prototype.production.repository = first.display().to_string();
        assert_eq!(app.cloud_prototype.production.run_here_key(), Some(a));
        app.cloud_prototype.production.repository = second.display().to_string();
        assert_eq!(app.cloud_prototype.production.run_here_key(), Some(b));
        app.cloud_repository_setup_action(&ctx, a, Action::Stop);
        assert!(app.board.panel(a).is_none(), "the stopped agent's panel closed");
        assert!(app.board.panel(b).is_some(), "the other agent works on");
        assert_eq!(app.cloud_prototype.production.setup_runs.len(), 1);
        assert_eq!(app.cloud_prototype.production.setup_runs[0].panel, b);
        app.end_cloud_repository_setup();
        assert!(app.cloud_prototype.production.setup_runs.is_empty());
    }

    #[test]
    fn one_folder_entered_twice_still_gets_one_run() {
        let (temp, ctx, mut app) = app();
        let folder = temp.path().join("repository");
        std::fs::create_dir_all(&folder).unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&folder, &link).unwrap();
        let first = start(&mut app, &ctx, &folder);
        // The link names another text, so the dialog offers a setup; the run it would start
        // works in the same folder.
        app.cloud_prototype.production.repository = link.display().to_string();
        app.start_cloud_repository_setup(&ctx);
        let started = Instant::now();
        while !app.cloud_prototype.production.setup_starting.is_empty() {
            assert!(started.elapsed() < Duration::from_secs(20), "the baseline was read");
            std::thread::sleep(Duration::from_millis(20));
            app.sync_cloud_repository_setup(&ctx);
        }
        assert_eq!(app.cloud_prototype.production.setup_runs.len(), 1);
        assert_eq!(app.cloud_prototype.production.setup_runs[0].panel, first);
        let error = app.cloud_prototype.error.clone().unwrap_or_default();
        assert!(error.contains("already runs"), "{error}");
        app.end_cloud_repository_setup();
    }

    #[test]
    fn a_new_setup_waits_only_for_the_runs_of_its_own_repository() {
        let first = std::path::PathBuf::from("/runs-test/first");
        let second = std::path::PathBuf::from("/runs-test/second");
        let (release, released) = std::sync::mpsc::channel::<()>();
        run::keep_ending_of(
            &first,
            std::thread::spawn(move || {
                let _ = released.recv_timeout(Duration::from_secs(20));
            }),
        );
        let started = Instant::now();
        run::wait_for_ended_runs_of(&second);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "another repository's ending is not waited for"
        );
        let waiter = std::thread::spawn(move || {
            run::wait_for_ended_runs_of(&first);
            Instant::now()
        });
        std::thread::sleep(Duration::from_millis(200));
        assert!(!waiter.is_finished(), "its own repository's ending is waited for");
        let let_go = Instant::now();
        release.send(()).unwrap();
        assert!(waiter.join().unwrap() >= let_go);
    }
}
