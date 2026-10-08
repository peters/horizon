use super::{Confirmation, DELETED_RESOURCES_MESSAGE, HorizonApp, Stage, cloud_runtime, lifecycle::Action};
use crate::app::cloud_panel::runtime::{action_button, danger_button};
use crate::theme;
use egui::{RichText, Vec2};
use horizon_core::{Board, cloud_panel::CloudGroup};
mod body;
mod cost;
mod drawer;
mod machine;
mod output;
pub(super) mod placement;
mod rebuild;
mod section;
mod self_stop;
mod sizing;
mod status;
mod steps;
pub(in crate::app::cloud_panel) mod strip;
#[cfg(test)]
mod tests;
mod timeline;
mod view;
pub(super) mod wording;
pub(super) use drawer::Tab;
pub(in crate::app::cloud_panel) use output::forget_log_heights;
#[cfg(test)]
pub(in crate::app::cloud_panel) use output::{log_height_cache_present, remember_log_height_cache};
use sizing::profile_details;
pub(super) use status::{DiagnosisKey, Failure, Status};

/// Terminal liveness for the Connections tab. A running process alone does not
/// confirm the remote connection; browser and desktop connections are on their panels.
pub(super) fn attachment_summary(group: &CloudGroup, runtime: &super::Runtime, board: &Board) -> String {
    if runtime.needs_attach
        || !runtime.pending_session_attachments.is_empty()
        || !runtime.pending_member_attachments.is_empty()
        || !runtime.pending_browser_attachments.is_empty()
    {
        return "Sessions: attaching…".into();
    }
    let occupancy = view::occupancy(group, board);
    if occupancy.terminals == 0 {
        return "Terminals: none attached".into();
    }
    format!("Terminals: {}/{} running", occupancy.running, occupancy.terminals)
}

/// A confirmation asked for from the header opens Manage with its button in view.
fn reveal_confirmation(runtime: &mut super::Runtime, button: &egui::Response) {
    if std::mem::take(&mut runtime.reveal_confirmation) {
        button.scroll_to_me(Some(egui::Align::Center));
    }
}

fn accent_button<'a>(ui: &egui::Ui, label: &'a str) -> egui::Button<'a> {
    action_button(label).min_size(Vec2::new(ui.available_width(), 34.0))
}

fn deleted_runtime_actions(ui: &mut egui::Ui, id: u32, runtime: &mut super::Runtime) -> Option<Action> {
    ui.label(DELETED_RESOURCES_MESSAGE);
    if let Some(elapsed) = runtime.progress.ended_in(Stage::Deleted) {
        ui.small(format!("Deleted in {}", cloud_runtime::progress::duration(elapsed)));
    }
    if let Some(error) = runtime
        .error
        .as_ref()
        .filter(|error| error.as_str() != DELETED_RESOURCES_MESSAGE)
    {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    if runtime.receiver.is_some() {
        ui.label("Redeploying cloud…");
        progress_output(ui, id, runtime);
        return None;
    }
    if runtime.confirmation == Confirmation::Redeploy {
        ui.label("Redeploy this cloud? A new worker and managed workspace storage will be allocated. Files from the deleted worker are gone.");
        let redeploy = ui.add(accent_button(ui, "Redeploy cloud"));
        reveal_confirmation(runtime, &redeploy);
        let action = redeploy.clicked().then_some(Action::Deploy);
        if ui.add(action_button("Keep removed")).clicked() {
            runtime.confirmation = Confirmation::None;
        }
        return action;
    }
    if ui.add(accent_button(ui, "Redeploy cloud…")).clicked() {
        runtime.confirmation = Confirmation::Redeploy;
    }
    ui.add(danger_button("Remove cloud"))
        .clicked()
        .then_some(Action::Remove)
}

/// Manage's lifecycle actions. Sharing and the desktop viewer are in Connections, which
/// keeps them during a rebuild or resize too.
fn runtime_actions(ui: &mut egui::Ui, id: u32, runtime: &mut super::Runtime) -> Option<Action> {
    if deleting(runtime) {
        deletion_progress(ui, id, runtime);
        return None;
    }
    if deleted_or_redeploying(runtime) {
        return deleted_runtime_actions(ui, id, runtime);
    }
    if runtime.state.as_ref().is_some_and(|state| {
        matches!(
            state.operation,
            horizon_core::cloud_runtime::CreateState::Terminated { .. }
        )
    }) {
        progress_output(ui, id, runtime);
        ui.label("Worker deleted. Finish managed workspace storage cleanup to stop its storage charges.");
        return if runtime.receiver.is_some() {
            ui.spinner();
            None
        } else {
            deletion_action(ui, runtime)
        };
    }
    if rebuild::in_progress(runtime) {
        rebuild::progress(ui, id, runtime);
        return None;
    }
    if let Some(next) = rebuild::pending_notice(ui, runtime) {
        return Some(next);
    }
    if confirming_stop(runtime) {
        return stop_confirmation_card(ui, runtime);
    }
    // Steps and progress are in the header, body and Overview; Manage keeps the actions,
    // what went wrong and what the last rebuild reported.
    rebuild::notes(ui, runtime);
    for error in runtime.error.iter().chain(&runtime.remote_release_error) {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    if let Some(action) = super::resize::controls(ui, id, runtime) {
        return Some(action);
    }
    if runtime.resize.pending.is_some() {
        return None;
    }
    if runtime.remote_release.is_some() {
        ui.spinner();
        ui.label("Releasing remote devices…");
        return None;
    }
    if runtime.checking_provider()
        || (runtime.receiver.is_none() && runtime.state.as_ref().is_some_and(super::Runtime::needs_provider_check))
    {
        return recovery_actions(ui, runtime);
    }
    let removable = !runtime.state_unavailable
        && runtime.receiver.is_none()
        && runtime
            .state
            .as_ref()
            .is_none_or(|state| state.operation == horizon_core::cloud_runtime::CreateState::Prepared);
    let mut action = operation_action(ui, runtime);

    if runtime.stage == Some(Stage::Ready) {
        action = ready_actions(ui, runtime).or(action);
    }
    // Under the cloud's own action, as the way out.
    if removable && ui.add(danger_button("Remove cloud")).clicked() {
        return Some(Action::Remove);
    }
    bound_provider_check(ui, runtime)
        .or_else(|| deletion_action(ui, runtime))
        .or(action)
}

/// The one lifecycle action the cloud's state allows: cancel, confirm a stop, resume,
/// read an unreadable record again, or deploy/reconnect.
fn operation_action(ui: &mut egui::Ui, runtime: &super::Runtime) -> Option<Action> {
    let mut action = None;
    if runtime.receiver.is_some() && runtime.stage != Some(Stage::Ready) {
        if ui.add(danger_button("Cancel operation")).clicked()
            && let Some(cancel) = &runtime.cancel
        {
            cancel.cancel();
        }
    } else if runtime.stage == Some(Stage::Stopping) {
        ui.label("Stop requested; provider confirmation is pending.");
        if ui.add(action_button("Reconcile stop")).clicked() {
            action = Some(Action::Stop);
        }
    } else if runtime.stage == Some(Stage::Stopped) {
        ui.label(wording::stopped_note(runtime));
        if ui.add(action_button("Resume worker")).clicked() {
            action = Some(Action::Resume);
        }
    } else if runtime.state_unavailable {
        // Deploy reads the record first and stops again while it stays unreadable, so it
        // never deploys over a record it cannot see.
        ui.small("Horizon continues only once it can read this cloud's deployment record.");
        if ui.add(accent_button(ui, "Read record again")).clicked() {
            action = Some(Action::Deploy);
        }
    } else if ui
        .add(accent_button(
            ui,
            if runtime.state.is_some() {
                "Reconnect cloud"
            } else {
                "Deploy cloud"
            },
        ))
        .clicked()
    {
        action = Some(Action::Deploy);
    }
    action
}

impl super::Runtime {
    /// A lifetime total requires a reported billing period, not just an empty response.
    fn total_cost(&self, now: std::time::SystemTime) -> Option<cloud_runtime::cost::TotalCost> {
        let total = self
            .billing
            .total(self.state.as_ref()?.worker.as_ref()?, now)
            .filter(|total| total.billed_through.is_some())?;
        // A deleted worker's snapshot can still read as running: its live estimate stops,
        // and billing reports what it was charged at the next refresh.
        Some(if self.worker_terminated() {
            cloud_runtime::cost::TotalCost {
                estimated: 0.0,
                ..total
            }
        } else {
            total
        })
    }

    /// Frame header text, for example `$0.83 run · $4.20 total`.
    pub(in crate::app::cloud_panel) fn cost_badge(&self, now: std::time::SystemTime) -> Option<String> {
        cloud_runtime::cost::badge(self.current_run_cost(now).as_ref(), self.total_cost(now).as_ref())
    }
}

/// The current run's live cost while Ready, otherwise the worker's hourly rate,
/// then the cost since creation.
fn worker_cost(ui: &mut egui::Ui, runtime: &super::Runtime, now: std::time::SystemTime) {
    if let Some(run) = runtime.current_run_cost(now) {
        ui.label(run.summary())
            .on_hover_text("Estimated from the worker's hourly rate and the time since it last started.");
    } else if let Some(rate) = runtime
        .state
        .as_ref()
        .and_then(|state| state.worker.as_ref())
        .and_then(cloud_runtime::cost::hourly_rate)
        // A stopped or deleted worker's last rate is not what bills now.
        .filter(|_| !cost::compute_idle(runtime))
    {
        ui.label(format!("Worker rate: {}", cloud_runtime::cost::format_rate(rate)));
    }
    total_cost(ui, runtime, now);
}

/// Billing failures only hide the total; the run above never depends on billing.
fn total_cost(ui: &mut egui::Ui, runtime: &super::Runtime, now: std::time::SystemTime) {
    if let Some(total) = runtime.total_cost(now) {
        ui.label(total.summary()).on_hover_ui(|ui| {
            ui.label(runtime.billing.explanation(&total, std::time::Instant::now()));
        });
    } else if let Some(error) = runtime.billing.error() {
        ui.label(format!("Total unavailable: {error}"))
            .on_hover_text("RunPod billing could not be read. Horizon tries again every few minutes.");
    } else if runtime.billing.refreshing() {
        ui.small("Since creation · reading RunPod billing…");
    } else if runtime.billing.sample().is_some() {
        ui.small("Since creation · awaiting provider billing");
    }
}

fn ready_actions(ui: &mut egui::Ui, runtime: &mut super::Runtime) -> Option<Action> {
    if !rebuild::blocks_stop(runtime) && ui.add(danger_button("Stop worker…")).clicked() {
        runtime.confirmation = Confirmation::Stop;
    }
    rebuild::offer(ui, runtime)
}

/// A stop asked for, from the header or from Manage, is the only thing Manage shows
/// until it is confirmed or dismissed.
pub(super) fn confirming_stop(runtime: &super::Runtime) -> bool {
    runtime.confirmation == Confirmation::Stop && runtime.stage == Some(Stage::Ready) && !rebuild::blocks_stop(runtime)
}

fn stop_confirmation_card(ui: &mut egui::Ui, runtime: &mut super::Runtime) -> Option<Action> {
    runtime.reveal_confirmation = false;
    let mut action = None;
    egui::Frame::new()
        .fill(theme::alpha(theme::PALETTE_RED(), 18))
        .stroke(egui::Stroke::new(1.0, theme::alpha(theme::PALETTE_RED(), 110)))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(16, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(wording::stop_confirmation(runtime))
                    .size(14.0)
                    .color(theme::FG()),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.add(danger_button("Stop worker")).clicked() {
                    // Answered: a preflight that fails must show its error in Manage, not keep asking.
                    runtime.confirmation = Confirmation::None;
                    action = Some(Action::Stop);
                }
                if ui.add(action_button("Keep running")).clicked() {
                    runtime.confirmation = Confirmation::None;
                }
            });
        });
    action
}

fn bound_provider_check(ui: &mut egui::Ui, runtime: &super::Runtime) -> Option<Action> {
    if runtime.receiver.is_none()
        && runtime
            .state
            .as_ref()
            .is_some_and(|state| matches!(state.operation, horizon_core::cloud_runtime::CreateState::Bound { .. }))
    {
        return ui
            .add(action_button("Check provider"))
            .clicked()
            .then_some(Action::Reconcile);
    }
    None
}

fn deletion_action(ui: &mut egui::Ui, runtime: &mut super::Runtime) -> Option<Action> {
    if runtime.state.is_some() {
        if runtime.confirmation == Confirmation::Delete {
            ui.colored_label(egui::Color32::LIGHT_RED, wording::delete_confirmation(runtime));
            if ui.add(danger_button("Delete resources permanently")).clicked() {
                return Some(Action::Delete);
            }
            if ui.add(action_button("Keep resources")).clicked() {
                runtime.confirmation = Confirmation::None;
            }
        } else if ui.add(danger_button("Delete cloud resources…")).clicked() {
            runtime.confirmation = Confirmation::Delete;
        }
    }
    None
}

fn recovery_actions(ui: &mut egui::Ui, runtime: &mut super::Runtime) -> Option<Action> {
    ui.label("Worker status needs confirmation");
    ui.small("Check the original request with the provider. This check cannot allocate, start or delete a worker.");
    ui.collapsing("Provider-confirmed worker ID (optional)", |ui| {
        ui.small("Use an ID supplied by the provider. Horizon verifies that it belongs to this cloud.");
        ui.add(egui::TextEdit::singleline(&mut runtime.recovery_worker_id));
    });
    if runtime.checking_provider() {
        ui.spinner();
        ui.label("Checking provider…");
        None
    } else {
        let action = ui
            .add(action_button("Check provider"))
            .clicked()
            .then_some(Action::Reconcile);
        if runtime
            .state
            .as_ref()
            .is_some_and(|state| matches!(state.operation, horizon_core::cloud_runtime::CreateState::Bound { .. }))
        {
            deletion_action(ui, runtime).or(action)
        } else {
            action
        }
    }
}

fn desktop_button(ui: &mut egui::Ui, runtime: &super::Runtime) -> bool {
    let enabled = runtime
        .state
        .as_ref()
        .is_some_and(|state| state.profile.capabilities.desktop);
    ui.add_enabled(
        enabled && runtime.desktop.is_some(),
        action_button("Add desktop viewer"),
    )
    .on_disabled_hover_text(if enabled {
        "Desktop tunnel is not connected"
    } else {
        "Desktop is disabled by this cloud profile"
    })
    .clicked()
}

fn deleting(runtime: &super::Runtime) -> bool {
    runtime.receiver.is_some() && runtime.stage.is_some_and(|stage| Stage::DELETION.contains(&stage))
}

fn deleted_or_redeploying(runtime: &super::Runtime) -> bool {
    // A stage event can arrive before the deleted snapshot is replaced.
    runtime.stage == Some(Stage::Deleted)
        || (runtime.receiver.is_some()
            && runtime
                .state
                .as_ref()
                .is_some_and(|state| state.stage == Stage::Deleted))
}

/// A running deletion replaces the deployment checklist and every other action.
/// Cancel ends once the worker delete starts: a sent delete request cannot be recalled.
fn deletion_progress(ui: &mut egui::Ui, id: u32, runtime: &mut super::Runtime) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(runtime.progress.elapsed().map_or_else(
            || "Deleting cloud resources".to_owned(),
            |elapsed| {
                format!(
                    "Deleting cloud resources · {}",
                    cloud_runtime::progress::duration(elapsed)
                )
            },
        ));
    });
    stage_rows(ui, runtime, &Stage::DELETION);
    if let Some(detail) = runtime.progress.activity() {
        ui.add_space(5.0);
        ui.small(detail);
    }
    ui.add_space(8.0);
    if runtime.stage == Some(Stage::ReleaseDevices)
        && ui.add(danger_button("Cancel operation")).clicked()
        && let Some(cancel) = &runtime.cancel
    {
        cancel.cancel();
    }
    verbose_output(ui, id, runtime);
}

fn stage_rows(ui: &mut egui::Ui, runtime: &super::Runtime, stages: &[Stage]) {
    for &stage in stages {
        let current = runtime.stage == Some(stage);
        ui.label(
            RichText::new(runtime.progress.stage_label(stage))
                .size(16.0)
                .color(if current {
                    theme::PALETTE_CYAN()
                } else {
                    theme::FG_DIM()
                }),
        );
    }
}

fn verbose_output(ui: &mut egui::Ui, id: u32, runtime: &mut super::Runtime) {
    ui.add_space(4.0);
    ui.label(RichText::new("Output").size(12.0).color(theme::FG_DIM()));
    output::show(ui, id, "manage", runtime, 260.0, None);
}

fn progress_output(ui: &mut egui::Ui, id: u32, runtime: &mut super::Runtime) {
    // A failed deletion keeps its own steps beside the error until another attempt starts.
    let stages: &[Stage] = if runtime.progress.is_deletion() {
        &Stage::DELETION
    } else {
        rebuild::stages(runtime)
    };
    stage_rows(ui, runtime, stages);
    timeline::show(ui, id, runtime);
    runtime.progress.render(ui);
    rebuild::notes(ui, runtime);
    for error in runtime.error.iter().chain(&runtime.remote_release_error) {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    verbose_output(ui, id, runtime);
}
