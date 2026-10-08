//! Card controls for rebuilding a cloud's image: the offer and its confirmation, the
//! running rebuild, and the notice for a replacement left pending.
use super::super::{Confirmation, Runtime, Stage, rebuild::Kind};
use super::Action;
use crate::app::cloud_panel::runtime::{action_button, danger_button};
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
/// For a cloud on the public base image, such as a quick-start cloud: no recipe, no build.
const CONFIRMATION_BASE: &str = "Restart the worker on the base image that this Horizon version pins? Nothing is built, and an image that is already the pin restarts nothing. Running agent processes restart. Files under /workspace, including worktrees and agent logins, are kept, and this cannot change the cloud's size or capabilities.";
const OFFER: &str = "Rebuild this cloud's image from its committed recipe and restart the worker on it.";
const OFFER_BASE: &str = "Restart the worker on the base image that this Horizon version pins.";
const PREPARED_BASE: &str = "A switch to the pinned base image was being prepared and did not finish; the worker keeps its current image. Continue to restart the worker on the pinned base image, or cancel the rebuild.";
const BUILT_BASE: &str = "The pinned base image is ready, but the worker has not switched to it. Continue to restart the worker on it, or cancel to keep the current image.";
const PREPARED: &str = "An image rebuild was being prepared and did not finish; the worker keeps its current image. Continue to build the image and restart the worker on it, or cancel the rebuild.";
const BUILT: &str = "A rebuilt image is ready, but the worker has not switched to it. Continue to restart the worker on it, or cancel to keep the current image.";
const REQUESTED: &str = "The worker's image switch may be in progress. Continue to finish it, or cancel to switch the worker back to its previous image.";
const CANCEL_REQUESTED: &str = "Cancel the image switch? The worker switches back to its previous image, which restarts it again. Running agent processes restart; files under /workspace are kept.";
/// On a provider that rebuilds on a new server (`provider::Rebuild::NewServer`).
const NEW_SERVER: &str =
    " The server is released and a new one starts on the same workspace volume, with a new address.";
const REQUESTED_NEW_SERVER: &str = "The rebuild may have released the server. Continue to start a new server on the rebuilt image, or cancel to start one on the previous image; before the release the cloud keeps its server.";
const CANCEL_REQUESTED_NEW_SERVER: &str = "Cancel the rebuild? If the server was already released, a new one starts on the previous image on the same workspace volume. Running agent processes restart; files under /workspace are kept.";

/// Whether the cloud runs the public base image without a recipe, so its rebuild moves
/// it to the pinned base image instead of building one.
fn on_public_base(runtime: &Runtime) -> bool {
    runtime
        .state
        .as_ref()
        .is_some_and(|state| cloud_runtime::repository::launch::quick_start::on_public_base(&state.profile))
}

/// Whether the cloud's provider rebuilds on a new server rather than in place.
fn new_server(runtime: &Runtime) -> bool {
    runtime.state.as_ref().is_some_and(|state| {
        cloud_runtime::provider::Description::of(&state.profile).rebuild == cloud_runtime::provider::Rebuild::NewServer
    })
}

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

/// A running rebuild can be cancelled only before the worker's image switch is
/// requested, and a cancelling attempt cannot itself be cancelled.
pub(super) fn cancellable(runtime: &Runtime) -> bool {
    // The record shown during an attempt is the one it started from: a switch
    // requested before it began may already be applied.
    let requested_before = runtime
        .state
        .as_ref()
        .is_some_and(|state| state.stage == Stage::Replace);
    runtime
        .rebuild
        .as_ref()
        .is_some_and(|attempt| attempt.kind != Kind::Cancel)
        && !requested_before
        && matches!(runtime.stage, Some(Stage::Validate | Stage::Build | Stage::Push))
}

/// A replacement left pending, shown in Manage with its continue and cancel choices.
pub(super) fn has_pending(runtime: &Runtime) -> bool {
    pending(runtime.state.as_ref()).is_some() && !in_progress(runtime)
}

fn cancel_before_switch(ui: &mut egui::Ui, runtime: &Runtime) {
    if !cancellable(runtime) {
        ui.small("The worker is switching images and restarting; this step cannot be cancelled.");
    } else if let Some(cancel) = &runtime.cancel {
        if cancel.is_cancelled() {
            ui.label("Cancelling…");
        } else if ui
            .add(danger_button("Cancel rebuild"))
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
            notice(
                ui,
                if on_public_base(runtime) {
                    PREPARED_BASE
                } else {
                    PREPARED
                },
            );
            false
        }
        ReplacementPhase::Built(_) => {
            notice(ui, if on_public_base(runtime) { BUILT_BASE } else { BUILT });
            false
        }
        ReplacementPhase::Requested(_) => {
            notice(
                ui,
                if new_server(runtime) {
                    REQUESTED_NEW_SERVER
                } else {
                    REQUESTED
                },
            );
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
        if ui.add(action_button("Continue rebuild")).clicked() {
            action = Some(Action::ContinueRebuild);
        }
        if requested {
            if ui.add(danger_button("Cancel rebuild…")).clicked() {
                runtime.confirmation = Confirmation::CancelRebuild;
            }
        } else if ui
            .add(danger_button("Cancel rebuild"))
            .on_hover_text("Discards the rebuilt image; the worker keeps its current image.")
            .clicked()
        {
            action = Some(Action::CancelRebuild);
        }
    });
    action
}

fn confirm_switch_back(ui: &mut egui::Ui, runtime: &mut Runtime) -> Option<Action> {
    let (text, button) = if new_server(runtime) {
        (CANCEL_REQUESTED_NEW_SERVER, "Cancel and keep the previous image")
    } else {
        (CANCEL_REQUESTED, "Switch back and restart")
    };
    ui.colored_label(egui::Color32::LIGHT_RED, text);
    if ui.add(danger_button(button)).clicked() {
        return Some(Action::CancelRebuild);
    }
    if ui.add(action_button("Keep the new image")).clicked() {
        runtime.confirmation = Confirmation::None;
    }
    None
}

/// The Ready block's rebuild offer with its confirmation.
pub(super) fn offer(ui: &mut egui::Ui, runtime: &mut Runtime) -> Option<Action> {
    if !can_rebuild(runtime) {
        return None;
    }
    let (offer, confirmation) = if on_public_base(runtime) {
        (OFFER_BASE, CONFIRMATION_BASE)
    } else {
        (OFFER, CONFIRMATION)
    };
    if runtime.confirmation != Confirmation::Rebuild {
        let detail = if on_public_base(runtime) {
            "Move to the current base image and restart; the workspace stays."
        } else {
            "Build the committed recipe and restart; the workspace stays."
        };
        if super::section::row(ui, "Image", detail, |ui| {
            ui.add(super::section::row_button(
                ui,
                danger_button("Rebuild image & restart…"),
            ))
            .on_hover_text(offer)
            .clicked()
        }) {
            runtime.confirmation = Confirmation::Rebuild;
        }
        return None;
    }
    if new_server(runtime) {
        ui.label(format!("{confirmation}{NEW_SERVER}"));
    } else {
        ui.label(confirmation);
    }
    if ui.add(danger_button("Rebuild and restart")).clicked() {
        return Some(Action::Rebuild);
    }
    if ui.add(action_button("Keep current image")).clicked() {
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
                && cloud_runtime::deployment::replacement::rebuildable(&state.profile)
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
        ui.label(RichText::new(&note.text).size(14.0).color(if note.warning {
            theme::PALETTE_YELLOW()
        } else {
            theme::FG_DIM()
        }));
    }
}
