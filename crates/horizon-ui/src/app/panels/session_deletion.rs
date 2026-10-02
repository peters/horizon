use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
            let response = ui.checkbox(&mut selected, "Select");
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Checkbox,
                    response.enabled(),
                    selected,
                    format!("Select conversation {}", binding.session_id),
                )
            });
            if response.changed() {
                let selections = Arc::make_mut(&mut self.selected);
                if selected {
                    selections.insert(binding.session_id.clone());
                } else {
                    selections.remove(&binding.session_id);
                }
            }
        } else {
            let response = ui.small_button(RichText::new("Delete").color(theme::PALETTE_RED()));
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    response.enabled(),
                    format!("Delete conversation {}", binding.session_id),
                )
            });
            if response.clicked() {
                self.confirmation_all = false;
                self.confirmation = Some(vec![binding.clone()].into());
            }
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
        let eligible_ids: HashSet<_> = options.iter().map(|binding| binding.session_id.as_str()).collect();
        Arc::make_mut(&mut self.selected).retain(|id| eligible_ids.contains(id.as_str()));
        if self.confirmation.as_ref().is_some_and(|sessions| {
            if self.confirmation_all {
                return !AgentSessionBinding::same_saved_session_scope(sessions, options);
            }
            let eligible_scopes: HashSet<_> = options
                .iter()
                .map(|binding| (binding.kind, binding.session_id.as_str(), binding.cwd.as_deref()))
                .collect();
            sessions.iter().any(|session| {
                !eligible_scopes.contains(&(session.kind, session.session_id.as_str(), session.cwd.as_deref()))
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

#[derive(Clone)]
struct PickerCatalogCache {
    owner: PanelId,
    kind: horizon_core::PanelKind,
    revision: (Option<Instant>, Option<Instant>, u64),
    panels: Arc<[PickerPanelScope]>,
}

struct PickerPanelScope {
    id: PanelId,
    session: Option<String>,
    cwd: Option<PathBuf>,
}

impl PickerCatalogCache {
    fn matches_panels<'a>(
        &self,
        mut panels: impl Iterator<Item = (PanelId, Option<&'a str>, Option<&'a Path>)>,
    ) -> bool {
        for panel in self.panels.iter() {
            if panels.next() != Some((panel.id, panel.session.as_deref(), panel.cwd.as_deref())) {
                return false;
            }
        }
        panels.next().is_none()
    }
}

pub(super) fn deletion_progress(ctx: &egui::Context) -> Option<(usize, usize)> {
    let job = ctx.data(|data| data.get_temp::<DeletionJob>(job_id()))?;
    let state = job.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    Some((state.done, state.total))
}

impl HorizonApp {
    fn picker_options_update(&self, ctx: &egui::Context, panel_id: PanelId) -> Option<Vec<AgentSessionBinding>> {
        let kind = self.board.panel(panel_id)?.kind;
        let revision = (
            self.session_catalog_refresh.last_full_refresh,
            self.session_catalog_refresh.picker_times.get(&kind).copied(),
            AgentSessionCatalog::pending_deletion_revision(),
        );
        let scopes = || {
            self.board
                .panels
                .iter()
                .filter(|panel| panel.kind == kind)
                .map(|panel| {
                    let session = if panel.id == panel_id {
                        panel
                            .session_binding
                            .as_ref()
                            .map(|binding| binding.session_id.as_str())
                    } else {
                        panel.session_id()
                    };
                    (panel.id, session, panel.launch_cwd.as_deref())
                })
        };
        let cache_id = egui::Id::new(("saved_session_picker_catalog", ctx.viewport_id()));
        if ctx
            .data(|data| data.get_temp::<PickerCatalogCache>(cache_id))
            .is_some_and(|cache| {
                cache.owner == panel_id
                    && cache.kind == kind
                    && cache.revision == revision
                    && cache.matches_panels(scopes())
            })
        {
            return None;
        }
        let options = self.session_rebind_options(panel_id);
        let panels = scopes()
            .map(|(id, session, cwd)| PickerPanelScope {
                id,
                session: session.map(str::to_owned),
                cwd: cwd.map(Path::to_path_buf),
            })
            .collect();
        ctx.data_mut(|data| {
            data.insert_temp(
                cache_id,
                PickerCatalogCache {
                    owner: panel_id,
                    kind,
                    revision,
                    panels,
                },
            )
        });
        Some(options)
    }

    pub(super) fn render_saved_session_picker(
        &mut self,
        ctx: &egui::Context,
        panel_id: PanelId,
    ) -> Option<AgentSessionBinding> {
        self.poll_saved_session_deletion(ctx);
        if super::session_rebind::session_picker_panel(ctx) != Some(panel_id) {
            return None;
        }
        let kind = self.board.panels.iter().find(|panel| panel.id == panel_id)?.kind;
        self.refresh_session_catalog_for_picker(ctx, kind);
        let options = self.picker_options_update(ctx, panel_id);
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
        self.session_catalog_refresh.receiver = None;
        self.session_catalog_refresh.provider = None;
        self.session_catalog_refresh.picker_times.clear();
        self.session_catalog_refresh.last_full_refresh = None;
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
mod tests;
