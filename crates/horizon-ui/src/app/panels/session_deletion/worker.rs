use std::sync::Arc;
use std::thread::JoinHandle;

use horizon_core::{AgentSessionBinding, AgentSessionDeletionReport};

use super::{DeletionJob, job_id};
use crate::app::HorizonApp;

#[derive(Default)]
pub(in crate::app) struct SavedSessionDeletionState {
    pub(super) worker: Option<DeletionWorker>,
    notice: Option<String>,
    persisted_notice: Option<String>,
}

pub(super) struct DeletionWorker {
    pub(super) handle: JoinHandle<()>,
    pub(super) context: egui::Context,
    pub(super) sessions: Arc<[AgentSessionBinding]>,
}

impl SavedSessionDeletionState {
    pub(in crate::app) fn has_notice(&self) -> bool {
        self.notice.is_some()
    }
}

impl HorizonApp {
    pub(in crate::app) fn restore_saved_session_deletion_notice(&mut self) {
        match self.session_store.saved_session_deletion_notice() {
            Ok(notice) => {
                self.saved_session_deletion.persisted_notice.clone_from(&notice);
                self.saved_session_deletion.notice = notice;
            }
            Err(error) => {
                self.saved_session_deletion.notice = Some(format!(
                    "Could not read saved conversation recovery information: {error}"
                ));
            }
        }
    }

    pub(in crate::app) fn saved_session_deletion_is_running(&self) -> bool {
        self.saved_session_deletion.worker.is_some()
    }

    pub(super) fn join_finished_saved_session_deletion(&mut self) -> bool {
        if self
            .saved_session_deletion
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.handle.is_finished())
        {
            return false;
        }
        if let Some(worker) = self.saved_session_deletion.worker.take() {
            self.join_saved_session_deletion(worker);
        }
        true
    }

    fn join_saved_session_deletion(&mut self, worker: DeletionWorker) {
        if worker.handle.join().is_err() {
            if let Some(job) = worker.context.data(|data| data.get_temp::<DeletionJob>(job_id())) {
                let mut state = job.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let done = state.done;
                state.report.failures.extend(worker.sessions.iter().skip(done).map(|session| {
                    horizon_core::AgentSessionDeletionFailure {
                        session_id: session.session_id.clone(),
                        message: "Deletion worker stopped unexpectedly; inspect retained recovery bundles before retrying".into(),
                    }
                }));
                state.finished = true;
            }
            self.retain_deletion_notice("Saved conversation deletion stopped unexpectedly. Inspect retained recovery bundles using docs/session-recovery.md before retrying.");
        }
    }

    pub(in crate::app) fn finish_saved_session_deletion_for_shutdown(&mut self) -> bool {
        if let Some(ctx) = self
            .saved_session_deletion
            .worker
            .as_ref()
            .map(|worker| worker.context.clone())
        {
            self.poll_saved_session_deletion(&ctx);
        }
        self.saved_session_deletion.worker.is_none() && self.saved_session_deletion.notice.is_none()
    }

    pub(in crate::app) fn wait_for_saved_session_deletion(&mut self) {
        if let Some(worker) = self.saved_session_deletion.worker.take() {
            let ctx = worker.context.clone();
            self.join_saved_session_deletion(worker);
            self.poll_saved_session_deletion(&ctx);
        }
    }

    pub(super) fn retain_saved_session_deletion_notice(&mut self, report: &AgentSessionDeletionReport) {
        let mut details = Vec::new();
        for recovery in &report.recoveries {
            details.push(format!(
                "Conversation {}: {}\nRecovery directory: {}",
                recovery.session_id,
                recovery.message,
                recovery.directory.display()
            ));
        }
        for warning in &report.cleanup_warnings {
            details.push(format!(
                "Conversation {}: {}\nCleanup directory: {}",
                warning.session_id,
                warning.message,
                warning.directory.display()
            ));
        }
        if !details.is_empty() {
            self.retain_deletion_notice(&details.join("\n\n"));
        }
    }

    fn retain_deletion_notice(&mut self, notice: &str) {
        let combined = self
            .saved_session_deletion
            .notice
            .as_ref()
            .map_or_else(|| notice.to_owned(), |previous| format!("{previous}\n\n{notice}"));
        match self.session_store.save_session_deletion_notice(notice) {
            Ok(()) => {
                self.restore_saved_session_deletion_notice();
            }
            Err(error) => {
                self.saved_session_deletion.notice = Some(format!(
                    "{combined}\n\nCould not save this notice: {error}. Keep these recovery paths before closing."
                ));
            }
        }
    }

    pub(in crate::app) fn render_saved_session_deletion_notice(&mut self, ctx: &egui::Context) {
        let Some(notice) = self.saved_session_deletion.notice.clone() else {
            return;
        };
        let acknowledged = egui::Modal::new(egui::Id::new("saved_session_deletion_notice"))
            .area(egui::Modal::default_area(egui::Id::new("saved_session_deletion_notice")).order(egui::Order::Tooltip))
            .show(ctx, |ui| {
                ui.set_max_width(620.0);
                ui.heading("Saved conversation recovery");
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    ui.label(notice);
                });
                ui.label("Keep the listed paths if recovery or cleanup is still needed.");
                ui.button("I have reviewed this notice").clicked()
            })
            .inner;
        if acknowledged {
            if let Some(expected) = &self.saved_session_deletion.persisted_notice
                && let Err(error) = self.session_store.acknowledge_session_deletion_notice(expected)
            {
                self.saved_session_deletion.notice =
                    Some(format!("Could not acknowledge recovery information: {error}"));
                self.restore_saved_session_deletion_notice();
                return;
            }
            self.saved_session_deletion.notice = None;
            self.saved_session_deletion.persisted_notice = None;
            self.restore_saved_session_deletion_notice();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{DeletionProgress, receipt_id};
    use super::*;
    use std::sync::{Mutex, mpsc};

    fn install_worker(app: &mut HorizonApp, ctx: &egui::Context, panic: bool) -> mpsc::Sender<()> {
        let session = AgentSessionBinding::new(
            horizon_core::PanelKind::Claude,
            format!("shutdown-test-{panic}"),
            None,
            None,
            None,
        );
        let sessions: Arc<[_]> = vec![session].into();
        let state = Arc::new(Mutex::new(DeletionProgress {
            total: 1,
            ..Default::default()
        }));
        let reservation = horizon_core::reserve_saved_session_deletions(&sessions).expect("reservation");
        ctx.data_mut(|data| {
            data.insert_temp(
                job_id(),
                DeletionJob {
                    owner: horizon_core::PanelId(99),
                    viewport: egui::ViewportId::ROOT,
                    reservation: Arc::new(reservation),
                    state: Arc::clone(&state),
                },
            )
        });
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            rx.recv().expect("release worker");
            assert!(!panic, "synthetic worker panic");
            state.lock().expect("state").finished = true;
        });
        app.saved_session_deletion.worker = Some(DeletionWorker {
            handle,
            context: ctx.clone(),
            sessions,
        });
        tx
    }

    #[test]
    fn graceful_shutdown_waits_for_worker_before_consuming_report() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let ctx = egui::Context::default();
        let release = install_worker(&mut app, &ctx, false);
        app.begin_shutdown();
        app.poll_shutdown_progress();
        assert!(!app.exit_cleanup_complete);
        assert!(!app.finish_saved_session_deletion_for_shutdown());
        release.send(()).expect("release");
        app.wait_for_saved_session_deletion();
        assert!(app.finish_saved_session_deletion_for_shutdown());
        assert!(ctx.data(|data| data.get_temp::<DeletionJob>(job_id())).is_none());
        assert!(
            ctx.data(|data| data.get_temp::<Arc<AgentSessionDeletionReport>>(receipt_id()))
                .is_some()
        );
    }

    #[test]
    fn synchronous_cleanup_joins_and_panic_preserves_notice_without_hanging() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let ctx = egui::Context::default();
        let release = install_worker(&mut app, &ctx, true);
        let sender = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            release.send(()).expect("release");
        });
        app.run_exit_cleanup();
        sender.join().expect("sender");
        assert!(!app.saved_session_deletion_is_running());
        assert!(app.saved_session_deletion.has_notice());
        assert!(
            app.session_store
                .saved_session_deletion_notice()
                .expect("persisted notice")
                .is_some()
        );
        let report = ctx
            .data(|data| data.get_temp::<Arc<AgentSessionDeletionReport>>(receipt_id()))
            .expect("panic report");
        assert_eq!(report.failures.len(), 1);
        assert!(
            !app.finish_saved_session_deletion_for_shutdown(),
            "exception requires acknowledgement"
        );
    }

    #[test]
    fn recovery_paths_restore_as_a_blocking_notice_after_restart() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let report = AgentSessionDeletionReport {
            recoveries: vec![horizon_core::AgentSessionDeletionRecovery {
                key: horizon_core::AgentSessionKey::new(horizon_core::PanelKind::Claude, "synthetic"),
                session_id: "synthetic".into(),
                directory: "/synthetic/retained-bundle".into(),
                message: "Rollback incomplete".into(),
            }],
            ..Default::default()
        };
        app.retain_saved_session_deletion_notice(&report);
        app.saved_session_deletion = SavedSessionDeletionState::default();
        app.restore_saved_session_deletion_notice();
        assert!(
            app.saved_session_deletion
                .notice
                .as_ref()
                .expect("restored")
                .contains("/synthetic/retained-bundle")
        );
        assert!(app.host_dialog_open());
        assert!(!app.finish_saved_session_deletion_for_shutdown());
    }
}
