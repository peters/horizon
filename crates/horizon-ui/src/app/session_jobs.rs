//! The session store work of a click, on the coordination worker.
//!
//! Listing the sessions reads the files of every session, a duplicate copies the
//! transcripts of a session and a delete removes its folder and the profiles of
//! its browsers. Each takes long enough on a full disk to stop a frame, so the
//! window asks the coordination worker and applies its answer on a later frame.
//! One job runs at a time: the window says that it works and refuses a second
//! click of the same store.

use egui::Context;
use horizon_core::{ResolvedSession, SessionStore, SessionSummary};

use super::{HorizonApp, SessionJobLabel};

/// The work that a job of the session store does.
pub(in crate::app) enum SessionJob {
    List,
    Create,
    Duplicate(String),
    TakeOver(String),
    Resume(String),
    Delete {
        session_id: String,
        with_unreadable_board: bool,
    },
    DeleteAll(Vec<String>),
}

/// The session work that a click asked for, so its failure keeps the sentence
/// that the window showed before the job moved to a worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum SessionAction {
    Create,
    Duplicate,
    TakeOver,
    Resume,
}

impl SessionAction {
    /// What the window says when this action fails.
    fn failure(self) -> &'static str {
        match self {
            Self::Create => "Failed to create session",
            Self::Duplicate => "Failed to copy session",
            Self::TakeOver => "Failed to take over session",
            Self::Resume => "Failed to resume session",
        }
    }
}

/// The answer of a job of the session store.
pub(in crate::app) enum SessionJobAnswer {
    Sessions(horizon_core::Result<Vec<SessionSummary>>),
    Activated {
        action: SessionAction,
        session: horizon_core::Result<Box<ResolvedSession>>,
    },
    Removed {
        session_id: String,
        result: horizon_core::Result<()>,
    },
    /// The sessions that a "Remove All" ended, with the first refusal.
    RemovedMany(horizon_core::Result<()>),
}

impl SessionJob {
    fn label(&self) -> SessionJobLabel {
        match self {
            Self::List => SessionJobLabel::List,
            Self::Create => SessionJobLabel::Create,
            Self::Duplicate(_) => SessionJobLabel::Duplicate,
            Self::TakeOver(_) => SessionJobLabel::TakeOver,
            Self::Resume(_) => SessionJobLabel::Resume,
            Self::Delete { .. } | Self::DeleteAll(_) => SessionJobLabel::Delete,
        }
    }

    fn action(&self) -> SessionAction {
        match self {
            Self::Create | Self::List => SessionAction::Create,
            Self::Duplicate(_) => SessionAction::Duplicate,
            Self::TakeOver(_) => SessionAction::TakeOver,
            Self::Resume(_) | Self::Delete { .. } | Self::DeleteAll(_) => SessionAction::Resume,
        }
    }

    /// Does the work. This is the work of the coordination worker: each call
    /// reads or writes the sessions of this profile.
    fn run(self, store: &SessionStore, config: &horizon_core::Config) -> SessionJobAnswer {
        match self {
            Self::List => SessionJobAnswer::Sessions(store.list_profile_sessions()),
            Self::Create => SessionJobAnswer::Activated {
                action: SessionAction::Create,
                session: store.create_new_session(config).map(Box::new),
            },
            Self::Duplicate(session_id) => SessionJobAnswer::Activated {
                action: SessionAction::Duplicate,
                session: store.duplicate_session(&session_id).map(Box::new),
            },
            Self::TakeOver(session_id) => SessionJobAnswer::Activated {
                action: SessionAction::TakeOver,
                session: store.take_over_session(&session_id).map(Box::new),
            },
            Self::Resume(session_id) => SessionJobAnswer::Activated {
                action: SessionAction::Resume,
                session: store.resume_session(&session_id).map(Box::new),
            },
            Self::Delete {
                session_id,
                with_unreadable_board,
            } => {
                let result = if with_unreadable_board {
                    store.delete_session_with_unreadable_board(&session_id)
                } else {
                    store.delete_session(&session_id)
                };
                SessionJobAnswer::Removed { session_id, result }
            }
            Self::DeleteAll(session_ids) => {
                let mut result = Ok(());
                for session_id in session_ids {
                    if let Err(error) = store.delete_session(&session_id) {
                        result = Err(error);
                        break;
                    }
                }
                SessionJobAnswer::RemovedMany(result)
            }
        }
    }
}

/// The job of the session store that runs, so the window says so and no second
/// job of the same store starts beside it.
#[derive(Default)]
pub(in crate::app) struct SessionJobs {
    running: Option<SessionJobLabel>,
}

impl SessionJobs {
    /// The work that runs now, for the line that the window shows.
    pub(in crate::app) fn running(&self) -> Option<SessionJobLabel> {
        self.running
    }

    /// Whether a job of the session store runs.
    pub(in crate::app) fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Forgets the job that ended, because a session switch replaced the board
    /// that asked for it.
    pub(in crate::app) fn clear(&mut self) {
        self.running = None;
    }
}

impl HorizonApp {
    /// Asks the coordination worker for one job of the session store. A second
    /// job is refused while one runs, so a click cannot queue one per frame.
    pub(in crate::app) fn run_session_job(&mut self, job: SessionJob, ctx: &Context) {
        if self.session_jobs.is_running() {
            return;
        }
        let label = job.label();
        let action = job.action();
        self.session_jobs.running = Some(label);
        let store = self.session_store.clone();
        let config = self.template_config.clone();
        let ctx = ctx.clone();
        self.browser_create_host.io.then(
            move || job.run(&store, &config),
            move |app, answer| {
                app.apply_session_job(answer, &ctx, action);
            },
        );
    }

    /// Applies the answer of a job of the session store that finished.
    fn apply_session_job(&mut self, answer: Option<SessionJobAnswer>, ctx: &Context, action: SessionAction) {
        self.session_jobs.clear();
        let Some(answer) = answer else {
            tracing::error!("a job of the session store did not finish");
            return;
        };
        match answer {
            SessionJobAnswer::Sessions(Ok(sessions)) => self.show_session_manager_list(sessions, None),
            SessionJobAnswer::Sessions(Err(error)) => {
                self.show_session_manager_list(Vec::new(), Some(format!("Failed to load sessions: {error}")))
            }
            SessionJobAnswer::Activated {
                action,
                session: Ok(session),
            } => self.activate_session_of_a_job(ctx, &session, action),
            SessionJobAnswer::Activated {
                action,
                session: Err(horizon_core::Error::UnreadableBoard { reason, .. }),
            } => {
                // The session asked for a board that this Horizon cannot read.
                // The window asks the person before the board goes with it.
                tracing::info!("a session keeps its board until a person answers: {reason}");
                self.session_job_failed(action, &format!("an unreadable board: {reason}"));
            }
            SessionJobAnswer::Activated {
                action: _,
                session: Err(error),
            } => self.session_job_failed(action, &error.to_string()),
            SessionJobAnswer::Removed {
                session_id,
                result: Ok(()),
            } => {
                let _ = session_id;
                self.reload_session_manager_from_a_job(None, None, ctx);
            }
            SessionJobAnswer::Removed {
                session_id,
                result: Err(horizon_core::Error::UnreadableBoard { reason, .. }),
            } => {
                // The removal read the board for its browser profiles and refused
                // it. Ask the person again, with the reason, before the board goes
                // with the session.
                tracing::info!("session {session_id} keeps its board until a person answers: {reason}");
                self.ask_to_remove_an_unreadable_board(session_id);
            }
            SessionJobAnswer::Removed {
                session_id: _,
                result: Err(error),
            } => self.session_job_refused("Failed to remove session", &error.to_string()),
            SessionJobAnswer::RemovedMany(Ok(())) => self.reload_session_manager_from_a_job(None, None, ctx),
            SessionJobAnswer::RemovedMany(Err(error)) => self.reload_session_manager_from_a_job(
                None,
                // The sessions that the removal ended are listed again, with
                // the reason that stopped the rest.
                Some(format!("Failed to remove sessions: {error}")),
                ctx,
            ),
        }
    }

    /// Opens the session that a job resolved, in the window that asked for it.
    fn activate_session_of_a_job(&mut self, ctx: &Context, session: &ResolvedSession, action: SessionAction) {
        if self.startup_chooser.is_some() {
            self.activate_startup_session(ctx, session);
        } else {
            self.activate_runtime_session(ctx, session);
            if self.session_manager.is_some() {
                // The manager lists the sessions again, so it opens with this one.
                self.reload_session_manager_from_a_job(Some(session.session_id.clone()), None, ctx);
            }
        }
        let _ = action;
    }

    /// Applies the answers of the session store jobs that the worker wrote, for
    /// a test that judges a window right after its click.
    #[cfg(test)]
    pub(in crate::app) fn settle_session_jobs(&mut self) {
        self.settle_browser_host_io();
    }

    /// Says why a job of the session store failed, in the window that asked for
    /// it, with the sentence that this window used before the job moved.
    fn session_job_failed(&mut self, action: SessionAction, message: &str) {
        self.session_job_refused(action.failure(), message);
    }

    fn session_job_refused(&mut self, prefix: &str, message: &str) {
        let sentence = format!("{prefix}: {message}");
        if self.startup_chooser.is_some() {
            self.set_startup_error(sentence);
        } else {
            self.set_session_manager_error(sentence);
        }
        tracing::error!(%message, "a job of the session store failed");
    }
}

#[cfg(test)]
mod tests {
    use egui::Context;

    use super::{SessionJob, SessionJobs};
    use crate::app::test_support::{run_app_frame, test_app_with_config_and_startup};
    use horizon_core::{Config, RuntimeState, StartupDecision};

    fn app() -> (tempfile::TempDir, Context, crate::app::HorizonApp) {
        test_app_with_config_and_startup(
            &Config::default(),
            StartupDecision::Ephemeral {
                runtime_state: Box::new(RuntimeState::default()),
            },
        )
    }

    fn sessions_of(app: &crate::app::HorizonApp) -> usize {
        app.session_store
            .list_profile_sessions()
            .expect("the sessions of this profile")
            .len()
    }

    #[test]
    fn the_session_manager_lists_duplicates_and_removes_with_the_same_answers() {
        let (temp, ctx, mut app) = app();
        let first = app
            .session_store
            .create_session_from_runtime(RuntimeState::default())
            .expect("a saved session");
        let second = app
            .session_store
            .create_session_from_runtime(RuntimeState::default())
            .expect("a saved session");
        assert_eq!(sessions_of(&app), 2);

        // The list reads the sessions of this profile on the worker.
        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            app.toggle_session_manager(&ctx);
            assert!(app.session_manager.is_none(), "the manager opens on the answer");
            assert_eq!(
                app.session_jobs.running(),
                Some(crate::app::SessionJobLabel::List),
                "the window says that it lists the sessions"
            );
        }
        app.settle_session_jobs();
        assert!(app.session_manager.is_some(), "the answer opened the manager");
        assert_eq!(
            app.session_manager.as_ref().expect("the manager").sessions.len(),
            2,
            "the manager lists every saved session"
        );

        // A duplicate copies the folder of the session on the worker.
        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            app.run_session_job(SessionJob::Duplicate(first.session_id.clone()), &ctx);
        }
        app.settle_session_jobs();
        assert_eq!(sessions_of(&app), 3, "the duplicate is a saved session of its own");

        // A removal takes the folder of the session away on the worker.
        let to_remove = second.session_id.clone();
        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            app.run_session_job(
                SessionJob::Delete {
                    session_id: to_remove.clone(),
                    with_unreadable_board: false,
                },
                &ctx,
            );
        }
        app.settle_session_jobs();
        assert!(
            !app.session_store
                .list_profile_sessions()
                .expect("the sessions of this profile")
                .iter()
                .any(|session| session.session_id == to_remove),
            "the removed session is gone"
        );

        // A removal that fails says so in the words of the window.
        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            app.run_session_job(
                SessionJob::Delete {
                    session_id: "no-such-session".to_owned(),
                    with_unreadable_board: false,
                },
                &ctx,
            );
        }
        app.settle_session_jobs();
        let error = app
            .session_manager
            .as_ref()
            .and_then(|state| state.error.clone())
            .expect("the window says why the removal failed");
        assert!(error.starts_with("Failed to remove session"), "{error}");

        // One job runs at a time, so a second click queues nothing.
        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            app.run_session_job(SessionJob::List, &ctx);
            app.run_session_job(SessionJob::Create, &ctx);
            assert_eq!(
                app.session_jobs.running(),
                Some(crate::app::SessionJobLabel::List),
                "the second job of the same store is refused"
            );
        }
        app.settle_session_jobs();
        let _ = run_app_frame;
        let _ = temp;
        let _: SessionJobs = SessionJobs::default();
    }
}
