//! Card controls for rebuilding a cloud's image: the offer and its confirmation, the
//! running rebuild, and the notice for a replacement left pending.
use super::super::{Confirmation, Runtime, Stage, rebuild::Kind};
use super::Action;
use crate::theme;
use egui::RichText;
use horizon_core::cloud_runtime::{
    self, CreateState,
    state::{Deployment, ReplacementPhase},
};

#[cfg(test)]
mod tests;

/// A rebuild keeps its worker and worktrees. The reconnect after the switch also
/// reports Provision while it re-verifies the worker; the elapsed total includes it.
const STAGES: [Stage; 7] = [
    Stage::Validate,
    Stage::Build,
    Stage::Push,
    Stage::Replace,
    Stage::Readiness,
    Stage::Sessions,
    Stage::Ready,
];

const CONFIRMATION: &str = "Rebuild the image from the latest committed .horizon recipe with the newest agent CLIs, then restart the worker on it? Running agent processes restart. Files under /workspace, including worktrees and agent logins, are kept. Uncommitted changes are not used, and this cannot change the cloud's size or capabilities.";
const PREPARED: &str = "An image rebuild was being prepared and did not finish; the worker keeps its current image. Continue to build the image and restart the worker on it, or cancel the rebuild.";
const BUILT: &str = "A rebuilt image is ready, but the worker has not switched to it. Continue to restart the worker on it, or cancel to keep the current image.";
const REQUESTED: &str = "The worker's image switch may be in progress. Continue to finish it, or cancel to switch the worker back to its previous image.";
const CANCEL_REQUESTED: &str = "Cancel the image switch? The worker switches back to its previous image, which restarts it again. Running agent processes restart; files under /workspace are kept.";

/// The stage rows the card lists: a rebuild's while one is shown, else the deployment's.
pub(super) fn stages(runtime: &Runtime) -> &'static [Stage] {
    if runtime.rebuild.is_some() || runtime.stage == Some(Stage::Replace) {
        &STAGES
    } else {
        &Stage::ALL
    }
}

pub(super) fn in_progress(runtime: &Runtime) -> bool {
    runtime.rebuild.is_some() && runtime.receiver.is_some() && runtime.stage != Some(Stage::Ready)
}

/// A running rebuild replaces the deployment checklist and every other action. It can
/// be cancelled only before the image switch is requested; a cancelled rebuild stays
/// pending, to be continued or discarded.
pub(super) fn progress(ui: &mut egui::Ui, id: u32, runtime: &mut Runtime) {
    let Some((kind, started)) = runtime.rebuild.as_ref().map(|attempt| (attempt.kind, attempt.started)) else {
        return;
    };
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(format!(
            "{} · {}",
            kind.heading(),
            cloud_runtime::progress::duration(started.elapsed())
        ));
    });
    super::stage_rows(ui, runtime, &STAGES);
    runtime.progress.render(ui);
    notes(ui, runtime);
    ui.add_space(8.0);
    if kind != Kind::Cancel {
        cancel_before_switch(ui, runtime);
    }
    super::verbose_output(ui, id, runtime);
}

fn cancel_before_switch(ui: &mut egui::Ui, runtime: &Runtime) {
    // The record shown during an attempt is the one it started from: a switch
    // requested before it began may already be applied.
    let requested_before = runtime
        .state
        .as_ref()
        .is_some_and(|state| state.stage == Stage::Replace);
    if requested_before || !matches!(runtime.stage, Some(Stage::Validate | Stage::Build | Stage::Push)) {
        ui.small("The worker is switching images and restarting; this step cannot be cancelled.");
    } else if let Some(cancel) = &runtime.cancel {
        if cancel.is_cancelled() {
            ui.label("Cancelling…");
        } else if ui
            .button("Cancel rebuild")
            .on_hover_text(
                "Stops before the worker's image is switched. Cancelled while the committed recipe is read, the rebuild leaves nothing pending and the card offers Reconnect; later, it stays pending to be continued or discarded.",
            )
            .clicked()
        {
            cancel.cancel();
        }
    }
}

/// Explains a pending replacement and, while no other operation runs, offers to
/// continue or cancel it. Cancelling a requested switch restarts the worker again, so
/// it needs a confirmation.
pub(super) fn pending_notice(ui: &mut egui::Ui, runtime: &mut Runtime) -> Option<Action> {
    let requested = match &pending(runtime.state.as_ref())?.phase {
        ReplacementPhase::Prepared {} => {
            notice(ui, PREPARED);
            false
        }
        ReplacementPhase::Built(_) => {
            notice(ui, BUILT);
            false
        }
        ReplacementPhase::Requested(_) => {
            notice(ui, REQUESTED);
            true
        }
    };
    let action = if runtime.busy() {
        None
    } else if requested && runtime.confirmation == Confirmation::CancelRebuild {
        confirm_switch_back(ui, runtime)
    } else {
        continue_or_cancel(ui, runtime, requested)
    };
    ui.add_space(8.0);
    action
}

fn continue_or_cancel(ui: &mut egui::Ui, runtime: &mut Runtime, requested: bool) -> Option<Action> {
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        if ui.button("Continue rebuild").clicked() {
            action = Some(Action::ContinueRebuild);
        }
        if requested {
            if ui.button("Cancel rebuild…").clicked() {
                runtime.confirmation = Confirmation::CancelRebuild;
            }
        } else if ui
            .button("Cancel rebuild")
            .on_hover_text("Discards the rebuilt image; the worker keeps its current image.")
            .clicked()
        {
            action = Some(Action::CancelRebuild);
        }
    });
    action
}

fn confirm_switch_back(ui: &mut egui::Ui, runtime: &mut Runtime) -> Option<Action> {
    ui.colored_label(egui::Color32::LIGHT_RED, CANCEL_REQUESTED);
    if ui.button("Switch back and restart").clicked() {
        return Some(Action::CancelRebuild);
    }
    if ui.button("Keep the new image").clicked() {
        runtime.confirmation = Confirmation::None;
    }
    None
}

/// The Ready block's rebuild offer with its confirmation.
pub(super) fn offer(ui: &mut egui::Ui, runtime: &mut Runtime) -> Option<Action> {
    if !can_rebuild(runtime) {
        return None;
    }
    if runtime.confirmation != Confirmation::Rebuild {
        if ui
            .button("Rebuild image & restart…")
            .on_hover_text("Rebuild this cloud's image from its committed recipe and restart the worker on it.")
            .clicked()
        {
            runtime.confirmation = Confirmation::Rebuild;
        }
        return None;
    }
    ui.label(CONFIRMATION);
    if ui.button("Rebuild and restart").clicked() {
        return Some(Action::Rebuild);
    }
    if ui.button("Keep current image").clicked() {
        runtime.confirmation = Confirmation::None;
    }
    None
}

/// Stop acts on the worker as recorded, so it waits for a pending replacement.
pub(super) fn blocks_stop(runtime: &Runtime) -> bool {
    pending(runtime.state.as_ref()).is_some()
}

fn can_rebuild(runtime: &Runtime) -> bool {
    runtime.stage == Some(Stage::Ready)
        && runtime.state.as_ref().is_some_and(|state| {
            matches!(state.operation, CreateState::Bound { .. })
                && state.stage == Stage::Ready
                && !state.stop_requested
                && state.profile.build.is_some()
                && state.image_replacement.is_none()
                && state.session_restart.is_none()
        })
}

fn pending(state: Option<&Deployment>) -> Option<&cloud_runtime::state::ImageReplacement> {
    state
        .filter(|state| matches!(state.operation, CreateState::Bound { .. }))?
        .image_replacement
        .as_ref()
}

fn notice(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new("Image rebuild pending")
            .strong()
            .color(theme::PALETTE_YELLOW()),
    );
    ui.label(text);
}

/// The last attempt's outcomes, kept until another operation starts.
pub(super) fn notes(ui: &mut egui::Ui, runtime: &Runtime) {
    for note in runtime.rebuild.iter().flat_map(|attempt| &attempt.notes) {
        ui.label(RichText::new(&note.text).size(12.0).color(if note.warning {
            theme::PALETTE_YELLOW()
        } else {
            theme::FG_DIM()
        }));
    }
}
