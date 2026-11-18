//! `horizon <folder>` of a start of a Horizon that already runs: the window half.
//!
//! The caller writes one request into the home, and this pump of a window takes
//! it: it claims one request on the coordination worker, applies it through the
//! same function that a start with that folder uses, and writes the answer that
//! the caller reads.
//!
//! A window answers only while it holds a session lease, because that lease is
//! what the caller saw before it wrote the request. A window that runs with
//! `--ephemeral` keeps no lease and takes no request; it removes what is stale
//! instead.
//!
//! The frame does no file work of its own. The host request pump of this window
//! polls the request folder, and it wakes the window when a request waits there,
//! so a window that presents nothing still answers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::folder_requests::{self, FolderRequest};

use super::HorizonApp;

/// The least wait between two scans of the request folder of this window. The
/// host request pump wakes the window when a request waits, so this only bounds
/// the reads of a folder that holds one.
const SCAN_INTERVAL: Duration = Duration::from_millis(250);

/// The state of the folder requests of one window.
#[derive(Default)]
pub(in crate::app) struct FolderRequestPump {
    /// Set by the host request pump when the folder of this home holds a request
    /// that a window may apply, and replaced by that pump when it installs this
    /// window.
    pub(in crate::app) waiting: Arc<AtomicBool>,
    /// When this window last read the request folder.
    last_scan: Option<Instant>,
    /// The requests that this window claimed and has not applied yet.
    pending: Vec<FolderRequest>,
}

impl HorizonApp {
    /// Whether this window holds a session lease, which is how a second start of
    /// this computer finds it. A window that took no session, or one that runs
    /// with `--ephemeral`, writes no lease: no start can ask it for anything. A
    /// lease counts from the answer of the session writer that wrote it, and
    /// until a write of it fails.
    pub(in crate::app) fn holds_session_lease(&self) -> bool {
        let held = |session: &super::ActiveSession| session.persistent && session.lease_written;
        self.active_session.as_ref().is_some_and(held)
    }

    /// Takes the folder requests that wait for this window, and applies one per
    /// frame through the folder of a start.
    pub(in crate::app) fn poll_folder_requests(&mut self, ctx: &Context) {
        self.apply_claimed_folder_request(ctx);
        let waiting = self.folder_requests.waiting.swap(false, Ordering::AcqRel);
        if !waiting
            || self
                .folder_requests
                .last_scan
                .is_some_and(|last| last.elapsed() < SCAN_INTERVAL)
        {
            return;
        }
        self.folder_requests.last_scan = Some(Instant::now());
        let root = self.session_store.home().root().to_path_buf();
        let now = folder_requests::now_millis();
        // The caller saw the lease of a window of this home, so only a window
        // that holds one answers it.
        if !self.holds_session_lease() {
            self.browser_create_host.io.then(
                move || folder_requests::prune(&root, now),
                |_, pruned| log_failure(pruned, "prune the folder requests of a window without a lease"),
            );
            return;
        }
        self.browser_create_host.io.then(
            move || folder_requests::claim(&root, now),
            |app, claimed| match claimed {
                Some(Ok(claimed)) => app.folder_requests.pending.extend(claimed),
                Some(Err(error)) => tracing::warn!(%error, "the folder requests of this home are not readable"),
                None => tracing::warn!("the claim of the folder requests of this home did not finish"),
            },
        );
    }

    /// Applies the first request that this window claimed, unless the folder of
    /// a start still waits for its frame or a chooser of the start holds the
    /// window.
    fn apply_claimed_folder_request(&mut self, ctx: &Context) {
        if self.startup_folder.is_some() || self.folder_requests.pending.is_empty() {
            return;
        }
        if self.startup_chooser.is_some() {
            // The window asks the person before it opens a board, so it cannot
            // show a folder of another process yet.
            for request in std::mem::take(&mut self.folder_requests.pending) {
                self.answer_folder_request(
                    request,
                    horizon_core::folder_requests::Answer::Refused {
                        reason: horizon_core::folder_requests::WINDOW_BUSY.to_string(),
                    },
                );
            }
            return;
        }
        let request = self.folder_requests.pending.remove(0);
        let Some(folder) = request.folder().map(std::path::Path::to_path_buf) else {
            // The start named no folder: it asks for the window itself, and this
            // window shows and raises it, or says that it ends with its window.
            if self.window_stays_open() {
                self.show_main_window(ctx);
                self.answer_folder_request(request, horizon_core::folder_requests::Answer::Window);
            } else {
                self.answer_folder_request(
                    request,
                    horizon_core::folder_requests::Answer::Refused {
                        reason: horizon_core::folder_requests::KEEP_RUNNING_OFF.to_owned(),
                    },
                );
            }
            return;
        };
        self.show_main_window(ctx);
        self.startup_folder = Some(super::startup_folder::StartupFolder {
            folder,
            request: Some(request),
        });
    }

    /// Answers the caller of a request on the coordination worker, and marks the
    /// board for the save that a change of the person makes.
    pub(in crate::app) fn answer_folder_request(
        &mut self,
        request: FolderRequest,
        answer: horizon_core::folder_requests::Answer,
    ) {
        let root = self.session_store.home().root().to_path_buf();
        self.browser_create_host.io.then(
            move || horizon_core::folder_requests::answer(&root, &request, &answer),
            |_, answered| log_failure(answered, "answer a folder request"),
        );
        self.mark_runtime_dirty();
    }
}

/// One line of the log for a request that this window could not answer or
/// remove. It is not an error of the window: the caller waits a bounded time.
fn log_failure(result: Option<Result<(), horizon_core::folder_requests::FolderRequestError>>, what: &str) {
    match result {
        Some(Err(error)) => tracing::warn!(%error, "could not {what}"),
        Some(Ok(())) => {}
        None => tracing::warn!("the work of the folder requests did not finish: {what}"),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use horizon_core::folder_requests::{self, Answer};
    use horizon_core::{PresetConfig, RuntimeState, StartupDecision};

    use crate::app::HorizonApp;
    use crate::app::test_support::{run_app_frame, run_app_frame_with_input, test_app_with_startup};

    /// The window holds a session and writes its lease, which is what a caller
    /// of the channel sees before it asks.
    fn holds_a_lease(app: &mut HorizonApp) {
        app.active_session = Some(crate::app::ActiveSession {
            session_id: "session-of-the-window".to_string(),
            lease: None,
            lease_answers: Vec::new(),
            lease_written: true,
            last_lease_refresh: None,
            persistent: true,
        });
    }

    /// The terminal panel of a folder starts a real shell, so the test keeps no
    /// agent preset: a folder of this test is a terminal in that folder.
    fn no_agent_presets(app: &mut HorizonApp) {
        app.presets
            .retain(|preset: &PresetConfig| preset.kind == horizon_core::PanelKind::Shell);
    }

    fn waiting_app() -> (tempfile::TempDir, egui::Context, HorizonApp) {
        let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        no_agent_presets(&mut app);
        run_app_frame(&ctx, &mut app);
        (temp, ctx, app)
    }

    /// One frame that the host request pump woke takes the request of a second
    /// start, shows its folder and answers the caller.
    ///
    /// The whole suite runs under the strict guard of the UI thread, so a read,
    /// a claim, a write or a removal on the frame ends this test.
    #[test]
    fn a_frame_takes_the_request_of_a_second_start_and_answers_it() {
        let (_temp, ctx, mut app) = waiting_app();
        let root = app.session_store.home().root().to_path_buf();
        let folder = root.join("repo");
        std::fs::create_dir_all(&folder).expect("the folder");
        holds_a_lease(&mut app);

        let (asking_root, asking_folder) = (root.clone(), folder.clone());
        let caller = std::thread::spawn(move || folder_requests::ask(&asking_root, &asking_folder));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !caller.is_finished() {
            app.folder_requests
                .waiting
                .store(true, std::sync::atomic::Ordering::Release);
            run_app_frame(&ctx, &mut app);
            assert!(Instant::now() < deadline, "the window took and answered the request");
            std::thread::sleep(Duration::from_millis(10));
        }
        let answer = caller.join().expect("the caller").expect("a caller of the channel");

        assert_eq!(
            answer,
            Some(Answer::Made {
                workspace: "repo".to_string()
            })
        );
        let workspace = app.workspace_of_folder(&folder).expect("the workspace of the folder");
        assert_eq!(app.board.active_workspace, Some(workspace), "the folder is in front");
        assert!(
            std::fs::read_dir(folder_requests::directory(&root)).is_ok_and(|entries| entries.count() == 0),
            "the request and its claim are gone: {:?}",
            std::fs::read_dir(folder_requests::directory(&root)).map(|entries| entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name())
                .collect::<Vec<_>>())
        );
        app.board.shutdown_terminal_panels();
    }

    /// A second start that the board already shows is answered `shown`, and the
    /// board keeps its panels.
    #[test]
    fn a_folder_of_a_workspace_already_on_the_board_is_answered_shown() {
        let (_temp, ctx, mut app) = waiting_app();
        let root = app.session_store.home().root().to_path_buf();
        let folder = root.join("repo");
        std::fs::create_dir_all(&folder).expect("the folder");
        holds_a_lease(&mut app);
        let workspace = app.ensure_workspace_of_folder(&ctx, &folder);
        let panels_before = app.board.panels.len();

        let (asking_root, asking_folder) = (root.clone(), folder.clone());
        let caller = std::thread::spawn(move || folder_requests::ask(&asking_root, &asking_folder));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !caller.is_finished() {
            app.folder_requests
                .waiting
                .store(true, std::sync::atomic::Ordering::Release);
            run_app_frame(&ctx, &mut app);
            assert!(Instant::now() < deadline, "the window answered the request");
            std::thread::sleep(Duration::from_millis(10));
        }
        let answer = caller.join().expect("the caller").expect("a caller of the channel");

        assert!(matches!(answer, Some(Answer::Shown { .. })), "{answer:?}");
        assert_eq!(app.board.workspaces.len(), 1, "no second workspace of that folder");
        assert_eq!(app.board.panels.len(), panels_before, "the folder adds no panel");
        assert_eq!(app.board.active_workspace, Some(workspace));
        app.board.shutdown_terminal_panels();
    }

    /// A start that names no folder asks the window of this home to show itself:
    /// the frame sends the show and the focus, and the caller reads the answer.
    #[test]
    fn a_frame_shows_the_window_for_a_second_start_that_names_no_folder() {
        let (_temp, ctx, mut app) = waiting_app();
        let root = app.session_store.home().root().to_path_buf();
        holds_a_lease(&mut app);
        app.template_config.features.when_window_closes = horizon_core::WindowCloseSetting::KeepRunning;

        let asking_root = root.clone();
        let caller = std::thread::spawn(move || horizon_core::folder_requests::ask_window(&asking_root));
        let mut sent = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !caller.is_finished() {
            app.folder_requests
                .waiting
                .store(true, std::sync::atomic::Ordering::Release);
            sent.extend(
                run_app_frame_with_input(&ctx, &mut app, egui::RawInput::default())
                    .viewport_output
                    .get(&egui::ViewportId::ROOT)
                    .map(|output| output.commands.clone())
                    .unwrap_or_default(),
            );
            assert!(Instant::now() < deadline, "the window showed itself");
            std::thread::sleep(Duration::from_millis(10));
        }
        let answer = caller.join().expect("the caller").expect("a caller of the channel");

        assert_eq!(answer, Some(Answer::Window), "the window showed itself");
        assert!(sent.contains(&egui::ViewportCommand::Visible(true)), "{sent:?}");
        assert!(sent.contains(&egui::ViewportCommand::Focus), "{sent:?}");
        app.board.shutdown_terminal_panels();
    }

    /// A window that ends with its window answers the start that names no folder
    /// with the reason, so that start opens its own window as it did.
    #[test]
    fn a_window_that_ends_with_its_window_shows_nothing_for_a_second_start() {
        let (_temp, ctx, mut app) = waiting_app();
        let root = app.session_store.home().root().to_path_buf();
        holds_a_lease(&mut app);
        app.template_config.features.when_window_closes = horizon_core::WindowCloseSetting::Quit;

        let asking_root = root.clone();
        let caller = std::thread::spawn(move || {
            horizon_core::folder_requests::ask_window_with(&asking_root, Duration::from_millis(1500))
        });
        let mut sent = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !caller.is_finished() {
            app.folder_requests
                .waiting
                .store(true, std::sync::atomic::Ordering::Release);
            sent.extend(
                run_app_frame_with_input(&ctx, &mut app, egui::RawInput::default())
                    .viewport_output
                    .get(&egui::ViewportId::ROOT)
                    .map(|output| output.commands.clone())
                    .unwrap_or_default(),
            );
            assert!(Instant::now() < deadline, "the window answered the request");
            std::thread::sleep(Duration::from_millis(10));
        }
        let answer = caller.join().expect("the caller").expect("a caller of the channel");

        assert_eq!(
            answer,
            Some(Answer::Refused {
                reason: horizon_core::folder_requests::KEEP_RUNNING_OFF.to_owned()
            })
        );
        assert!(!sent.contains(&egui::ViewportCommand::Visible(true)), "{sent:?}");
        app.board.shutdown_terminal_panels();
    }

    /// A window that holds no lease takes no request: the caller of this test
    /// waits its bound and reads no answer, and the board opens nothing.
    #[test]
    fn a_window_without_a_lease_takes_no_request() {
        let (_temp, ctx, mut app) = waiting_app();
        let root = app.session_store.home().root().to_path_buf();
        let folder = root.join("repo");
        std::fs::create_dir_all(&folder).expect("the folder");

        let (asking_root, asking_folder) = (root.clone(), folder.clone());
        let caller = std::thread::spawn(move || {
            folder_requests::ask_with(&asking_root, &asking_folder, Duration::from_millis(1500))
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !caller.is_finished() {
            app.folder_requests
                .waiting
                .store(true, std::sync::atomic::Ordering::Release);
            run_app_frame(&ctx, &mut app);
            assert!(Instant::now() < deadline, "the caller of the channel ended");
            std::thread::sleep(Duration::from_millis(10));
        }
        let answer = caller.join().expect("the caller").expect("a caller of the channel");

        assert_eq!(answer, None, "a window without a lease answers nothing");
        assert!(app.board.workspaces.is_empty(), "the window opened no board");
    }
}
