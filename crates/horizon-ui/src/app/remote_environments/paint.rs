//! Cached labels and action collection; no storage or provider calls while painting.

use egui::{Align, Context, Layout, RichText};
use horizon_core::{
    cloud_run::{CloudProvider, WorkerLifetime},
    remote_workspace::RemoteRuntimePhase,
};

use super::{InventoryAction, LoadError, RemoteEnvironmentSummary, RemoteEnvironments};
use crate::theme;

pub(super) struct InventoryRow {
    pub(super) summary: RemoteEnvironmentSummary,
    label: String,
    details: Vec<(&'static str, String)>,
}

impl InventoryRow {
    pub(super) fn new(summary: RemoteEnvironmentSummary) -> Self {
        let provider = match summary.provider {
            CloudProvider::Azure => "Azure",
            CloudProvider::RunPod => "RunPod",
            CloudProvider::LocalDocker => "Local Docker",
        };
        let label = format!(
            "{} · {} · {provider} / {}",
            summary.workspace_local_id, summary.repository, summary.profile
        );
        let lifetime = match summary.lifetime {
            WorkerLifetime::Persistent => "Persistent".into(),
            WorkerLifetime::TimeLimited { seconds } => format!("Time-limited ({seconds} seconds)"),
        };
        let mut details = vec![
            ("Workspace ID", summary.workspace_local_id.clone()),
            ("Owning session", summary.owning_session_id.clone()),
            ("Saved revision", summary.revision.to_string()),
            ("Runtime generation", summary.generation.to_string()),
            ("Saved panel intents", summary.panel_count.to_string()),
            ("Execution policy", lifetime),
        ];
        if let Some(workflow_id) = summary.workflow_id {
            details.push(("Workflow ID", workflow_id.to_string()));
        }
        if let Some(job_id) = summary.job_id {
            details.push(("Job ID", job_id.to_string()));
        }
        if let Some(identity) = &summary.worker_identity {
            details.push(("Exact resource ID", identity.resource_id.clone()));
        } else {
            details.push(("Exact resource ID", "No resource identity recorded".into()));
        }
        if let Some(checkpoint) = &summary.checkpoint {
            details.push((
                "Saved checkpoint",
                format!("{} (runtime {})", checkpoint.generation, checkpoint.runtime_generation),
            ));
            details.push(("Captured at (Unix ms)", checkpoint.captured_at_millis.to_string()));
            details.push(("Checkpoint commit", checkpoint.base_commit.as_str().into()));
        } else {
            details.push(("Saved checkpoint", "None recorded".into()));
        }
        Self {
            summary,
            label,
            details,
        }
    }
}

pub(super) fn show(ctx: &Context, state: &RemoteEnvironments) -> InventoryAction {
    let viewport = ctx.content_rect();
    let width = (viewport.width() - 64.0).clamp(260.0, 820.0);
    let mut action = InventoryAction::None;
    let id = egui::Id::new("remote-environments");
    let modal = egui::Modal::new(id)
        .area(egui::Modal::default_area(id).order(egui::Order::Debug))
        .frame(
            egui::Frame::default()
                .fill(theme::PANEL_BG())
                .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE()))
                .corner_radius(12)
                .inner_margin(16),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.heading("Remote Environments");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let close = ui.button("Close");
                    #[cfg(test)]
                    ui.ctx()
                        .data_mut(|data| data.insert_temp(egui::Id::new("inventory-close-test"), close.rect));
                    if close.clicked() {
                        action = InventoryAction::Close;
                    }
                });
            });
            ui.label(
                RichText::new(
                    "Saved inventory across all sessions. Provider checks are manual; uptime and cost are not checked.",
                )
                .color(theme::FG_DIM()),
            );
            ui.label(
                RichText::new("Closing this overview does not stop or delete remote work.").color(theme::FG_DIM()),
            );
            ui.separator();
            render_controls(ui, state, &mut action);
            let content_height = if state.page.as_ref().is_some_and(|page| !page.rows.is_empty()) {
                (viewport.height() - 96.0 - ui.min_rect().height()).max(80.0)
            } else {
                80.0
            };
            egui::ScrollArea::vertical()
                .id_salt("remote-environments-content")
                .max_height(content_height)
                .min_scrolled_height(content_height)
                .auto_shrink([false, false])
                .show(ui, |ui| render_content(ui, state, &mut action));
        });
    if modal.should_close() {
        InventoryAction::Close
    } else {
        action
    }
}

fn render_controls(ui: &mut egui::Ui, state: &RemoteEnvironments, action: &mut InventoryAction) {
    let idle = state.pending.is_none();
    ui.horizontal(|ui| {
        if ui.add_enabled(idle, egui::Button::new("Refresh saved page")).clicked() {
            *action = InventoryAction::Refresh;
        }
        let can_go_first =
            state.page_cursor.is_some() || state.failure.as_ref().is_some_and(|failure| failure.cursor.is_some());
        if ui
            .add_enabled(idle && can_go_first, egui::Button::new("First page"))
            .clicked()
        {
            *action = InventoryAction::First;
        }
        let has_next = state.page.as_ref().is_some_and(|page| page.next_cursor.is_some());
        if ui
            .add_enabled(idle && has_next, egui::Button::new("Next page"))
            .clicked()
        {
            *action = InventoryAction::Next;
        }
        if !idle {
            ui.label("Loading saved inventory…");
        }
    });
    if state.stop.is_pending() {
        ui.label("An explicitly confirmed Stop is pending. Closing this overview does not cancel it.");
    }
    if let Some(failure) = &state.failure {
        let message = match failure.error {
            LoadError::OpenStore => "Cannot open the remote inventory store. Check its permissions and format.",
            LoadError::ReadPage => "Cannot read this saved page. Its data may be invalid or incompatible.",
            LoadError::WorkerUnavailable => "The inventory reader could not finish.",
        };
        ui.colored_label(theme::PALETTE_YELLOW(), message);
        if state.page.is_some() {
            ui.colored_label(
                theme::PALETTE_YELLOW(),
                "Showing the previous saved page; it may be stale.",
            );
        }
        if ui.add_enabled(idle, egui::Button::new("Retry failed page")).clicked() {
            *action = InventoryAction::Retry;
        }
    }
}

fn render_content(ui: &mut egui::Ui, state: &RemoteEnvironments, action: &mut InventoryAction) {
    let Some(page) = &state.page else {
        return;
    };
    if page.rows.is_empty() {
        ui.add_space(12.0);
        ui.label(if state.page_cursor.is_none() {
            "No saved remote environments. Local session files are not required for this inventory."
        } else {
            "No more saved environments on this page. Return to the first page to refresh the inventory."
        });
        return;
    }
    ui.label(RichText::new("Environment / profile — saved setup phase").color(theme::FG_DIM()));
    for (index, row) in page.rows.iter().enumerate() {
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(state.selected == Some(index), &row.label).clicked() {
                *action = InventoryAction::Select(index);
            }
            ui.label(RichText::new(phase_label(row.summary.saved_phase)).color(theme::FG_DIM()));
        });
    }
    if let Some(row) = state.selected.and_then(|index| page.rows.get(index)) {
        render_observation(ui, state, action);
        super::stop::show(
            ui,
            &state.stop,
            &row.summary,
            state.pending.is_none() && !state.observation.is_pending(),
            action,
        );
        ui.separator();
        ui.strong("Saved environment details");
        ui.label(
            RichText::new("Ownership in this record is not permission to attach or manage a resource.")
                .color(theme::FG_DIM()),
        );
        egui::Grid::new("remote-environment-details")
            .num_columns(2)
            .max_col_width((ui.available_width() - 180.0).max(100.0))
            .show(ui, |ui| {
                for (label, value) in &row.details {
                    ui.label(*label);
                    ui.add(
                        egui::Label::new(RichText::new(value).monospace())
                            .wrap()
                            .selectable(true),
                    );
                    ui.end_row();
                }
            });
    }
}

fn render_observation(ui: &mut egui::Ui, state: &RemoteEnvironments, action: &mut InventoryAction) {
    let observation = &state.observation;
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        ui.strong("Provider status");
        let check = ui.add_enabled(
            state.pending.is_none() && !observation.is_pending() && !state.stop.is_pending(),
            egui::Button::new("Check provider status"),
        );
        #[cfg(test)]
        ui.ctx()
            .data_mut(|data| data.insert_temp(egui::Id::new("observation-check-test"), check.rect));
        if check.clicked() {
            *action = InventoryAction::Observe;
        }
        if observation.is_pending() {
            ui.label(observation.pending_label());
        }
    });
    if let Some(previous) = &observation.last_success {
        ui.label(previous.lifecycle);
        ui.horizontal_wrapped(|ui| {
            ui.label("Last successful check (UTC):");
            ui.monospace(&previous.checked_at);
        });
        if let Some(resource_id) = &previous.resource_id {
            ui.horizontal_wrapped(|ui| {
                ui.label("Observed resource ID:");
                ui.add(
                    egui::Label::new(RichText::new(resource_id).monospace())
                        .wrap()
                        .selectable(true),
                );
            });
        }
        ui.label(RichText::new("Point-in-time observation, not continuous monitoring.").color(theme::FG_DIM()));
    } else if !observation.is_pending() {
        ui.label("Provider status has not been checked.");
    }
    if let Some(error) = &observation.failure {
        ui.colored_label(theme::PALETTE_YELLOW(), error);
        if observation.last_success.is_some() {
            ui.colored_label(
                theme::PALETTE_YELLOW(),
                "Latest check failed; the previous observation may be stale.",
            );
        }
    }
    ui.label(
        RichText::new("Task, repository and SSH readiness are not checked. No resource or saved state is changed.")
            .color(theme::FG_DIM()),
    );
}

fn phase_label(phase: Option<RemoteRuntimePhase>) -> &'static str {
    match phase {
        None => "Dormant",
        Some(RemoteRuntimePhase::Provisioning) => "Provisioning",
        Some(RemoteRuntimePhase::Reconciling) => "Reconciling",
        Some(RemoteRuntimePhase::Materializing) => "Materializing",
        Some(RemoteRuntimePhase::Ready) => "Ready (saved, not live)",
        Some(RemoteRuntimePhase::Checkpointing) => "Checkpointing",
        Some(RemoteRuntimePhase::Cancelling) => "Cancelling",
        Some(RemoteRuntimePhase::Deleting) => "Deleting",
        Some(RemoteRuntimePhase::Failed) => "Failed",
        Some(RemoteRuntimePhase::Stopping { .. }) => "Stop requested (saved)",
        Some(RemoteRuntimePhase::Stopped { .. }) => "Stopped (saved, not live)",
    }
}
