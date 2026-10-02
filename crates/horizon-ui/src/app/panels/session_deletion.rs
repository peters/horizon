use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use egui::{Button, RichText};
use horizon_core::{AgentSessionBinding, AgentSessionCatalog, AgentSessionDeletionReport, AgentSessionKey, PanelId};

use super::session_rebind::finish_session_deletion;
use crate::app::HorizonApp;
use crate::theme;

#[derive(Clone, Default)]
pub(super) struct SessionDeletionUi {
    managing: bool,
    selected: Arc<HashSet<String>>,
    confirmation: Option<Arc<[AgentSessionBinding]>>,
    confirmation_all: bool,
    pub(super) message: Option<String>,
    details: Arc<[String]>,
    cleanup_details: Arc<[String]>,
    recovery_details: Arc<[String]>,
}

impl SessionDeletionUi {
    pub(super) fn restored(ctx: &egui::Context) -> Self {
        let mut state = Self::default();
        if let Some(report) = ctx.data(|data| data.get_temp::<Arc<AgentSessionDeletionReport>>(receipt_id())) {
            state.finish(&report);
        }
        state
    }
    pub(super) fn render_toolbar(&mut self, ui: &mut egui::Ui, options: &[AgentSessionBinding]) {
        if let Some(message) = &self.message {
            ui.label(RichText::new(message).size(12.0).color(theme::FG_SOFT()));
            if !self.details.is_empty() {
                ui.collapsing(
                    format!(
                        "Could not delete {} conversation{}",
                        self.details.len(),
                        if self.details.len() == 1 { "" } else { "s" }
                    ),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("deletion_failures")
                            .max_height(96.0)
                            .show(ui, |ui| {
                                for detail in self.details.iter() {
                                    ui.label(detail);
                                }
                            });
                    },
                );
            }
            if !self.cleanup_details.is_empty() {
                ui.collapsing("Files awaiting cleanup", |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("deletion_cleanup")
                        .max_height(96.0)
                        .show(ui, |ui| {
                            for detail in self.cleanup_details.iter() {
                                ui.label(detail);
                            }
                        });
                });
            }
            if !self.recovery_details.is_empty() {
                ui.collapsing("Saved history needs recovery", |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("deletion_recovery")
                        .max_height(96.0)
                        .show(ui, |ui| {
                            for detail in self.recovery_details.iter() {
                                ui.label(detail);
                            }
                        });
                });
            }
            ui.add_space(6.0);
        }
        let supported = options
            .first()
            .is_some_and(|binding| AgentSessionCatalog::supports_saved_session_deletion(binding.kind));
        if !supported {
            if !options.is_empty() {
                ui.label(
                    RichText::new("Saved conversation deletion is available for Codex and Claude.")
                        .size(12.0)
                        .color(theme::FG_SOFT()),
                );
            }
            return;
        }
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(self.managing, "Select sessions").clicked() {
                self.managing = !self.managing;
            }
            if self.managing {
                if ui.button("Select all").clicked() {
                    self.selected = Arc::new(options.iter().map(|binding| binding.session_id.clone()).collect());
                }
                if ui.button("Clear").clicked() {
                    Arc::make_mut(&mut self.selected).clear();
                }
                if ui
                    .add_enabled(
                        !self.selected.is_empty(),
                        Button::new(format!("Delete selected ({})", self.selected.len())),
                    )
                    .clicked()
                {
                    self.confirmation_all = false;
                    self.confirmation = Some(
                        options
                            .iter()
                            .filter(|binding| self.selected.contains(&binding.session_id))
                            .cloned()
                            .collect(),
                    );
                }
            } else if ui.button(format!("Delete all ({})…", options.len())).clicked() {
                self.confirmation_all = true;
                self.confirmation = Some(options.to_vec().into());
            }
        });
        ui.add_space(8.0);
    }

    pub(super) fn render_row_controls(&mut self, ui: &mut egui::Ui, binding: &AgentSessionBinding) {
        if !AgentSessionCatalog::supports_saved_session_deletion(binding.kind) {
            return;
        }
        if self.managing {
            let mut selected = self.selected.contains(&binding.session_id);
            if ui.checkbox(&mut selected, "Select").changed() {
                let selections = Arc::make_mut(&mut self.selected);
                if selected {
                    selections.insert(binding.session_id.clone());
                } else {
                    selections.remove(&binding.session_id);
                }
            }
        } else if ui
            .small_button(RichText::new("Delete").color(theme::PALETTE_RED()))
            .clicked()
        {
            self.confirmation_all = false;
            self.confirmation = Some(vec![binding.clone()].into());
        }
    }

    pub(super) fn render_confirmation(&mut self, ui: &mut egui::Ui) -> Option<Vec<AgentSessionBinding>> {
        let sessions = self.confirmation.clone()?;
        ui.label(
            RichText::new(format!(
                "Delete {} saved conversation{}?",
                sessions.len(),
                if sessions.len() == 1 { "" } else { "s" }
            ))
            .size(19.0)
            .strong(),
        );
        ui.add_space(10.0);
        if let Some(session) = sessions.first() {
            ui.label(format!("Provider: {}", session.kind.display_name()));
            render_deletion_folders(ui, &sessions);
        }
        ui.label("This permanently removes the selected conversations and their saved subagent history. Project files are kept. This cannot be undone.");
        if sessions.len() == 1 {
            ui.add_space(8.0);
            ui.label(sessions[0].label.as_deref().unwrap_or("Saved conversation"));
            ui.monospace(&sessions[0].session_id);
        }
        ui.add_space(14.0);
        let mut request = None;
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                self.confirmation = None;
            }
            if ui
                .add(Button::new(
                    RichText::new(format!("Permanently delete {}", sessions.len())).color(theme::PALETTE_RED()),
                ))
                .clicked()
            {
                request = Some(sessions.to_vec());
                self.confirmation = None;
            }
        });
        request
    }

    pub(super) fn reconcile_options(&mut self, options: &[AgentSessionBinding]) {
        Arc::make_mut(&mut self.selected).retain(|id| options.iter().any(|binding| &binding.session_id == id));
        if self.confirmation.as_ref().is_some_and(|sessions| {
            (self.confirmation_all && sessions.len() != options.len())
                || sessions.iter().any(|session| {
                    !options.iter().any(|binding| {
                        binding.kind == session.kind
                            && binding.session_id == session.session_id
                            && binding.cwd == session.cwd
                    })
                })
        }) {
            self.confirmation = None;
        }
    }

    pub(super) fn confirming(&self) -> bool {
        self.confirmation.is_some()
    }

    pub(super) fn finish(&mut self, report: &AgentSessionDeletionReport) {
        self.selected = Arc::new(HashSet::new());
        self.message = Some(format!(
            "Deleted {} conversation{}. {} failed.",
            report.deleted.len(),
            if report.deleted.len() == 1 { "" } else { "s" },
            report.failures.len()
        ));
        if !report.cleanup_warnings.is_empty()
            && let Some(message) = &mut self.message
        {
            let _ = write!(message, " {} awaiting file cleanup.", report.cleanup_warnings.len());
        }
        if !report.recoveries.is_empty()
            && let Some(message) = &mut self.message
        {
            let _ = write!(
                message,
                " Recovery needed for {} conversation{}.",
                report.recoveries.len(),
                if report.recoveries.len() == 1 { "" } else { "s" }
            );
        }
        self.recovery_details = report
            .recoveries
            .iter()
            .map(|recovery| {
                format!(
                    "{}: {} — {}",
                    recovery.session_id,
                    recovery.directory.display(),
                    recovery.message
                )
            })
            .collect();
        self.cleanup_details = report
            .cleanup_warnings
            .iter()
            .map(|warning| {
                format!(
                    "{}: {} — {}",
                    warning.session_id,
                    warning.directory.display(),
                    warning.message
                )
            })
            .collect();
        self.details = report
            .failures
            .iter()
            .map(|failure| format!("{}: {}", failure.session_id, failure.message))
            .collect();
    }
}

fn render_deletion_folders(ui: &mut egui::Ui, sessions: &[AgentSessionBinding]) {
    let folders: std::collections::BTreeSet<_> = sessions.iter().filter_map(|session| session.cwd.as_deref()).collect();
    let unknown = sessions.iter().any(|session| session.cwd.is_none());
    if folders.len() == 1 && !unknown {
        if let Some(folder) = folders.first() {
            ui.label(format!("Folder: {folder}"));
        }
        return;
    }
    ui.label(format!("Folders: {} recorded folders", folders.len()));
    egui::ScrollArea::vertical().max_height(96.0).show(ui, |ui| {
        for folder in folders {
            ui.label(folder);
        }
        if unknown {
            ui.label("Conversations with no recorded folder are also included.");
        }
    });
}

#[derive(Clone)]
struct DeletionJob {
    owner: PanelId,
    viewport: egui::ViewportId,
    reservation: Arc<horizon_core::AgentSessionDeletionReservation>,
    state: Arc<Mutex<DeletionProgress>>,
}

#[derive(Default)]
struct DeletionProgress {
    finished: bool,
    done: usize,
    total: usize,
    report: AgentSessionDeletionReport,
}

fn receipt_id() -> egui::Id {
    egui::Id::new("saved_conversation_deletion_receipt")
}

fn job_id() -> egui::Id {
    egui::Id::new("saved_conversation_deletion_job")
}

pub(super) fn deletion_progress(ctx: &egui::Context) -> Option<(usize, usize)> {
    let job = ctx.data(|data| data.get_temp::<DeletionJob>(job_id()))?;
    let state = job.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    Some((state.done, state.total))
}

impl HorizonApp {
    pub(super) fn render_saved_session_picker(
        &mut self,
        ctx: &egui::Context,
        panel_id: PanelId,
    ) -> Option<AgentSessionBinding> {
        self.poll_saved_session_deletion(ctx);
        if super::session_rebind::session_picker_panel(ctx) != Some(panel_id) {
            return None;
        }
        let options = self.session_rebind_options(panel_id);
        let binding = super::session_rebind::render_session_picker(ctx, panel_id, options);
        let binding = binding.filter(|chosen| {
            self.session_rebind_options(panel_id).iter().any(|current| {
                current.kind == chosen.kind && current.session_id == chosen.session_id && current.cwd == chosen.cwd
            })
        });
        if let Some(sessions) = take_deletion_request(ctx) {
            self.start_saved_session_deletion(ctx, panel_id, sessions);
        }
        binding
    }

    pub(in crate::app) fn poll_saved_session_deletion(&mut self, ctx: &egui::Context) {
        let Some(job) = ctx.data(|data| data.get_temp::<DeletionJob>(job_id())) else {
            return;
        };
        let mut state = job.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.finished {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        let report = std::mem::take(&mut state.report);
        drop(state);
        ctx.data_mut(|data| data.remove::<DeletionJob>(job_id()));
        let owner = job.owner;
        let viewport = job.viewport;
        self.session_catalog.remove_deleted_sessions(&report);
        drop(job);
        ctx.data_mut(|data| data.insert_temp(receipt_id(), Arc::new(report.clone())));
        self.session_catalog_refresh = None;
        self.last_session_catalog_refresh = None;
        let options = self.session_rebind_options(owner);
        finish_session_deletion(ctx, owner, viewport, options, &report);
    }

    pub(super) fn start_saved_session_deletion(
        &mut self,
        ctx: &egui::Context,
        owner: PanelId,
        requested: Vec<AgentSessionBinding>,
    ) {
        if deletion_progress(ctx).is_some() {
            return;
        }
        let allowed = self.session_rebind_options(owner);
        let mut unavailable = Vec::new();
        let sessions: Arc<[_]> = requested
            .into_iter()
            .filter(|requested| {
                let eligible = allowed
                    .iter()
                    .any(|binding| binding.kind == requested.kind && binding.session_id == requested.session_id);
                if !eligible {
                    unavailable.push(horizon_core::AgentSessionDeletionFailure {
                        session_id: requested.session_id.clone(),
                        message: "This conversation is now attached to a panel or no longer available".into(),
                    });
                }
                eligible
            })
            .collect();
        let protected = self
            .board
            .panels
            .iter()
            .filter_map(|panel| panel.session_id().map(|id| AgentSessionKey::new(panel.kind, id)))
            .collect();
        let reservation = match horizon_core::reserve_saved_session_deletions(&sessions) {
            Ok(reservation) => Arc::new(reservation),
            Err(error) => {
                let report = failed_deletion_start(&sessions, unavailable, &error.to_string());
                ctx.data_mut(|data| data.insert_temp(receipt_id(), Arc::new(report.clone())));
                finish_session_deletion(ctx, owner, ctx.viewport_id(), allowed, &report);
                return;
            }
        };
        ctx.data_mut(|data| data.remove::<Arc<AgentSessionDeletionReport>>(receipt_id()));
        let catalog = self.session_catalog.clone();
        let job = DeletionJob {
            owner,
            viewport: ctx.viewport_id(),
            reservation,
            state: Arc::new(Mutex::new(DeletionProgress {
                total: sessions.len(),
                report: AgentSessionDeletionReport {
                    failures: unavailable,
                    ..Default::default()
                },
                ..Default::default()
            })),
        };
        let state = Arc::clone(&job.state);
        let reservation = Arc::clone(&job.reservation);
        let worker_sessions = Arc::clone(&sessions);
        let repaint = ctx.clone();
        let worker = std::thread::Builder::new()
            .name("conversation-deletion".into())
            .spawn(move || {
                for session in worker_sessions.iter() {
                    let report = reservation.delete_saved_sessions(&catalog, std::slice::from_ref(session), &protected);
                    let mut progress = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    progress.done += 1;
                    progress.report.deleted.extend(report.deleted);
                    progress.report.failures.extend(report.failures);
                    progress.report.cleanup_warnings.extend(report.cleanup_warnings);
                    progress.report.recoveries.extend(report.recoveries);
                    drop(progress);
                    repaint.request_repaint();
                }
                drop(reservation);
                state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).finished = true;
                repaint.request_repaint();
            });
        if let Err(error) = worker {
            let unavailable = std::mem::take(
                &mut job
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .report
                    .failures,
            );
            let report = failed_deletion_start(&sessions, unavailable, &error.to_string());
            ctx.data_mut(|data| data.insert_temp(receipt_id(), Arc::new(report.clone())));
            finish_session_deletion(ctx, owner, ctx.viewport_id(), allowed, &report);
        } else {
            ctx.data_mut(|data| data.insert_temp(job_id(), job));
        }
    }
}

fn failed_deletion_start(
    sessions: &[AgentSessionBinding],
    mut failures: Vec<horizon_core::AgentSessionDeletionFailure>,
    message: &str,
) -> AgentSessionDeletionReport {
    let mut seen = HashSet::new();
    failures.retain(|failure| seen.insert(failure.session_id.clone()));
    failures.extend(
        sessions
            .iter()
            .filter(|session| seen.insert(session.session_id.clone()))
            .map(|session| horizon_core::AgentSessionDeletionFailure {
                session_id: session.session_id.clone(),
                message: message.to_owned(),
            }),
    );
    AgentSessionDeletionReport {
        failures,
        ..Default::default()
    }
}

fn request_id() -> egui::Id {
    egui::Id::new("saved_conversation_deletion_request")
}
pub(super) fn queue_deletion_request(ctx: &egui::Context, sessions: Vec<AgentSessionBinding>) {
    ctx.data_mut(|data| data.insert_temp(request_id(), sessions));
}
pub(super) fn take_deletion_request(ctx: &egui::Context) -> Option<Vec<AgentSessionBinding>> {
    ctx.data_mut(|data| data.remove_temp(request_id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;

    #[test]
    fn batch_start_failure_counts_every_conversation_and_preserves_preflight_failures() {
        let sessions: Vec<_> = (0..10)
            .map(|index| {
                AgentSessionBinding::new(
                    horizon_core::PanelKind::Codex,
                    format!("session-{index}"),
                    None,
                    None,
                    None,
                )
            })
            .collect();
        let unavailable = vec![horizon_core::AgentSessionDeletionFailure {
            session_id: "already-attached".into(),
            message: "Attached to an open panel".into(),
        }];
        let mut duplicated = sessions.clone();
        duplicated.push(sessions[0].clone());
        for error in ["Reservation conflict", "Worker could not start"] {
            let report = failed_deletion_start(&duplicated, unavailable.clone(), error);
            assert!(report.deleted.is_empty());
            assert_eq!(report.failures.len(), 11);
            assert_eq!(report.failures[0].session_id, "already-attached");
            assert_eq!(report.failures[0].message, "Attached to an open panel");
            for session in &sessions {
                assert!(
                    report
                        .failures
                        .iter()
                        .any(|failure| failure.session_id == session.session_id && failure.message == error)
                );
            }
            let mut ui = SessionDeletionUi::default();
            ui.finish(&report);
            assert_eq!(ui.message.as_deref(), Some("Deleted 0 conversations. 11 failed."));
        }
    }

    #[test]
    fn recovery_receipt_does_not_claim_success_or_resumable_failure() {
        let ctx = egui::Context::default();
        let report = AgentSessionDeletionReport {
            recoveries: vec![horizon_core::AgentSessionDeletionRecovery {
                key: AgentSessionKey::new(horizon_core::PanelKind::Claude, "synthetic-id"),
                session_id: "synthetic-id".into(),
                directory: "/sample/recovery".into(),
                message: "Restore transcript first".into(),
            }],
            ..Default::default()
        };
        ctx.data_mut(|data| data.insert_temp(receipt_id(), Arc::new(report)));
        let restored = SessionDeletionUi::restored(&ctx);
        assert_eq!(
            restored.message.as_deref(),
            Some("Deleted 0 conversations. 0 failed. Recovery needed for 1 conversation.")
        );
        assert!(restored.details.is_empty());
        assert!(restored.recovery_details[0].contains("/sample/recovery"));
    }

    #[test]
    fn scope_changes_cancel_confirmation_without_silently_reducing_it() {
        let a = AgentSessionBinding::new(
            horizon_core::PanelKind::Claude,
            "a".into(),
            Some("/sample/a".into()),
            None,
            None,
        );
        let b = AgentSessionBinding::new(
            horizon_core::PanelKind::Claude,
            "b".into(),
            Some("/sample/b".into()),
            None,
            None,
        );
        let mut state = SessionDeletionUi {
            selected: Arc::new(HashSet::from(["a".into(), "b".into()])),
            confirmation: Some(vec![a.clone(), b.clone()].into()),
            ..Default::default()
        };
        state.reconcile_options(std::slice::from_ref(&a));
        assert!(!state.confirming());
        assert_eq!(state.selected.as_ref(), &HashSet::from(["a".into()]));
        state.confirmation = Some(vec![b.clone()].into());
        let changed = AgentSessionBinding {
            cwd: Some("/sample/changed".into()),
            ..b
        };
        state.reconcile_options(&[a, changed]);
        assert!(!state.confirming());
    }

    #[test]
    fn delete_all_confirmation_cancels_on_scope_growth_but_single_selection_remains_exact() {
        let a = AgentSessionBinding::new(horizon_core::PanelKind::Claude, "a".into(), None, None, None);
        let b = AgentSessionBinding::new(horizon_core::PanelKind::Claude, "b".into(), None, None, None);
        let mut state = SessionDeletionUi {
            confirmation: Some(vec![a.clone()].into()),
            confirmation_all: true,
            ..Default::default()
        };
        state.reconcile_options(std::slice::from_ref(&a));
        assert!(state.confirming());
        state.reconcile_options(&[a.clone(), b.clone()]);
        assert!(!state.confirming());
        state.confirmation_all = false;
        state.confirmation = Some(vec![a.clone()].into());
        state.reconcile_options(&[a.clone(), b]);
        assert_eq!(state.confirmation.as_deref(), Some(std::slice::from_ref(&a)));
    }

    #[test]
    fn cleanup_warning_is_retained_separately_from_failed_deletions() {
        let ctx = egui::Context::default();
        let report = AgentSessionDeletionReport {
            deleted: vec![AgentSessionKey::new(horizon_core::PanelKind::Claude, "synthetic-id")],
            cleanup_warnings: vec![horizon_core::AgentSessionDeletionCleanupWarning {
                session_id: "synthetic-id".into(),
                directory: "/sample/recovery-bundle".into(),
                message: "Synthetic cleanup error".into(),
            }],
            ..Default::default()
        };
        ctx.data_mut(|data| data.insert_temp(receipt_id(), Arc::new(report)));
        let restored = SessionDeletionUi::restored(&ctx);
        assert_eq!(
            restored.message.as_deref(),
            Some("Deleted 1 conversation. 0 failed. 1 awaiting file cleanup.")
        );
        assert!(restored.details.is_empty());
        assert_eq!(restored.cleanup_details.len(), 1);
        assert!(restored.cleanup_details[0].contains("/sample/recovery-bundle"));
        assert!(restored.cleanup_details[0].contains("Synthetic cleanup error"));
    }

    #[test]
    fn completion_report_survives_dismissal_and_reopening() {
        let ctx = egui::Context::default();
        let report = AgentSessionDeletionReport {
            failures: vec![horizon_core::AgentSessionDeletionFailure {
                session_id: "synthetic-id".into(),
                message: "Provider unavailable".into(),
            }],
            ..Default::default()
        };
        ctx.data_mut(|data| data.insert_temp(receipt_id(), Arc::new(report)));
        let restored = SessionDeletionUi::restored(&ctx);
        assert!(
            restored
                .message
                .as_deref()
                .is_some_and(|message| message.contains("1 failed"))
        );
        assert!(restored.details[0].contains("Provider unavailable"));
    }

    #[test]
    fn management_controls_wrap_and_bulk_errors_remain_bounded() {
        let ctx = egui::Context::default();
        let options: Vec<_> = (0..16)
            .map(|index| {
                AgentSessionBinding::new(
                    horizon_core::PanelKind::Codex,
                    format!("synthetic-{index}"),
                    None,
                    None,
                    None,
                )
            })
            .collect();
        let mut state = SessionDeletionUi {
            managing: true,
            selected: Arc::new(options.iter().map(|binding| binding.session_id.clone()).collect()),
            ..Default::default()
        };
        state.finish(&AgentSessionDeletionReport {
            failures: (0..500)
                .map(|index| horizon_core::AgentSessionDeletionFailure {
                    session_id: format!("synthetic-{index}"),
                    message: "Synthetic provider failure".into(),
                })
                .collect(),
            ..Default::default()
        });
        state.selected = Arc::new(options.iter().map(|binding| binding.session_id.clone()).collect());
        let output = ctx
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 700.0))),
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(280.0);
                    state.render_toolbar(ui, &options);
                    assert!(ui.cursor().top() < 180.0, "failure summary stays bounded");
                },
            )
            .discard_textures();
        let text = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.job.text == "Delete selected (16)" => Some(text),
                _ => None,
            })
            .expect("delete action visible");
        assert!(
            text.pos.x + text.galley.size().x <= 310.0,
            "destructive action remains inside narrow viewport"
        );
    }

    #[test]
    fn confirmation_cancel_does_not_enqueue_deletion() {
        let ctx = egui::Context::default();
        let mut state = SessionDeletionUi {
            confirmation: Some(
                vec![AgentSessionBinding::new(
                    horizon_core::PanelKind::Codex,
                    "synthetic-id".into(),
                    None,
                    None,
                    None,
                )]
                .into(),
            ),
            ..Default::default()
        };
        let run = |state: &mut SessionDeletionUi, events| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    assert!(state.render_confirmation(ui).is_none());
                },
            )
            .discard_textures()
        };
        let output = run(&mut state, Vec::new());
        let at = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.job.text == "Cancel" => {
                    Some(text.pos + text.galley.size() / 2.0)
                }
                _ => None,
            })
            .expect("cancel visible");
        for pressed in [true, false] {
            run(
                &mut state,
                vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert!(!state.confirming());
        assert!(take_deletion_request(&ctx).is_none());
    }

    #[test]
    fn frame_lifecycle_finishes_deletion_without_renderable_panels() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let ctx = egui::Context::default();
        assert!(app.board.panels.is_empty());
        let binding = AgentSessionBinding::new(
            horizon_core::PanelKind::Codex,
            "01a0f8a8-d7e2-7810-9e6b-999999999999".into(),
            None,
            None,
            None,
        );
        let key = AgentSessionKey::new(binding.kind, &binding.session_id);
        let reservation =
            horizon_core::reserve_saved_session_deletions(std::slice::from_ref(&binding)).expect("reserve");
        ctx.data_mut(|data| {
            data.insert_temp(
                job_id(),
                DeletionJob {
                    owner: PanelId(42),
                    viewport: egui::ViewportId::ROOT,
                    reservation: Arc::new(reservation),
                    state: Arc::new(Mutex::new(DeletionProgress {
                        finished: true,
                        done: 1,
                        total: 1,
                        report: AgentSessionDeletionReport {
                            deleted: vec![key.clone()],
                            ..Default::default()
                        },
                    })),
                },
            )
        });
        assert!(horizon_core::saved_session_deletion_pending(
            binding.kind,
            &binding.session_id
        ));
        let _ = ctx
            .run_ui(egui::RawInput::default(), |_| {
                app.process_frame_inputs(&ctx);
            })
            .discard_textures();
        assert!(deletion_progress(&ctx).is_none());
        assert!(!horizon_core::saved_session_deletion_pending(
            binding.kind,
            &binding.session_id
        ));
        let report = ctx
            .data(|data| data.get_temp::<Arc<AgentSessionDeletionReport>>(receipt_id()))
            .expect("receipt");
        assert_eq!(report.deleted, vec![key]);
        assert!(app.board.panels.is_empty());
    }

    #[test]
    fn bulk_confirmation_names_every_folder_and_unknown_scope() {
        let ctx = egui::Context::default();
        let mut sessions: Vec<_> = [Some("/sample/a"), Some("/sample/b"), None]
            .into_iter()
            .enumerate()
            .map(|(i, cwd)| {
                AgentSessionBinding::new(
                    horizon_core::PanelKind::Codex,
                    format!("synthetic-{i}"),
                    cwd.map(str::to_owned),
                    None,
                    None,
                )
            })
            .collect();
        let a = sessions[0].cwd.clone().expect("folder a");
        let b = sessions[1].cwd.clone().expect("folder b");
        let mut state = SessionDeletionUi {
            confirmation: Some(std::mem::take(&mut sessions).into()),
            ..Default::default()
        };
        let output = ctx
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 700.0))),
                    ..Default::default()
                },
                |ui| {
                    assert!(state.render_confirmation(ui).is_none());
                },
            )
            .discard_textures();
        let text: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                _ => None,
            })
            .collect();
        assert!(text.contains(&"Folders: 2 recorded folders"));
        assert!(text.contains(&a.as_str()));
        assert!(text.contains(&b.as_str()));
        assert!(text.contains(&"Conversations with no recorded folder are also included."));
        assert!(!text.iter().any(|text| text.starts_with("Folder: ")));
    }
}
