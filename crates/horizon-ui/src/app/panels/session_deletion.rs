use std::collections::HashSet;
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
    pub(super) message: Option<String>,
    details: Arc<[String]>,
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
                    format!("{} conversations could not be deleted", self.details.len()),
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
                    self.confirmation = Some(
                        options
                            .iter()
                            .filter(|binding| self.selected.contains(&binding.session_id))
                            .cloned()
                            .collect(),
                    );
                }
            } else if ui.button(format!("Delete all ({})…", options.len())).clicked() {
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
            ui.label(format!(
                "Folder: {}",
                session.cwd.as_deref().unwrap_or("All folders in this list")
            ));
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
        self.details = report
            .failures
            .iter()
            .map(|failure| format!("{}: {}", failure.session_id, failure.message))
            .collect();
    }
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
        let binding = super::session_rebind::render_session_picker(ctx, panel_id);
        if let Some(sessions) = take_deletion_request(ctx) {
            self.start_saved_session_deletion(ctx, panel_id, sessions);
        }
        binding
    }

    pub(super) fn poll_saved_session_deletion(&mut self, ctx: &egui::Context) {
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
        drop(job);
        ctx.data_mut(|data| data.insert_temp(receipt_id(), Arc::new(report.clone())));
        self.session_catalog.remove_deleted_sessions(&report);
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
        let sessions: Vec<_> = requested
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
                let report = AgentSessionDeletionReport {
                    failures: vec![horizon_core::AgentSessionDeletionFailure {
                        session_id: "Selection".into(),
                        message: error.to_string(),
                    }],
                    ..Default::default()
                };
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
        let repaint = ctx.clone();
        let worker = std::thread::Builder::new()
            .name("conversation-deletion".into())
            .spawn(move || {
                for session in sessions {
                    let report = reservation.delete_saved_sessions(&catalog, &[session], &protected);
                    let mut progress = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    progress.done += 1;
                    progress.report.deleted.extend(report.deleted);
                    progress.report.failures.extend(report.failures);
                    drop(progress);
                    repaint.request_repaint();
                }
                drop(reservation);
                state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).finished = true;
                repaint.request_repaint();
            });
        if let Err(error) = worker {
            let report = AgentSessionDeletionReport {
                failures: vec![horizon_core::AgentSessionDeletionFailure {
                    session_id: "Worker".into(),
                    message: error.to_string(),
                }],
                ..Default::default()
            };
            ctx.data_mut(|data| data.insert_temp(receipt_id(), Arc::new(report.clone())));
            finish_session_deletion(ctx, owner, ctx.viewport_id(), allowed, &report);
        } else {
            ctx.data_mut(|data| data.insert_temp(job_id(), job));
        }
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
}
